//! One HTTP/1.1 proxy exchange: client request in, client response out.
//!
//! This module holds the decision-making. It validates what the client asked
//! for, rewrites the request into the origin form an origin server expects,
//! hands it to [`upstream`], and cleans up the response on the way back. A
//! `CONNECT` request is not forwarded at all; it goes to [`connect`], which
//! answers it by intercepting the connection. Either way this module never
//! fails: a request that cannot be carried becomes an HTTP error response, so
//! the client always gets an answer.

use std::error::Error as StdError;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode, Version};
use proxy_storage::{TrafficError, TrafficErrorKind};
use proxy_tls::Interceptor;

use rules::{ModifyResponse, RuleDecision, RuleEngine, Transformation};

use super::connect::{self, ConnectError};
use super::headers;
use super::redact;
use super::reply::{self, ProxyBody};
use super::ruled;
use super::setup;
use super::target::{Target, TargetError};
use super::upstream::UpstreamError;
use crate::capture::{message, Capture, Recorder};
use crate::controls::pause::{self, RequestOutcome, ResponseOutcome};
use crate::controls::{Controls, Exchange, StagedBody};
use crate::modify;
use crate::shutdown::Shutdown;
use crate::sse;
use crate::upstream::{self, UpstreamDestination, UpstreamManager};
use crate::websocket;

/// What a client is told about an exchange a developer stopped.
///
/// The same phrase for either half, because it is the same thing: ProxyLab
/// was asked to produce no answer, and a `502` is what a proxy says when it
/// has none to give. Nothing about the breakpoint reaches the client.
pub const CANCELLED_AT_BREAKPOINT: &str = "cancelled at a ProxyLab breakpoint";

/// Handles one request from `client` and produces the response to send back.
///
/// Every request that reaches here is captured, whether it is carried or
/// refused: `capture` records it, and publishes the finished exchange once
/// the response has been forwarded — or once it is clear that none will be.
#[allow(clippy::too_many_arguments)]
pub async fn proxy(
    client: SocketAddr,
    request: Request<Incoming>,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: &Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
) -> Response<ProxyBody> {
    let method = request.method().clone();
    let uri = request.uri().clone();

    if method == Method::CONNECT {
        // An intercepted connection is not a request/response exchange, and
        // it outlives this one, so `connect` reports its own lifecycle
        // instead of being summarised here by a status.
        return match intercept_connect(
            client,
            request,
            interceptor,
            upstream,
            capture,
            rules,
            controls,
            shutdown,
        )
        .await
        {
            Ok(response) => response,
            Err(rejection) => reject(client, &method, &redact::loggable(&uri), &rejection),
        };
    }

    // Worked out the way the rules work it out, so a breakpoint and a rule
    // agree about what host a request is for. A target that cannot be read
    // names no host, which matches nothing — and the request is about to be
    // refused for exactly that reason anyway.
    let parsed = Target::parse(&uri);
    let exchange = Exchange::forwarded(
        parsed.as_ref().map(Target::host).unwrap_or_default(),
        uri.path(),
        method.as_str(),
        message::forwarded_url(&uri),
    );

    // Before the recording starts, so the exchange this is remembered by is
    // the request that was actually sent. See [`crate::controls`].
    let (request, cancelled) =
        match pause::request(controls.breakpoints(), &exchange, request, &shutdown).await {
            RequestOutcome::Carry(request) => (request, false),
            RequestOutcome::Cancelled(request) => (request, true),
        };

    // Worked out again, because an edit may have changed where the request is
    // going and what it is called. Everything from here on — the recording,
    // the reporting, the rules, and a response breakpoint — is about the
    // request as it now stands.
    let method = request.method().clone();
    let uri = request.uri().clone();
    let parsed = Target::parse(&uri);
    let exchange = Exchange::forwarded(
        parsed.as_ref().map(Target::host).unwrap_or_default(),
        uri.path(),
        method.as_str(),
        message::forwarded_url(&uri),
    );

    let recorder = capture.forwarded(client, &request);
    // A request target is whatever the client sent, credentials included, so
    // every line that names one names the redacted form. What was captured
    // is unaffected: see `redact`.
    let target = redact::loggable(&uri);

    if cancelled {
        recorder.fail(TrafficError::new(
            TrafficErrorKind::RequestRejected,
            CANCELLED_AT_BREAKPOINT,
        ));
        eprintln!("{client} {method} {target} -> 502 ({CANCELLED_AT_BREAKPOINT})");

        return reply::error(StatusCode::BAD_GATEWAY, CANCELLED_AT_BREAKPOINT);
    }

    match forward(
        client,
        request,
        &recorder,
        &exchange,
        &interceptor,
        &upstream,
        rules.as_deref(),
        &controls,
        shutdown,
    )
    .await
    {
        Ok(response) => {
            println!(
                "{}",
                forwarded_report(client, &method, &target, response.status())
            );
            response
        }
        Err(rejection) => {
            recorder.fail(rejection.traffic_error());

            reject(client, &method, &target, &rejection)
        }
    }
}

/// The line reporting a request the proxy carried.
///
/// Method, target, and status: what the exchange was, and nothing it
/// carried. `target` is already in its loggable form.
fn forwarded_report(
    client: SocketAddr,
    method: &Method,
    target: &str,
    status: StatusCode,
) -> String {
    format!("{client} {method} {target} -> {status}")
}

/// The line reporting a request the proxy would not carry.
///
/// The same as [`forwarded_report`], and why. The reason may name causes the
/// client is not told about; it never names anything the request carried.
fn rejected_report(
    client: SocketAddr,
    method: &Method,
    target: &str,
    rejection: &Rejection,
    status: StatusCode,
) -> String {
    format!("{client} {method} {target} -> {status} ({rejection})")
}

/// Answers a request the proxy would not carry, and reports why.
///
/// `target` is already in its loggable form: this is where a refused request
/// is reported, and a refused request is exactly the one whose target is
/// most likely to be carrying something it should not.
fn reject(
    client: SocketAddr,
    method: &Method,
    target: &str,
    rejection: &Rejection,
) -> Response<ProxyBody> {
    let response = error_response(rejection);
    eprintln!(
        "{}",
        rejected_report(client, method, target, rejection, response.status())
    );

    response
}

/// Prepares an intercepted connection to the `CONNECT` target, or explains
/// why it cannot be.
///
/// The response is only produced once the certificate is minted and the
/// upstream connection is up, so the client is never promised a connection
/// that does not exist.
#[allow(clippy::too_many_arguments)]
async fn intercept_connect<B>(
    client: SocketAddr,
    request: Request<B>,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: &Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
) -> Result<Response<ProxyBody>, Rejection> {
    // A `CONNECT` that becomes a session publishes nothing of its own — the
    // requests inside it are the traffic. This recording is for the one that
    // does not get that far, which would otherwise be a request that simply
    // vanished.
    let recorder = capture.connecting(client, redact::loggable(request.uri()), &request);

    if !matches!(request.version(), Version::HTTP_10 | Version::HTTP_11) {
        let rejection = Rejection::UnsupportedVersion(request.version());
        recorder.fail(rejection.traffic_error());

        return Err(rejection);
    }

    // The engine itself, not a snapshot of it: an intercepted session
    // outlives this exchange, and each request inside it is evaluated against
    // the rules as they are when it arrives.
    connect::establish(
        client,
        request,
        interceptor,
        upstream,
        capture.clone(),
        rules,
        controls,
        recorder,
        shutdown,
    )
    .await
    .map_err(Rejection::Connect)?;

    Ok(connect::established())
}

/// Forwards a request to its origin, or explains why it cannot be.
///
/// `recorder` is fed as the exchange happens: the request body as it goes
/// upstream, the response head when it arrives, and the response body as it
/// goes back. Capture is a copy taken on the way past, so nothing here waits
/// for a body or holds one up.
#[allow(clippy::too_many_arguments)]
async fn forward<B>(
    client: SocketAddr,
    request: Request<StagedBody<B>>,
    recorder: &Recorder,
    exchange: &Exchange,
    interceptor: &Interceptor,
    upstream: &UpstreamManager,
    rules: Option<&RuleEngine>,
    controls: &Controls,
    shutdown: Shutdown,
) -> Result<Response<ProxyBody>, Rejection>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>> + Send + Sync + 'static,
{
    if !matches!(request.version(), Version::HTTP_10 | Version::HTTP_11) {
        return Err(Rejection::UnsupportedVersion(request.version()));
    }

    // The target is worked out first because the rules are matched against
    // where the request is going, and a request whose target cannot be read
    // is refused before any rule is consulted: there is no host to match on.
    let target = Target::parse(request.uri()).map_err(Rejection::Target)?;

    // ProxyLab answering as itself, before any rule is consulted: this host
    // is not a destination and never resolves, so a rule matching on where a
    // request is going has nothing to say about it — and a rule that blocked
    // it would be a phone unable to fetch the certificate that would let the
    // rest of its traffic be seen at all. It is still captured, so the
    // fetch appears in the traffic list like everything else.
    if setup::is_setup_host(target.host()) {
        return Ok(ruled::generated(
            setup::answer(request.uri().path(), interceptor.authority()),
            recorder,
        ));
    }

    // Before the rules and before anything is sent: the delay a developer
    // asked for is what their device's link would have cost, and every route
    // out of here — carried, blocked, or answered from a file — is on the far
    // side of it.
    controls.throttle().delay().await;

    let outcome = ruled::decide(
        rules,
        target.host(),
        request.uri().path(),
        request.method().as_str(),
    );

    match &outcome.decision {
        RuleDecision::Allow { .. } | RuleDecision::MapRemote { .. } => {}
        RuleDecision::Block { rule, block } => {
            println!("Rule {rule} blocked {}", redact::loggable(request.uri()));

            return Ok(ruled::blocked(block, recorder));
        }
        RuleDecision::MapLocal { rule, map } => {
            println!(
                "Rule {rule} answered {} locally",
                redact::loggable(request.uri())
            );

            return Ok(ruled::mapped_local(map, recorder).await);
        }
        RuleDecision::Redirect { rule, redirect } => {
            println!(
                "Rule {rule} redirected {} to {}",
                redact::loggable(request.uri()),
                redirect.location()
            );

            return Ok(ruled::redirected(redirect, recorder));
        }
    }

    // The request is going to an origin, so this is where the Modify Request
    // rules above the decision change it — after the breakpoint, before Map
    // Remote rewrites where it goes. A WebSocket handshake is carried as it
    // came. See [`crate::modify`].
    let (request, modified) = if websocket::is_upgrade(&request) {
        (request, false)
    } else {
        modify::request(request, &outcome.request).await
    };
    let (target, exchange) = if modified {
        let url = message::forwarded_url(request.uri());
        recorder.request_edited(&request, url.clone());

        let target = Target::parse(request.uri()).map_err(Rejection::Target)?;
        let exchange = Exchange::forwarded(
            target.host(),
            request.uri().path(),
            request.method().as_str(),
            url,
        );

        (target, exchange)
    } else {
        (target, exchange.clone())
    };

    if let RuleDecision::MapRemote { rule, map } = &outcome.decision {
        println!(
            "Rule {rule} mapped {} to {}",
            redact::loggable(request.uri()),
            map.target
        );

        return forward_mapped(
            client,
            request,
            recorder,
            &exchange,
            upstream,
            &map.target,
            controls,
            shutdown,
            &outcome.response,
        )
        .await;
    }

    let destination = UpstreamDestination::plaintext(target.host(), target.port())
        .map_err(Rejection::Upstream)?;

    // A WebSocket upgrade is carried from here on by a path of its own: it
    // needs a connection nothing else will use, and the headers a proxy
    // strips from every other request are the ones it cannot do without. See
    // [`crate::websocket`].
    if websocket::is_upgrade(&request) {
        let url = redact::loggable(request.uri());
        println!("{client} WebSocket upgrade {url}");

        return websocket::forward(
            client,
            &url,
            &destination,
            upstream,
            recorder,
            &exchange,
            into_origin_request(request, &target),
            controls,
            shutdown,
        )
        .await
        .map_err(Rejection::Upstream);
    }

    let origin_request = into_origin_request(request, &target)
        .map(|body| upstream::boxed(controls.throttle().upload(recorder.capture_request(body))));

    recorder.upstream_connect_started();
    let response = upstream
        .send(&destination, origin_request)
        .await
        .map_err(Rejection::Upstream)?;
    recorder.upstream_responded(&response);

    Ok(into_client_response(
        response,
        recorder,
        &exchange,
        controls,
        &shutdown,
        &outcome.response,
    )
    .await)
}

/// Forwards a request a rule sent somewhere else.
///
/// The same shape as the unmapped path — capture the request body, open the
/// upstream connection, hand the response back streaming — with the
/// destination and the request line coming from the rule instead of from the
/// request target. The captured exchange keeps the URL the client asked for
/// and records the destination it actually went to, so a traffic list shows
/// both halves of what happened.
#[allow(clippy::too_many_arguments)]
async fn forward_mapped<B>(
    client: SocketAddr,
    request: Request<B>,
    recorder: &Recorder,
    exchange: &Exchange,
    upstream: &UpstreamManager,
    target: &rules::RemoteTarget,
    controls: &Controls,
    shutdown: Shutdown,
    modify: &[Transformation<ModifyResponse>],
) -> Result<Response<ProxyBody>, Rejection>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    recorder.destination(ruled::mapped_destination(target));

    // A mapped WebSocket is still a WebSocket. The rule decided where it
    // goes, which is the whole of what Map Remote does here; how it is
    // carried once the destination is settled is the same as for any other
    // upgrade, on a connection of its own.
    if websocket::is_upgrade(&request) {
        let destination = ruled::mapped_upstream(target).map_err(Rejection::Upstream)?;
        let url = redact::loggable(request.uri());

        return websocket::forward(
            client,
            &url,
            &destination,
            upstream,
            recorder,
            exchange,
            ruled::into_mapped_request(request, target),
            controls,
            shutdown,
        )
        .await
        .map_err(Rejection::Upstream);
    }

    let mapped = ruled::into_mapped_request(request, target)
        .map(|body| upstream::boxed(controls.throttle().upload(recorder.capture_request(body))));

    recorder.upstream_connect_started();
    let response = ruled::send_mapped(target, upstream, mapped)
        .await
        .map_err(Rejection::Upstream)?;
    recorder.upstream_responded(&response);

    Ok(into_client_response(response, recorder, exchange, controls, &shutdown, modify).await)
}

/// Rewrites a proxy request into the request the origin expects.
///
/// `GET http://example.com/api/users?active=true HTTP/1.1` becomes
/// `GET /api/users?active=true HTTP/1.1`, with the proxy's own headers dropped
/// and `Host` set from the request target.
fn into_origin_request<B>(request: Request<B>, target: &Target) -> Request<B> {
    let (mut parts, body) = request.into_parts();

    parts.uri = target.origin_form().clone();
    parts.version = Version::HTTP_11;
    headers::strip_hop_by_hop(&mut parts.headers);
    headers::set_host(&mut parts.headers, target.host_header());

    Request::from_parts(parts, body)
}

/// Prepares the origin's response for the client.
///
/// The body is passed through untouched and still streaming; only the headers
/// that described the origin connection are removed, because the proxy frames
/// the body again on its own connection to the client.
///
/// This is also the one place a response breakpoint stops a response, a
/// Modify Response rule changes one, and a download ceiling paces one, which
/// is why every route that answers a client from an origin comes through
/// here. `modify` is the Modify Response rules that matched the request, in
/// the order they apply. A response the developer cancelled
/// becomes an error response instead, and none of what the origin said is
/// delivered.
///
/// Public to the crate rather than to `http1` alone because a replayed
/// request ends the same way — see [`crate::replay`]. There is no client on
/// that route, and the response is read to its end and dropped; what matters
/// is that the capture is wrapped and finished in exactly one place.
pub async fn into_client_response<B>(
    response: Response<B>,
    recorder: &Recorder,
    exchange: &Exchange,
    controls: &Controls,
    shutdown: &Shutdown,
    modify: &[Transformation<ModifyResponse>],
) -> Response<ProxyBody>
where
    B: Body<Data = Bytes, Error = hyper::Error> + Send + Sync + Unpin + 'static,
{
    // Before the breakpoint, so what a developer holds still is what the
    // client is about to be given. See [`crate::modify`].
    let (response, modified) = modify::response(response, modify, &exchange.method).await;

    let response = match pause::response(controls.breakpoints(), exchange, response, shutdown).await
    {
        ResponseOutcome::Deliver { response, edited } => {
            if edited || modified {
                // The exchange is remembered by what the client was
                // given, not by what the origin said.
                recorder.response_edited(&response);
            }

            response
        }
        ResponseOutcome::Cancelled => {
            recorder.fail(TrafficError::new(
                TrafficErrorKind::ResponseForwarding,
                CANCELLED_AT_BREAKPOINT,
            ));

            return reply::error(StatusCode::BAD_GATEWAY, CANCELLED_AT_BREAKPOINT);
        }
    };

    let (mut parts, body) = response.into_parts();

    headers::strip_hop_by_hop(&mut parts.headers);
    headers::set_close(&mut parts.headers);
    // The client connection is HTTP/1.1; hyper downgrades this again if the
    // client turns out to speak HTTP/1.0.
    parts.version = Version::HTTP_11;

    // Wrapping the body is also what ends the exchange: when the last of it
    // has been written to the client, the recorder publishes. The pacing goes
    // outside the capture, so what is recorded is every byte that was
    // forwarded however slowly it was handed over.
    let captured = recorder.capture_response(body);
    let body = if sse::is_event_stream(&parts.headers) {
        // Stored as soon as the head is known, so the inspector can open a
        // stream that has not ended and watch events arrive.
        recorder.announce();
        controls
            .throttle()
            .download(sse::SseTap::new(
                captured,
                recorder.stream_tap(),
                recorder.capture_limit(),
            ))
            .boxed()
    } else {
        controls.throttle().download(captured).boxed()
    };

    Response::from_parts(parts, body)
}

/// Builds the response for a request that could not be forwarded.
///
/// The body repeats the status and the client-safe reason. Internal error
/// details stay in the log.
fn error_response(rejection: &Rejection) -> Response<ProxyBody> {
    reply::error(rejection.status(), rejection.reason())
}

/// Why a request was answered by the proxy instead of by an origin server.
#[derive(Debug)]
enum Rejection {
    /// A protocol version this proxy cannot carry.
    UnsupportedVersion(Version),
    /// The request target cannot be forwarded.
    Target(TargetError),
    /// The origin server could not be reached, or gave no usable answer.
    Upstream(UpstreamError),
    /// The `CONNECT` request could not be turned into an intercepted
    /// connection.
    Connect(ConnectError),
}

impl Rejection {
    /// Status to answer the client with.
    fn status(&self) -> StatusCode {
        match self {
            Self::UnsupportedVersion(_) => StatusCode::HTTP_VERSION_NOT_SUPPORTED,
            Self::Target(error) => error.status(),
            Self::Upstream(error) => error.status(),
            Self::Connect(error) => error.status(),
        }
    }

    /// Explanation safe to send to the client.
    fn reason(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion(_) => "this proxy speaks HTTP/1.1 on a plaintext connection",
            Self::Target(error) => error.reason(),
            Self::Upstream(error) => error.reason(),
            Self::Connect(error) => error.reason(),
        }
    }

    /// The failure as a captured exchange records it.
    ///
    /// The summary is the same phrase the client is given, which is written
    /// for the purpose and names no internal detail. What the log says may be
    /// more specific; what is captured is not.
    fn traffic_error(&self) -> TrafficError {
        TrafficError::new(self.traffic_kind(), self.reason())
    }

    /// What kind of failure this is, in the captured model's terms.
    fn traffic_kind(&self) -> TrafficErrorKind {
        match self {
            Self::UnsupportedVersion(_) | Self::Target(_) => TrafficErrorKind::RequestRejected,
            Self::Upstream(error) => error.traffic_kind(),
            Self::Connect(error) => error.traffic_kind(),
        }
    }
}

/// The log form, which may name causes the client is not told about.
impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(f, "unsupported version {version:?}"),
            Self::Target(error) => error.fmt(f),
            Self::Upstream(error) => error.fmt(f),
            Self::Connect(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::header::{HeaderMap, HeaderValue, CONTENT_TYPE, HOST};
    use hyper::Uri;
    use proxy_storage::{CaptureConfig, EventPublisher, EventSubscriber, TrafficEvent};
    use proxy_tls::{CertificateAuthority, UpstreamTrust};

    fn request(uri: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .uri(uri)
            .body(Empty::new())
            .expect("valid request")
    }

    fn target(uri: &str) -> Target {
        let uri: Uri = uri.parse().expect("valid uri");
        Target::parse(&uri).expect("forwardable target")
    }

    fn rewritten(request: Request<Empty<Bytes>>) -> Request<Empty<Bytes>> {
        let target = target(&request.uri().to_string());
        into_origin_request(request, &target)
    }

    async fn body_text(response: Response<ProxyBody>) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("generated body is complete")
            .to_bytes();

        String::from_utf8(bytes.to_vec()).expect("generated body is text")
    }

    fn capture() -> Capture {
        Capture::new(CaptureConfig::default(), EventPublisher::new())
    }

    async fn rejection_of(request: Request<Empty<Bytes>>) -> Rejection {
        let capture = capture();
        let recorder = capture.forwarded(client(), &request);

        match forward(
            client(),
            request.map(StagedBody::unread),
            &recorder,
            &Exchange::forwarded("example.com", "/", "GET", "http://example.com/"),
            &interceptor(),
            &upstream(),
            None,
            &Controls::new(),
            Shutdown::never(),
        )
        .await
        {
            Ok(_) => panic!("request should not have been forwarded"),
            Err(rejection) => rejection,
        }
    }

    fn published(subscriber: &mut EventSubscriber) -> TrafficEvent {
        subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy")
    }

    fn connect_request(target: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .method(Method::CONNECT)
            .uri(target)
            .body(Empty::new())
            .expect("valid request")
    }

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

    /// The connections a test proxy would hold open, with a CA of its own.
    fn upstream() -> Arc<UpstreamManager> {
        Arc::new(UpstreamManager::new(interceptor()))
    }

    async fn connect_rejection_of(request: Request<Empty<Bytes>>) -> Rejection {
        match intercept_connect(
            client(),
            request,
            interceptor(),
            upstream(),
            &capture(),
            None,
            Controls::new(),
            Shutdown::never(),
        )
        .await
        {
            Ok(_) => panic!("nothing should have been established"),
            Err(rejection) => rejection,
        }
    }

    /// An address nothing is listening on: a port is bound to learn a free
    /// one and then released, so connecting to it is refused rather than
    /// ignored.
    async fn unused_addr() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);

        addr.to_string()
    }

    #[test]
    fn the_absolute_url_becomes_an_origin_form_request_line() {
        let request = rewritten(request("http://example.com/api/users?active=true"));

        assert_eq!(request.uri(), "/api/users?active=true");
        assert_eq!(request.version(), Version::HTTP_11);
    }

    #[test]
    fn the_host_header_is_taken_from_the_request_target() {
        let mut request = request("http://example.com:8080/x");
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_static("stale.invalid"));

        let request = rewritten(request);

        assert_eq!(request.headers()[HOST], "example.com:8080");
    }

    #[test]
    fn proxy_only_headers_do_not_reach_the_origin() {
        let mut request = request("http://example.com/x");
        let headers: &mut HeaderMap = request.headers_mut();
        headers.insert("proxy-connection", HeaderValue::from_static("keep-alive"));
        headers.insert("proxy-authorization", HeaderValue::from_static("Basic abc"));
        headers.insert("accept", HeaderValue::from_static("application/json"));

        let request = rewritten(request);

        assert!(request.headers().get("proxy-connection").is_none());
        assert!(request.headers().get("proxy-authorization").is_none());
        assert_eq!(request.headers()["accept"], "application/json");
    }

    #[tokio::test]
    async fn a_reachable_connect_target_is_answered_with_connection_established() {
        let destination = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = destination.local_addr().expect("local address");

        let response = intercept_connect(
            client(),
            connect_request(&addr.to_string()),
            interceptor(),
            upstream(),
            &capture(),
            None,
            Controls::new(),
            Shutdown::never(),
        )
        .await
        .expect("the destination is listening");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .extensions()
                .get::<hyper::ext::ReasonPhrase>()
                .map(|reason| reason.as_bytes()),
            Some(&b"Connection Established"[..])
        );
    }

    #[tokio::test]
    async fn a_malformed_connect_target_is_a_bad_request() {
        for target in ["example.com", "example.com:0x1bb", ":443", "/index.html"] {
            let rejection = connect_rejection_of(connect_request(target)).await;

            assert!(
                matches!(rejection, Rejection::Connect(_)),
                "{target} should have been refused: {rejection}"
            );
            assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn a_connect_target_that_cannot_be_reached_is_a_bad_gateway() {
        let rejection = connect_rejection_of(connect_request(&unused_addr().await)).await;
        let response = error_response(&rejection);

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_text(response).await,
            "502 Bad Gateway: could not connect to the destination server\n"
        );
        assert!(rejection.to_string().contains("connect failed"));
    }

    #[tokio::test]
    async fn a_connect_request_on_a_version_that_cannot_be_carried_is_refused() {
        let mut request = connect_request("example.com:443");
        *request.version_mut() = Version::HTTP_2;

        let rejection = connect_rejection_of(request).await;

        assert!(matches!(rejection, Rejection::UnsupportedVersion(_)));
        assert_eq!(rejection.status(), StatusCode::HTTP_VERSION_NOT_SUPPORTED);
    }

    #[tokio::test]
    async fn an_origin_form_target_is_a_bad_request() {
        let rejection = rejection_of(request("/index.html")).await;

        assert!(matches!(
            rejection,
            Rejection::Target(TargetError::NotAbsoluteForm)
        ));
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_https_target_is_not_implemented() {
        let rejection = rejection_of(request("https://example.com/x")).await;

        assert_eq!(rejection.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn a_version_that_cannot_be_forwarded_is_refused() {
        let mut request = request("http://example.com/x");
        *request.version_mut() = Version::HTTP_2;

        let rejection = rejection_of(request).await;

        assert!(matches!(rejection, Rejection::UnsupportedVersion(_)));
        assert_eq!(rejection.status(), StatusCode::HTTP_VERSION_NOT_SUPPORTED);
    }

    #[tokio::test]
    async fn an_error_response_states_the_status_and_the_reason() {
        let rejection = rejection_of(request("/index.html")).await;
        let response = error_response(&rejection);

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            body_text(response).await,
            "400 Bad Request: proxy requests must use an absolute URL\n"
        );
    }

    #[tokio::test]
    async fn an_error_response_closes_the_client_connection() {
        let rejection = rejection_of(request("/index.html")).await;
        let response = error_response(&rejection);

        assert_eq!(response.headers()[hyper::header::CONNECTION], "close");
    }

    #[tokio::test]
    async fn an_unreachable_origin_is_a_bad_gateway_without_internal_detail() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);

        let rejection = rejection_of(request(&format!("http://{addr}/x"))).await;
        let response = error_response(&rejection);

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body_text(response).await,
            "502 Bad Gateway: could not connect to the destination server\n"
        );
        assert!(rejection.to_string().contains("connect failed"));
    }

    /// A request carrying everything that must not reach a log: credentials
    /// in the URL, credentials in the headers, and a body.
    fn secretive_request() -> Request<Empty<Bytes>> {
        Request::builder()
            .method("POST")
            .uri("http://alice:hunter2@example.com/orders?token=abc")
            .header("authorization", "Bearer sk-secret")
            .header("cookie", "session=hunter2")
            .header("proxy-authorization", "Basic YWxpY2U6aHVudGVyMg==")
            .body(Empty::new())
            .expect("valid request")
    }

    /// Everything the log lines built below must not contain.
    const SECRETS: &[&str] = &[
        "hunter2",
        "sk-secret",
        "alice",
        "YWxpY2U6aHVudGVyMg==",
        "session=",
    ];

    #[test]
    fn a_carried_request_is_logged_with_its_credentials_redacted() {
        let request = secretive_request();
        let target = redact::loggable(request.uri());

        let line = forwarded_report(client(), request.method(), &target, StatusCode::OK);

        assert_eq!(
            line,
            "127.0.0.1:54321 POST http://[redacted]@example.com/orders?token=abc -> 200 OK"
        );
        for secret in SECRETS {
            assert!(!line.contains(secret), "{line} carries {secret}");
        }
    }

    #[tokio::test]
    async fn a_refused_request_is_logged_with_its_credentials_redacted() {
        let secretive = secretive_request();
        let target = redact::loggable(secretive.uri());
        let rejection = rejection_of(request("/index.html")).await;

        let line = rejected_report(
            client(),
            &Method::POST,
            &target,
            &rejection,
            StatusCode::BAD_REQUEST,
        );

        assert!(
            line.contains("http://[redacted]@example.com/orders"),
            "{line}"
        );
        for secret in SECRETS {
            assert!(!line.contains(secret), "{line} carries {secret}");
        }
    }

    #[test]
    fn a_log_line_names_the_request_and_nothing_the_request_carried() {
        let request = secretive_request();
        let target = redact::loggable(request.uri());

        let line = forwarded_report(client(), request.method(), &target, StatusCode::OK);

        // Nothing from the header section, whatever the header was called.
        for (name, _) in request.headers() {
            assert!(
                !line.to_ascii_lowercase().contains(name.as_str()),
                "{line} names the {name} header"
            );
        }
        // And nothing from a body: there is nowhere in the line for one.
        assert!(!line.contains("password"), "{line}");
        assert_eq!(line.lines().count(), 1);
    }

    #[tokio::test]
    async fn a_refused_connect_is_logged_with_its_credentials_redacted() {
        let uri: Uri = "alice:hunter2@example.com:443".parse().expect("valid uri");
        let rejection = connect_rejection_of(connect_request("example.com")).await;

        let line = rejected_report(
            client(),
            &Method::CONNECT,
            &redact::loggable(&uri),
            &rejection,
            StatusCode::BAD_REQUEST,
        );

        assert!(line.contains("[redacted]@example.com:443"), "{line}");
        assert!(!line.contains("hunter2"), "{line}");
    }

    #[tokio::test]
    async fn a_refused_request_is_published_as_a_rejected_exchange() {
        let capture = capture();
        let mut subscriber = capture.subscribe();
        let request = request("/index.html");
        let recorder = capture.forwarded(client(), &request);

        let rejection = forward(
            client(),
            request.map(StagedBody::unread),
            &recorder,
            &Exchange::forwarded("example.com", "/", "GET", "http://example.com/"),
            &interceptor(),
            &upstream(),
            None,
            &Controls::new(),
            Shutdown::never(),
        )
        .await
        .expect_err("an origin-form target cannot be forwarded");
        recorder.fail(rejection.traffic_error());

        let event = published(&mut subscriber);
        assert_eq!(event.request.method, "GET");
        assert_eq!(event.request.url, "/index.html");
        assert_eq!(event.response, None);
        assert_eq!(
            event.error.as_ref().map(proxy_storage::TrafficError::kind),
            Some(proxy_storage::TrafficErrorKind::RequestRejected)
        );
        assert_eq!(
            event.error.as_ref().map(|error| error.summary().to_owned()),
            Some("proxy requests must use an absolute URL".to_owned())
        );
    }

    #[tokio::test]
    async fn an_unreachable_origin_is_published_as_a_connect_failure() {
        let capture = capture();
        let mut subscriber = capture.subscribe();
        let request = request(&format!("http://{}/x", unused_addr().await));
        let recorder = capture.forwarded(client(), &request);

        let rejection = forward(
            client(),
            request.map(StagedBody::unread),
            &recorder,
            &Exchange::forwarded("example.com", "/", "GET", "http://example.com/"),
            &interceptor(),
            &upstream(),
            None,
            &Controls::new(),
            Shutdown::never(),
        )
        .await
        .expect_err("nothing is listening");
        recorder.fail(rejection.traffic_error());

        let event = published(&mut subscriber);
        assert_eq!(
            event.error.as_ref().map(proxy_storage::TrafficError::kind),
            Some(proxy_storage::TrafficErrorKind::UpstreamConnect)
        );
        // The timing still says how long the attempt took.
        assert!(event.timing.upstream_connect_micros.is_some());
        assert!(event.timing.completed_micros.is_some());
    }
}
