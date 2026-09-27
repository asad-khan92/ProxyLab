//! Starting an HTTP/1.1 conversation with an origin server.
//!
//! This layer is deliberately thin: given a stream the origin is reachable
//! over — a plain socket, or a TLS session — it performs the HTTP/1.1
//! handshake, puts the connection on a task of its own, and hands back the
//! sender requests go out on. What that sender is worth, how long it is kept,
//! and who gets it next is [`crate::upstream`]'s business, not this module's.
//!
//! [`UpstreamError`] lives here too, and is the whole vocabulary the proxy
//! has for "the origin did not answer": every path that reaches an origin —
//! plaintext, intercepted, mapped by a rule, replayed — fails in these terms
//! and answers the client with the same `502`.

use std::error::Error as StdError;
use std::fmt;
use std::io;

use hyper::body::Body;
use hyper::client::conn::http1::SendRequest;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use proxy_storage::TrafficErrorKind;
use proxy_tls::TlsError;
use tokio::io::{AsyncRead, AsyncWrite};

/// Starts an HTTP/1.1 conversation over `stream` and returns its sender.
///
/// `stream` is whatever the origin is reachable over: a plain socket, or the
/// TLS session an intercepted connection or a mapped destination established.
/// Requests are sent on the returned sender; the response head comes back as
/// soon as the origin has written it, with the body still arriving.
pub async fn connection<S, B>(stream: S) -> Result<SendRequest<B>, UpstreamError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(UpstreamError::Handshake)?;

    // The connection drives the socket for as long as the response body is
    // still arriving — and, once the connection is pooled, for as long as it
    // sits idle — so it runs in its own task. It ends when the sender is
    // dropped, which is what dropping a pooled connection does.
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("Upstream connection error: {error}");
        }
    });

    Ok(sender)
}

/// The same, on a connection that can hand its socket over.
///
/// The difference is one call — `with_upgrades` — and what it means: this
/// connection is allowed to stop being an HTTP connection. When the origin
/// answers `101 Switching Protocols`, the connection task hands the socket to
/// whoever asked for the upgrade instead of reading another response on it.
///
/// It is a separate function rather than the default because the two kinds of
/// connection have different fates. An ordinary one is pooled and carries
/// request after request; this one carries exactly one request, and either
/// becomes a tunnel or is closed. A pooled connection that quietly handed its
/// socket to an upgrade nobody asked for would be a connection the pool
/// thinks it still has.
pub async fn upgradable_connection<S, B>(stream: S) -> Result<SendRequest<B>, UpstreamError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(UpstreamError::Handshake)?;

    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            eprintln!("Upstream connection error: {error}");
        }
    });

    Ok(sender)
}

/// Why an exchange with the origin did not produce a response.
#[derive(Debug)]
pub enum UpstreamError {
    /// The TCP connection to the origin could not be opened.
    Connect(io::Error),
    /// The origin did not start an HTTP conversation.
    Handshake(hyper::Error),
    /// The request could not be sent, or no usable response came back.
    Send(hyper::Error),
    /// The TLS handshake with the destination failed.
    Tls(TlsError),
    /// The destination cannot be named in a URL.
    ///
    /// A host that has already been validated as one and still cannot be
    /// written as an authority, which means a stored rule or a stored request
    /// and this build disagree about what a host is.
    UnusableHost,
    /// The proxy is stopping and is opening no more upstream connections.
    Stopping,
}

impl UpstreamError {
    /// Status to answer the client with.
    ///
    /// Every failure here is the proxy failing to get an answer from the
    /// origin, which is exactly what `502 Bad Gateway` reports.
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_GATEWAY
    }

    /// Explanation safe to send to the client.
    ///
    /// Deliberately coarse: the client is told the proxy could not reach the
    /// origin, and the underlying error is logged rather than sent.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Connect(_) => "could not connect to the destination server",
            Self::Handshake(_) | Self::Send(_) => "no valid response from the destination server",
            Self::Tls(_) => "the TLS handshake with the destination server failed",
            Self::UnusableHost => "the destination is not a host this proxy can reach",
            Self::Stopping => "the proxy is shutting down",
        }
    }

    /// How the failure is described in a captured exchange.
    ///
    /// A connection that could not be opened and an origin that answered
    /// with nothing are different problems to whoever is debugging, so the
    /// capture keeps them apart even though the client is told the same
    /// thing about both.
    pub fn traffic_kind(&self) -> TrafficErrorKind {
        match self {
            Self::Connect(_) => TrafficErrorKind::UpstreamConnect,
            Self::Handshake(_) | Self::Send(_) => TrafficErrorKind::UpstreamHttp,
            Self::Tls(_) => TrafficErrorKind::UpstreamTls,
            // The proxy's own failure to carry the request, not the
            // destination's failure to answer.
            Self::UnusableHost | Self::Stopping => TrafficErrorKind::ProxyFailure,
        }
    }
}

impl fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(f, "connect failed: {error}"),
            Self::Handshake(error) => write!(f, "handshake failed: {error}"),
            Self::Send(error) => write!(f, "request failed: {error}"),
            Self::Tls(error) => write!(f, "upstream TLS handshake failed: {error}"),
            Self::UnusableHost => f.write_str("the destination cannot be named in a URL"),
            Self::Stopping => f.write_str("the proxy is shutting down"),
        }
    }
}

impl StdError for UpstreamError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Connect(error) => Some(error),
            Self::Handshake(error) | Self::Send(error) => Some(error),
            Self::Tls(error) => Some(error),
            Self::UnusableHost | Self::Stopping => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use tokio::net::{TcpListener, TcpStream};

    /// An address nothing is listening on: a port is bound to learn a free one
    /// and then released, so connecting to it is refused rather than ignored.
    async fn unused_addr() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);

        addr
    }

    #[tokio::test]
    async fn a_conversation_can_be_started_on_an_open_socket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let accepting = tokio::spawn(async move { listener.accept().await });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let sender = connection::<_, Empty<Bytes>>(stream)
            .await
            .expect("the HTTP/1.1 handshake is not a round trip");

        assert!(!sender.is_closed());
        drop(accepting);
    }

    #[tokio::test]
    async fn every_upstream_failure_is_a_bad_gateway() {
        let failures = [
            UpstreamError::Connect(io::Error::from(io::ErrorKind::ConnectionRefused)),
            UpstreamError::Tls(TlsError::NoHomeDirectory),
            UpstreamError::UnusableHost,
            UpstreamError::Stopping,
        ];

        for failure in failures {
            assert_eq!(failure.status(), StatusCode::BAD_GATEWAY);
            assert!(!failure.reason().is_empty(), "{failure:?}");
            assert!(!failure.to_string().is_empty(), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_connection_is_captured_as_a_connect_failure() {
        let error = UpstreamError::Connect(
            TcpStream::connect(unused_addr().await)
                .await
                .expect_err("nothing is listening"),
        );

        assert_eq!(error.traffic_kind(), TrafficErrorKind::UpstreamConnect);
        assert_eq!(
            error.reason(),
            "could not connect to the destination server"
        );
    }

    #[test]
    fn a_failed_tls_handshake_is_captured_as_a_tls_failure() {
        assert_eq!(
            UpstreamError::Tls(TlsError::NoHomeDirectory).traffic_kind(),
            TrafficErrorKind::UpstreamTls
        );
    }

    #[test]
    fn a_destination_the_proxy_cannot_name_is_its_own_failure() {
        assert_eq!(
            UpstreamError::UnusableHost.traffic_kind(),
            TrafficErrorKind::ProxyFailure
        );
        assert_eq!(
            UpstreamError::Stopping.traffic_kind(),
            TrafficErrorKind::ProxyFailure
        );
    }

    #[tokio::test]
    async fn the_logged_message_keeps_the_cause_the_client_is_not_told() {
        let error = UpstreamError::Connect(
            TcpStream::connect(unused_addr().await)
                .await
                .expect_err("nothing is listening"),
        );

        assert!(error.to_string().starts_with("connect failed: "));
        assert!(error.source().is_some());
        assert!(!error.reason().contains("connect failed"));
    }
}
