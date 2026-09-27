//! HTTPS interception: two TLS sessions with HTTP between them.
//!
//! Once a `CONNECT` has been answered, the client's connection carries a TLS
//! handshake. This module answers that handshake as a server, using the
//! certificate ProxyLab minted for the host the client asked for, and opens a
//! second, separate TLS session to the real origin server. Between the two,
//! the request and the response are ordinary HTTP messages the proxy can
//! read:
//!
//! ```text
//! client --TLS #1--> ProxyLab --HTTP--> ProxyLab --TLS #2--> origin
//!         ProxyLab's certificate                 origin's own certificate
//! ```
//!
//! The two sessions are independent. The client checks a certificate this
//! machine's development CA signed, so interception is something a client can
//! see rather than something done behind its back; ProxyLab separately
//! verifies the origin's real certificate, so the hop it makes on the
//! client's behalf is no less checked than the one the client would have made
//! itself. Neither session's keys, and nothing either session carries, is
//! written to a log.
//!
//! # Which HTTP, on each side
//!
//! Each handshake also settles which version of HTTP is inside it, over ALPN,
//! and the two answers are independent:
//!
//! ```text
//!            ALPN                              ALPN
//! client ──────────▶ ProxyLab ────────────────────▶ origin
//!   h2  or http/1.1              h2  or http/1.1
//! ```
//!
//! So all four combinations happen, and none of them is a special case: a
//! client speaking HTTP/2 to a server that only speaks HTTP/1.1 is the point
//! of a proxy that translates, and the translation is [`crate::origin`]'s.
//! This module only decides which framing to put on each side.
//!
//! Nothing is forced. An origin that does not offer `h2` is spoken to in
//! HTTP/1.1, and a client that does not ask for `h2` is answered in HTTP/1.1
//! — exactly as before HTTP/2 existed here.
//!
//! # How long a session lasts
//!
//! An HTTP/1.1 session carries one request and closes, which is what the
//! proxy does on a plaintext connection too (see
//! [`crate::http1::headers::set_close`]); a client with more to ask opens
//! another `CONNECT`.
//!
//! An HTTP/2 session does not work that way and is not made to. It stays open
//! and carries as many concurrent streams as the client opens, each its own
//! exchange with its own capture, and it ends when the client is finished
//! with it — or when the proxy stops, which it is told about through
//! [`Shutdown`].
//!
//! # One session in, several connections out
//!
//! An intercepted connection does not own its connection to the origin. The
//! TLS session opened to prove the origin reachable is handed to
//! [`crate::upstream`], and every request inside the intercepted connection
//! asks that manager for one:
//!
//! ```text
//! client HTTP/2 ──stream 1──▶ ┐                ┌──▶ HTTP/1.1 connection A
//!               ──stream 2──▶ ├─▶ UpstreamManager ─▶ HTTP/1.1 connection B
//!               ──stream 3──▶ ┘                └──▶ HTTP/1.1 connection C
//! ```
//!
//! That is what stops concurrent streams from queuing behind one another when
//! the origin only speaks HTTP/1.1. An origin that speaks HTTP/2 is reached
//! over one connection, as it should be, and the streams multiplex on it.
//!
//! The connections are also not this session's to keep: they are the proxy's,
//! so a connection this session opened answers the next request to the same
//! origin whichever client makes it.

mod exchange;

use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::header::HeaderValue;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::Request;
use hyper_util::rt::TokioIo;
use proxy_storage::{TrafficError, TrafficErrorKind};
use proxy_tls::{ClientSession, Host, Interceptor, Protocol, TlsError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use rules::RuleEngine;

use crate::capture::Capture;
use crate::controls::Controls;
use crate::http1::connect_target::ConnectTarget;
use crate::http1::upstream::UpstreamError;
use crate::http2;
use crate::shutdown::Shutdown;
use crate::upstream::{UpstreamDestination, UpstreamManager};

/// Intercepts the connection a `CONNECT` handed over.
///
/// `upgraded` is the client's connection, now out of HTTP and about to start
/// a TLS handshake. `socket` is the already-open TCP connection to the origin,
/// which the second TLS session is established over. `upstream` is the proxy's
/// connection manager, which that session is handed to once it is up and which
/// every request inside the intercepted connection gets its connection from.
///
/// Returns once the intercepted connection is finished and both sessions are
/// closed. Every failure is this connection's alone: nothing here is shared
/// with any other intercepted connection.
///
/// `capture` records what the session carries — the decrypted requests and
/// responses, never a TLS byte — and publishes an exchange per request, in
/// the same model a plaintext request produces. An HTTP/2 session publishes
/// one per stream, so a connection carrying four streams is four exchanges
/// and never one.
///
/// `shutdown` is how a connection that would otherwise outlive the proxy is
/// told to wind down. An HTTP/1.1 session ignores it, because it is over
/// after one exchange either way.
///
/// Each argument is one thing the connection needs, and none of them travel
/// together anywhere else: three come from the `CONNECT` that opened it, two
/// are the proxy's own, one is how it is stopped, and two are the sockets. A
/// struct to hold them would exist only here, and only to be taken apart
/// again.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: SocketAddr,
    target: &ConnectTarget,
    host: &Host,
    interceptor: &Arc<Interceptor>,
    upstream: &Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    upgraded: Upgraded,
    socket: TcpStream,
    shutdown: Shutdown,
) -> Result<(), InterceptError> {
    let (authority, destination) = endpoints(target, host)?;

    let client_session = interceptor
        .accept_client(host, TokioIo::new(upgraded))
        .await
        .map_err(InterceptError::ClientHandshake)?;

    carry(
        client,
        target,
        host,
        authority,
        destination,
        client_session,
        socket,
        Session {
            interceptor,
            upstream,
            capture,
            rules,
            controls,
            shutdown,
        },
    )
    .await
}

/// Intercepts a transparently captured flow, whose host is not known yet.
///
/// The same interception as [`run`], with one difference in where the host
/// comes from. A `CONNECT` names the host it wants; a flow captured below the
/// application was made to an address, and the host the client means is the
/// one it names in its TLS hello. So the client's handshake is read first,
/// the certificate is minted for that name, and the origin's certificate is
/// verified against the same name — which is exactly what the client would
/// have checked had it reached the origin itself. A hello naming nothing is
/// answered, and verified, as the address.
///
/// `socket` is already connected to the address the client chose; nothing is
/// looked up again by name, so the session goes where the client was going.
/// `stream` is the client's side of the flow, from its first byte.
#[allow(clippy::too_many_arguments)]
pub async fn run_named<S>(
    client: SocketAddr,
    destination: &ConnectTarget,
    interceptor: &Arc<Interceptor>,
    upstream: &Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    stream: S,
    socket: TcpStream,
    shutdown: Shutdown,
) -> Result<(), InterceptError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let fallback =
        Host::parse(destination.host()).map_err(|_| InterceptError::UnusableAuthority)?;

    let (host, client_session) = interceptor
        .accept_client_named(&fallback, stream)
        .await
        .map_err(InterceptError::ClientHandshake)?;
    let target = ConnectTarget::named(host.as_str(), destination.port());
    let (authority, upstream_destination) = endpoints(&target, &host)?;

    carry(
        client,
        &target,
        &host,
        authority,
        upstream_destination,
        client_session,
        socket,
        Session {
            interceptor,
            upstream,
            capture,
            rules,
            controls,
            shutdown,
        },
    )
    .await
}

/// What an intercepted session is carried with: the proxy's own parts, which
/// every session shares, and how it is told to stop.
struct Session<'a> {
    interceptor: &'a Arc<Interceptor>,
    upstream: &'a Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
}

/// The `Host` header and the upstream destination a session's requests use.
fn endpoints(
    target: &ConnectTarget,
    host: &Host,
) -> Result<(HeaderValue, UpstreamDestination), InterceptError> {
    let authority = authority(target).ok_or(InterceptError::UnusableAuthority)?;
    let destination = UpstreamDestination::tls(host, target.port())
        .map_err(|_| InterceptError::UnusableAuthority)?;

    Ok((authority, destination))
}

/// Everything after the client's handshake: the origin's session, and the
/// requests carried between the two.
///
/// Shared by [`run`] and [`run_named`], which differ only in how the client's
/// handshake was answered.
#[allow(clippy::too_many_arguments)]
async fn carry<S>(
    client: SocketAddr,
    target: &ConnectTarget,
    host: &Host,
    authority: HeaderValue,
    destination: UpstreamDestination,
    client_session: ClientSession<S>,
    socket: TcpStream,
    session: Session<'_>,
) -> Result<(), InterceptError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let Session {
        interceptor,
        upstream,
        capture,
        rules,
        controls,
        shutdown,
    } = session;

    let client_protocol = proxy_tls::client_protocol(&client_session);
    println!(
        "{client} TLS client handshake established {target} ({})",
        protocol_name(client_protocol)
    );

    // Only now, with the client's session up, is the origin's opened. The
    // certificate that comes back is verified on the way; a failure here ends
    // the connection rather than falling back to anything, because the one
    // thing worse than failing to intercept HTTPS is quietly not using TLS.
    let upstream_session = interceptor
        .connect_upstream(host, socket)
        .await
        .map_err(InterceptError::UpstreamHandshake)?;
    let upstream_protocol = proxy_tls::upstream_protocol(&upstream_session);
    println!(
        "{client} TLS upstream handshake established {target} ({})",
        protocol_name(upstream_protocol)
    );

    // The session that proved the origin reachable is not thrown away: it is
    // handed to the manager, which keeps it under this destination's key, and
    // the first request inside the intercepted connection picks it up. An
    // HTTP/2 session becomes *the* connection to this origin and every stream
    // multiplexes over it; an HTTP/1.1 one becomes the first connection in the
    // pool, and requests that arrive while it is busy take connections of
    // their own rather than queuing behind it.
    upstream
        .adopt(&destination, upstream_session, upstream_protocol)
        .await
        .map_err(InterceptError::Upstream)?;

    let exchange = {
        let target = target.clone();
        let upstream = Arc::clone(upstream);
        let shutdown = shutdown.clone();

        move |request: Request<Incoming>| {
            let destination = destination.clone();
            let target = target.clone();
            let authority = authority.clone();
            let capture = capture.clone();
            let rules = rules.clone();
            let controls = controls.clone();
            let upstream = Arc::clone(&upstream);
            let shutdown = shutdown.clone();

            async move {
                exchange::forward(
                    client,
                    &target,
                    &authority,
                    &destination,
                    &upstream,
                    &capture,
                    rules.as_deref(),
                    &controls,
                    request,
                    shutdown,
                )
                .await
            }
        }
    };

    let served = match client_protocol {
        Protocol::Http11 => {
            http1::Builder::new()
                .serve_connection(
                    TokioIo::new(client_session),
                    service_fn(move |request| {
                        let answering = exchange(request);

                        async move { Ok::<_, Infallible>(answering.await) }
                    }),
                )
                // A `wss://` upgrade inside this session hands the session
                // over the same way a `CONNECT` hands over a socket: once the
                // `101` is written, what follows is not HTTP and hyper gives
                // it to whoever asked for it.
                .with_upgrades()
                .await
        }
        Protocol::Http2 => http2::serve(client_session, &shutdown, exchange).await,
    };
    println!("{client} HTTPS connection closed {target}");

    served.map_err(InterceptError::ClientConnection)
}

/// How a protocol is named in a log line.
fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Http11 => "HTTP/1.1",
        Protocol::Http2 => "HTTP/2",
    }
}

/// The `Host` header value for the `CONNECT` authority.
///
/// `None` for an authority that cannot be a header value, which is a host
/// that has been validated as a host and still cannot be named in a request.
fn authority(target: &ConnectTarget) -> Option<HeaderValue> {
    HeaderValue::from_str(&target.to_string()).ok()
}

/// Why an intercepted connection ended before its exchanges were finished.
#[derive(Debug)]
pub enum InterceptError {
    /// The `CONNECT` authority cannot be written as a `Host` header.
    UnusableAuthority,
    /// The client's TLS handshake failed.
    ///
    /// Most often because the development CA is not installed, which is the
    /// client refusing the certificate — not something to work around.
    ClientHandshake(TlsError),
    /// The origin's TLS handshake failed.
    ///
    /// Its certificate did not verify, or the server would not agree on a
    /// protocol version or cipher suite.
    UpstreamHandshake(TlsError),
    /// The origin did not start an HTTP conversation inside its session.
    Upstream(UpstreamError),
    /// The client's HTTP conversation inside its session ended badly.
    ClientConnection(hyper::Error),
}

impl InterceptError {
    /// The failure as a captured exchange records it.
    ///
    /// A curated phrase per stage. A TLS error's own message names ciphers,
    /// certificates, and protocol versions; none of that is repeated here,
    /// because a captured exchange is a thing that gets shown and exported.
    pub fn traffic_error(&self) -> TrafficError {
        match self {
            Self::UnusableAuthority => TrafficError::new(
                TrafficErrorKind::RequestRejected,
                "the CONNECT target cannot be sent as a Host header",
            ),
            Self::ClientHandshake(_) => TrafficError::new(
                TrafficErrorKind::ClientTls,
                "the client did not accept the proxy's certificate",
            ),
            Self::UpstreamHandshake(_) => TrafficError::new(
                TrafficErrorKind::UpstreamTls,
                "the TLS handshake with the destination server failed",
            ),
            Self::Upstream(error) => TrafficError::new(error.traffic_kind(), error.reason()),
            Self::ClientConnection(_) => TrafficError::new(
                TrafficErrorKind::ClientDisconnected,
                "the client connection ended before the exchange did",
            ),
        }
    }
}

/// The log form. Safe to write: a TLS error names an algorithm, a
/// certificate problem, or a protocol version, and never key material.
impl fmt::Display for InterceptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnusableAuthority => {
                f.write_str("the CONNECT target cannot be sent as a Host header")
            }
            Self::ClientHandshake(error) => write!(f, "client TLS handshake failed: {error}"),
            Self::UpstreamHandshake(error) => write!(f, "upstream TLS handshake failed: {error}"),
            Self::Upstream(error) => write!(f, "upstream HTTP failed: {error}"),
            Self::ClientConnection(error) => write!(f, "client HTTP failed: {error}"),
        }
    }
}

impl StdError for InterceptError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::UnusableAuthority => None,
            Self::ClientHandshake(error) | Self::UpstreamHandshake(error) => Some(error),
            Self::Upstream(error) => Some(error),
            Self::ClientConnection(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use hyper::Uri;

    fn target(authority: &str) -> ConnectTarget {
        let uri: Uri = authority.parse().expect("valid uri");
        ConnectTarget::parse(&uri).expect("tunnellable target")
    }

    #[test]
    fn an_authority_becomes_a_host_header() {
        assert_eq!(
            authority(&target("example.com:443")).expect("a usable authority"),
            "example.com:443"
        );
        assert_eq!(
            authority(&target("[::1]:8443")).expect("a usable authority"),
            "[::1]:8443"
        );
    }

    #[test]
    fn every_protocol_has_a_name_a_log_line_can_use() {
        assert_eq!(protocol_name(Protocol::Http11), "HTTP/1.1");
        assert_eq!(protocol_name(Protocol::Http2), "HTTP/2");
    }

    #[test]
    fn every_failure_explains_itself_without_leaking_key_material() {
        let failures = [
            InterceptError::UnusableAuthority,
            InterceptError::ClientHandshake(TlsError::NoHomeDirectory),
            InterceptError::UpstreamHandshake(TlsError::ClientHandshake(std::io::Error::from(
                std::io::ErrorKind::UnexpectedEof,
            ))),
            InterceptError::Upstream(UpstreamError::Connect(std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused,
            ))),
        ];

        for failure in failures {
            assert!(!failure.to_string().is_empty(), "{failure:?}");
        }
    }
}
