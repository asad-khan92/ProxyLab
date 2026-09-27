//! Turning a `CONNECT` request into an intercepted HTTPS connection.
//!
//! This module owns the whole of the `CONNECT` decision: it validates the
//! target, works out the host a certificate has to be issued for, mints that
//! certificate, opens the upstream TCP connection, and — only once all of
//! that has succeeded — hands back the response that takes the client's
//! connection out of HTTP and into [`intercept`].
//!
//! The order matters. `200 Connection Established` promises the client a
//! working connection, so it is never sent on the strength of a target that
//! merely parsed: the certificate is minted and the upstream connection is
//! made first, and if either cannot be done the client is told so instead.
//!
//! What happens after the handover is [`intercept`]'s: ProxyLab terminates
//! the client's TLS session and opens its own to the origin. Nothing is
//! passed through as opaque bytes, so a `CONNECT` to something that is not
//! TLS at all is a connection that will not get past its handshake.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::ext::ReasonPhrase;
use hyper::{Request, Response, StatusCode};
use proxy_storage::{Destination, TrafficError, TrafficErrorKind};
use proxy_tls::{Host, HostError, Interceptor, TlsError};
use rules::RuleEngine;
use tokio::net::TcpStream;

use super::connect_target::{ConnectTarget, ConnectTargetError};
use super::redact;
use super::reply::{self, ProxyBody};
use crate::capture::{Capture, Recorder};
use crate::controls::Controls;
use crate::intercept::{self, InterceptError};
use crate::shutdown::Shutdown;
use crate::upstream::UpstreamManager;

/// The reason phrase a proxy answers a `CONNECT` with.
///
/// `200`'s canonical reason is `OK`, which says nothing about a connection
/// being established, so the phrase clients and packet traces expect is
/// written instead.
const CONNECTION_ESTABLISHED: ReasonPhrase = ReasonPhrase::from_static(b"Connection Established");

/// Prepares an intercepted connection for a `CONNECT` request.
///
/// On success the certificate is ready and the upstream connection is
/// already established; interception is waiting for the client's connection
/// to be handed over, which hyper does once the `200` response has been
/// written. The target is returned so the caller can answer and report it.
///
/// `recorder` is the `CONNECT`'s own recording. A `CONNECT` that becomes a
/// session discards it, because the requests inside the session are what is
/// published; one that fails at any point here publishes it, so a connection
/// that never carried a request is still visible.
#[allow(clippy::too_many_arguments)]
pub async fn establish<B>(
    client: SocketAddr,
    request: Request<B>,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    recorder: Recorder,
    shutdown: Shutdown,
) -> Result<ConnectTarget, ConnectError> {
    // The request target is whatever the client asked for, so it is reported
    // before it is trusted — redacted, because a target that is about to be
    // refused may still be carrying credentials. Nothing from inside the
    // intercepted connection is ever logged.
    println!("{client} CONNECT {}", redact::loggable(request.uri()));

    let established = prepare(
        client,
        request,
        interceptor,
        upstream,
        capture,
        rules,
        controls,
        recorder.clone(),
        shutdown,
    )
    .await;
    if let Err(error) = &established {
        recorder.fail(error.traffic_error());
    }

    established
}

/// Everything a `CONNECT` has to succeed at before the client is answered.
#[allow(clippy::too_many_arguments)]
async fn prepare<B>(
    client: SocketAddr,
    request: Request<B>,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    recorder: Recorder,
    shutdown: Shutdown,
) -> Result<ConnectTarget, ConnectError> {
    let target = ConnectTarget::parse(request.uri()).map_err(ConnectError::Target)?;
    let host = Host::parse(target.host()).map_err(ConnectError::Host)?;

    // The destination is only known now, so the recording learns it now: a
    // `CONNECT` that is refused before this names no host it could reach.
    recorder.destination(Destination::new(target.host(), target.port()));

    // Before anything is promised: a host no certificate can be issued for is
    // a host that cannot be intercepted, and finding that out after the `200`
    // would leave the client holding a connection that goes nowhere.
    interceptor
        .identity(&host)
        .map_err(ConnectError::Certificate)?;

    let socket = TcpStream::connect((target.host(), target.port()))
        .await
        .map_err(ConnectError::Upstream)?;

    // The upgrade only resolves after the response has been written, so
    // interception cannot run here; it gets its own task. Nothing is shared
    // between intercepted connections, so any number of them run side by
    // side. The task is deliberately detached: an intercepted connection
    // outlives the HTTP exchange that opened it, and shutting the listener
    // down should not wait on traffic that may never stop. It is not
    // abandoned, though — `shutdown` goes with it, so a connection that would
    // otherwise keep running is told to wind down when the proxy stops.
    let upgrade = hyper::upgrade::on(request);
    let intercepted = target.clone();
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => {
                let outcome = intercept::run(
                    client,
                    &intercepted,
                    &host,
                    &interceptor,
                    &upstream,
                    capture,
                    rules,
                    controls,
                    upgraded,
                    socket,
                    shutdown,
                )
                .await;

                report(client, &intercepted, &recorder, outcome);
            }
            // The client went away between the response and the handover.
            Err(error) => {
                recorder.fail(TrafficError::new(
                    TrafficErrorKind::ClientDisconnected,
                    "the client did not open the connection it asked for",
                ));
                eprintln!("{client} CONNECT {intercepted} was never opened: {error}");
            }
        }
    });

    println!("{client} CONNECT established {target}");

    Ok(target)
}

/// The response that takes the client's connection out of HTTP.
///
/// It carries no body and no framing headers: everything after its header
/// section is the client's TLS handshake. It also carries no `Connection`
/// header, unlike the responses the proxy writes for ordinary requests,
/// because this connection is not finished — it is being handed over.
pub fn established() -> Response<ProxyBody> {
    let mut response = Response::new(reply::empty());

    *response.status_mut() = StatusCode::OK;
    response.extensions_mut().insert(CONNECTION_ESTABLISHED);

    response
}

/// Reports a finished intercepted connection.
///
/// A failure is this connection's own, and is reported here rather than sent
/// to the client: by the time interception runs, the `200` has been written
/// and there is no HTTP response left to fail with. The message names the
/// stage that failed, which is what a developer needs — no key material, and
/// nothing the connection carried.
///
/// It also decides what the `CONNECT` publishes. A connection that never got
/// as far as a session publishes the failure that stopped it, because nothing
/// else will: there was no request inside it to record. A connection that did
/// get that far publishes nothing, because each request inside it was
/// recorded on its own — including when the session ended badly afterwards.
///
/// A transparently captured flow ends the same way and is reported by the
/// same function — see [`crate::transparent`].
pub(crate) fn report(
    client: SocketAddr,
    target: &ConnectTarget,
    recorder: &Recorder,
    outcome: Result<(), InterceptError>,
) {
    match outcome {
        Ok(()) => recorder.discard(),
        Err(error) => {
            match &error {
                InterceptError::ClientConnection(_) => recorder.discard(),
                failure => recorder.fail(failure.traffic_error()),
            }

            eprintln!("{client} CONNECT {target} was not intercepted: {error}");
        }
    }
}

/// Why a `CONNECT` request did not become an intercepted connection.
#[derive(Debug)]
pub enum ConnectError {
    /// The request target does not name a destination to connect to.
    Target(ConnectTargetError),
    /// The destination is not a host a certificate can be issued for.
    Host(HostError),
    /// No certificate could be minted for the destination.
    Certificate(TlsError),
    /// The TCP connection to the destination could not be opened.
    Upstream(io::Error),
}

impl ConnectError {
    /// Status to answer the client with.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Target(error) => error.status(),
            // The client named something that is not a host, which is a
            // request it got wrong.
            Self::Host(_) => StatusCode::BAD_REQUEST,
            // The request was fine and the proxy could not do its own job,
            // which is what `500 Internal Server Error` reports.
            Self::Certificate(_) => StatusCode::INTERNAL_SERVER_ERROR,
            // The request was fine and the proxy could not carry it out,
            // which is what `502 Bad Gateway` reports.
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
        }
    }

    /// Explanation safe to send to the client.
    ///
    /// Why the upstream connection failed is deliberately not told: refused,
    /// unresolvable, and unroutable are all reported the same way, and the
    /// underlying error is logged rather than sent.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Target(error) => error.reason(),
            Self::Host(_) => "the CONNECT target is not a host name or address",
            Self::Certificate(_) => "the proxy could not issue a certificate for this host",
            Self::Upstream(_) => "could not connect to the destination server",
        }
    }

    /// The failure as a captured exchange records it.
    pub fn traffic_error(&self) -> TrafficError {
        TrafficError::new(self.traffic_kind(), self.reason())
    }

    /// What kind of failure this is, in the captured model's terms.
    pub fn traffic_kind(&self) -> TrafficErrorKind {
        match self {
            Self::Target(_) | Self::Host(_) => TrafficErrorKind::RequestRejected,
            Self::Certificate(_) => TrafficErrorKind::ProxyFailure,
            Self::Upstream(_) => TrafficErrorKind::UpstreamConnect,
        }
    }
}

/// The log form, which may name causes the client is not told about.
impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Target(error) => error.fmt(f),
            Self::Host(error) => write!(f, "unusable host: {error}"),
            Self::Certificate(error) => write!(f, "certificate failed: {error}"),
            Self::Upstream(error) => write!(f, "connect failed: {error}"),
        }
    }
}

impl StdError for ConnectError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Target(_) => None,
            Self::Host(error) => Some(error),
            Self::Certificate(error) => Some(error),
            Self::Upstream(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::header::CONNECTION;
    use hyper::Method;
    use proxy_tls::{CertificateAuthority, UpstreamTrust};
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    fn interceptor() -> Arc<Interceptor> {
        Arc::new(
            Interceptor::new(
                CertificateAuthority::generate().expect("generate a CA"),
                &UpstreamTrust::PublicRoots,
            )
            .expect("build an interceptor"),
        )
    }

    /// A `CONNECT` request for `target`, with the empty body such a request
    /// has.
    ///
    /// It carries no upgrade to hand over, so the interception these tests
    /// arm never runs; what they cover is the decision made before that.
    fn request(target: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .method(Method::CONNECT)
            .uri(target)
            .body(Empty::new())
            .expect("valid request")
    }

    /// An address nothing is listening on: a port is bound to learn a free
    /// one and then released, so connecting to it is refused rather than
    /// ignored.
    async fn unused_addr() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);

        addr.to_string()
    }

    fn capture() -> Capture {
        Capture::with_defaults()
    }

    /// Runs a `CONNECT` through the whole of `establish`, with a capture and
    /// a recording of its own.
    async fn establishing(
        target: &str,
        interceptor: Arc<Interceptor>,
        capture: &Capture,
    ) -> Result<ConnectTarget, ConnectError> {
        let request = request(target);
        let recorder = capture.connecting(client(), redact::loggable(request.uri()), &request);
        let upstream = Arc::new(UpstreamManager::new(Arc::clone(&interceptor)));

        establish(
            client(),
            request,
            interceptor,
            upstream,
            capture.clone(),
            None,
            Controls::new(),
            recorder,
            Shutdown::never(),
        )
        .await
    }

    async fn error(target: &str) -> ConnectError {
        establishing(target, interceptor(), &capture())
            .await
            .expect_err("nothing should have been established")
    }

    /// The exchange a failed `CONNECT` published, if it published one.
    async fn published_error(target: &str) -> Option<TrafficError> {
        let capture = capture();
        let mut subscriber = capture.subscribe();

        establishing(target, interceptor(), &capture)
            .await
            .expect_err("nothing should have been established");

        subscriber
            .try_recv()
            .map(|event| event.expect("the event stream is healthy"))
            .and_then(|event| event.error.clone())
    }

    #[tokio::test]
    async fn a_reachable_destination_becomes_an_interception_target() {
        let upstream = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = upstream.local_addr().expect("local address");

        let target = establishing(&addr.to_string(), interceptor(), &capture())
            .await
            .expect("the destination is listening");

        assert_eq!(target.host(), addr.ip().to_string());
        assert_eq!(target.port(), addr.port());
    }

    #[tokio::test]
    async fn a_certificate_is_minted_before_the_client_is_answered() {
        let upstream = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = upstream.local_addr().expect("local address");
        let interceptor = interceptor();

        establishing(&addr.to_string(), Arc::clone(&interceptor), &capture())
            .await
            .expect("the destination is listening");

        assert_eq!(
            interceptor.cached_hosts(),
            1,
            "no certificate was minted for the destination"
        );
    }

    #[tokio::test]
    async fn a_destination_that_refuses_the_connection_is_a_bad_gateway() {
        let error = error(&unused_addr().await).await;

        assert!(matches!(error, ConnectError::Upstream(_)));
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            error.reason(),
            "could not connect to the destination server"
        );
    }

    #[tokio::test]
    async fn a_malformed_target_is_a_bad_request_and_establishes_nothing() {
        for target in ["example.com", "example.com:https", ":443", "/index.html"] {
            let error = error(target).await;

            assert!(
                matches!(error, ConnectError::Target(_)),
                "{target} should have been refused as a target: {error}"
            );
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn a_target_that_is_not_a_host_is_a_bad_request() {
        // Parses as an authority with a host and a port, so it gets past the
        // target check, and has an empty label — so it is not a name a
        // certificate can be issued for.
        let error = error("example..test:443").await;

        assert!(matches!(error, ConnectError::Host(_)), "{error}");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn nothing_is_connected_to_before_the_host_is_accepted() {
        // A name no certificate can be issued for: the request is refused
        // before the upstream connection is ever attempted, so the failure
        // is the host and never the connection.
        let error = error("example..test:443").await;

        assert!(!matches!(error, ConnectError::Upstream(_)), "{error}");
    }

    #[tokio::test]
    async fn the_logged_message_keeps_the_cause_the_client_is_not_told() {
        let error = error(&unused_addr().await).await;

        assert!(error.to_string().starts_with("connect failed: "));
        assert!(error.source().is_some());
        assert!(!error.reason().contains("connect failed"));
    }

    #[test]
    fn the_response_establishes_the_connection() {
        let response = established();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .extensions()
                .get::<ReasonPhrase>()
                .map(|reason| reason.as_bytes()),
            Some(&b"Connection Established"[..])
        );
    }

    #[test]
    fn the_response_frames_nothing_and_closes_nothing() {
        let response = established();

        assert!(response.headers().is_empty());
        assert!(response.headers().get(CONNECTION).is_none());
    }

    #[test]
    fn every_rejection_explains_itself_without_leaking_internals() {
        let rejections = [
            ConnectError::Target(ConnectTargetError::MissingPort),
            ConnectError::Host(HostError::Unusable),
            ConnectError::Certificate(TlsError::NoHomeDirectory),
            ConnectError::Upstream(io::Error::from(io::ErrorKind::ConnectionRefused)),
        ];

        for rejection in rejections {
            assert!(!rejection.reason().is_empty(), "{rejection:?}");
            assert!(!rejection.to_string().is_empty(), "{rejection:?}");
        }
    }

    #[tokio::test]
    async fn a_connect_that_cannot_be_carried_is_published_as_a_failed_exchange() {
        let error = published_error(&unused_addr().await)
            .await
            .expect("a failed CONNECT is published");

        assert_eq!(error.kind(), TrafficErrorKind::UpstreamConnect);
        assert_eq!(
            error.summary(),
            "could not connect to the destination server"
        );
    }

    #[tokio::test]
    async fn a_malformed_connect_target_is_published_as_a_rejected_request() {
        let error = published_error("/index.html")
            .await
            .expect("a refused CONNECT is published");

        assert_eq!(error.kind(), TrafficErrorKind::RequestRejected);
    }

    #[tokio::test]
    async fn a_connect_the_client_never_opens_is_published_as_a_disconnect() {
        let upstream = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = upstream.local_addr().expect("local address");
        let capture = capture();
        let mut subscriber = capture.subscribe();

        // The `CONNECT` succeeds, but this request was never served by a
        // server, so there is no connection to hand over and interception
        // never starts — which is what a client that goes away between the
        // `200` and the handover looks like.
        establishing(&addr.to_string(), interceptor(), &capture)
            .await
            .expect("the destination is listening");

        let event = timeout(Duration::from_secs(5), subscriber.recv())
            .await
            .expect("the handover is given up on")
            .expect("the event stream is healthy");

        assert_eq!(
            event.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::ClientDisconnected)
        );
        assert_eq!(event.response, None);
    }
}
