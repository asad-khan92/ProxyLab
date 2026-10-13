//! One intercepted exchange, whichever protocol carried it.
//!
//! This is the per-request half of interception, and it is where HTTP/1.1 and
//! HTTP/2 stop being different things. A request arrives as a `Request` —
//! read off an HTTP/1.1 connection or off one HTTP/2 stream, it makes no
//! difference by the time it is here — and from that point on it takes the
//! path every other request through ProxyLab takes:
//!
//! ```text
//! request ──▶ capture begins ──▶ rules ──▶ upstream ──▶ capture ends
//! ```
//!
//! On an HTTP/2 connection this function runs once per stream, concurrently,
//! and nothing in it is shared between those runs. Each has its own recorder,
//! so each is its own [`TrafficExchange`](proxy_storage::TrafficExchange)
//! with its own id, its own timing, and its own bodies; each consults the
//! rules on its own, so one stream can be blocked while its neighbours are
//! carried; and each answers on its own, so an origin that fails for one
//! stream has not failed for the rest.
//!
//! Nothing here fails. A request that cannot be carried becomes an HTTP error
//! response inside the client's session — on that stream only — so a client
//! that got as far as a handshake always gets an answer.

use std::net::SocketAddr;

use hyper::body::Incoming;
use hyper::header::{HeaderValue, HOST};
use hyper::{Method, Request, Response, StatusCode, Uri, Version};
use proxy_storage::{Destination, TrafficError, TrafficErrorKind};

use rules::{ModifyResponse, RuleDecision, RuleEngine, Transformation};

use crate::capture::{Capture, Recorder};
use crate::controls::pause::{self, RequestOutcome};
use crate::controls::{Controls, Exchange, StagedBody};
use crate::http1::connect_target::ConnectTarget;
use crate::http1::reply::{self, ProxyBody};
use crate::http1::service::CANCELLED_AT_BREAKPOINT;
use crate::http1::{headers, ruled, service};
use crate::modify;
use crate::shutdown::Shutdown;
use crate::upstream::{self, UpstreamDestination, UpstreamManager};
use crate::websocket;

/// The port an `https` URL does not need to name.
const DEFAULT_HTTPS_PORT: u16 = 443;

/// The client's request body, once a breakpoint has had the chance to look at
/// it.
///
/// Every intercepted request is wrapped, whether or not a breakpoint matched:
/// one that did not is the client's own body with nothing read from it. See
/// [`StagedBody`].
type ClientBody = StagedBody<Incoming>;

/// Forwards one intercepted request and prepares the response for the client.
///
/// Never fails: see the module documentation.
#[allow(clippy::too_many_arguments)]
pub async fn forward(
    client: SocketAddr,
    target: &ConnectTarget,
    authority: &HeaderValue,
    destination: &UpstreamDestination,
    upstream: &UpstreamManager,
    capture: &Capture,
    rules: Option<&RuleEngine>,
    controls: &Controls,
    request: Request<Incoming>,
    shutdown: Shutdown,
) -> Response<ProxyBody> {
    let method = request.method().clone();
    // Built from the `CONNECT` authority and the request path, so it names
    // what was actually asked for without repeating anything else the client
    // put in its headers or its `:authority`.
    let url = url(target, request.uri());

    if !carriable(request.version()) {
        eprintln!("{client} HTTPS request {method} {url} -> 505 (unsupported version)");
        refused(capture, client, target, &url, &request, UNSUPPORTED_VERSION);

        return reply::error(StatusCode::HTTP_VERSION_NOT_SUPPORTED, UNSUPPORTED_VERSION);
    }

    if method == Method::CONNECT {
        // A `CONNECT` inside an intercepted session asks the origin server to
        // become a proxy, which an origin server is not.
        eprintln!("{client} HTTPS request {method} {url} -> 400 (nested CONNECT)");
        refused(capture, client, target, &url, &request, NESTED_CONNECT);

        return reply::error(StatusCode::BAD_REQUEST, NESTED_CONNECT);
    }

    // Before the recording starts, so the exchange this is remembered by is
    // the request that was actually sent. See [`crate::controls`].
    let paused = Exchange::intercepted(
        target.host(),
        request.uri().path(),
        method.as_str(),
        url.clone(),
    );
    let (request, cancelled) =
        match pause::request(controls.breakpoints(), &paused, request, &shutdown).await {
            RequestOutcome::Carry(request) => (request, false),
            RequestOutcome::Cancelled(request) => (request, true),
        };

    // Worked out again, because an edit may have changed the path and the
    // method. The host cannot change — this is one TLS session, opened to one
    // host — which is why only these two are read back.
    let method = request.method().clone();
    let url = self::url(target, request.uri());
    let exchange = Exchange::intercepted(
        target.host(),
        request.uri().path(),
        method.as_str(),
        url.clone(),
    );

    // The captured URL is the same one, so an intercepted HTTPS request and a
    // forwarded plaintext one are named the same way — and an HTTP/2 stream
    // and an HTTP/1.1 request are too. The version the capture records is the
    // request's own, so a stored exchange says which protocol carried it
    // without anything being added to the model to say so.
    let recorder = capture.intercepted(
        client,
        Destination::new(target.host(), target.port()),
        url.clone(),
        &request,
    );

    if cancelled {
        eprintln!("{client} HTTPS request {method} {url} -> 502 ({CANCELLED_AT_BREAKPOINT})");
        recorder.fail(TrafficError::new(
            TrafficErrorKind::RequestRejected,
            CANCELLED_AT_BREAKPOINT,
        ));

        return reply::error(StatusCode::BAD_GATEWAY, CANCELLED_AT_BREAKPOINT);
    }

    // Before the rules and before anything is sent, for the reason the
    // plaintext route waits here too: the delay is the link, and every route
    // out of here is on the far side of it.
    controls.throttle().delay().await;

    // Only now, with the request decrypted and the nested-CONNECT case
    // refused, are the rules consulted. The `CONNECT` that opened this
    // session was not evaluated: it carries no path, and blocking it would
    // have blocked every request in the session rather than the ones a rule
    // named. On an HTTP/2 connection that would have been every stream.
    let outcome = ruled::decide(
        rules,
        target.host(),
        request.uri().path(),
        request.method().as_str(),
    );

    match &outcome.decision {
        RuleDecision::Allow { .. } | RuleDecision::MapRemote { .. } => {}
        RuleDecision::Block { rule, block } => {
            println!("{client} HTTPS request {method} {url} -> blocked by rule {rule}");

            return ruled::blocked(block, &recorder);
        }
        RuleDecision::MapLocal { rule, map } => {
            println!("{client} HTTPS request {method} {url} -> answered locally by rule {rule}");

            return ruled::mapped_local(map, &recorder).await;
        }
        RuleDecision::Redirect { rule, redirect } => {
            println!(
                "{client} HTTPS request {method} {url} -> redirected to {} by rule {rule}",
                redirect.location()
            );

            return ruled::redirected(redirect, &recorder);
        }
    }

    // Going to an origin, so the Modify Request rules above the decision
    // change it here — the same place, and the same code, as on the plaintext
    // route. The host cannot change: this is one session, to one host. A
    // WebSocket handshake is carried as it came. See [`crate::modify`].
    let (request, modified) = if websocket::is_upgrade(&request) {
        (request, false)
    } else {
        modify::request(request, &outcome.request).await
    };
    let (method, url, exchange) = if modified {
        let url = self::url(target, request.uri());
        recorder.request_edited(&request, url.clone());

        let exchange = Exchange::intercepted(
            target.host(),
            request.uri().path(),
            request.method().as_str(),
            url.clone(),
        );

        (request.method().clone(), url, exchange)
    } else {
        (method, url, exchange)
    };

    if let RuleDecision::MapRemote { rule, map } = &outcome.decision {
        println!(
            "{client} HTTPS request {method} {url} -> mapped to {} by rule {rule}",
            map.target
        );

        return forward_mapped(
            client,
            &url,
            &map.target,
            upstream,
            &recorder,
            &exchange,
            controls,
            request,
            shutdown,
            &outcome.response,
        )
        .await;
    }

    // A `wss://` upgrade is carried from here on by a path of its own, on a
    // connection nothing else will use. It is the same interception either
    // way: the client checked ProxyLab's certificate, and ProxyLab verifies
    // the origin's. See [`crate::websocket`].
    if websocket::is_upgrade(&request) {
        println!("{client} WebSocket upgrade {url}");

        return match websocket::forward(
            client,
            &url,
            destination,
            upstream,
            &recorder,
            &exchange,
            into_upstream_request(request, authority),
            controls,
            shutdown,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                proxy_diagnostics::error(
                    "upstream",
                    "websocket",
                    "connection",
                    &format!("{client} WebSocket {url} -> 502 ({error})"),
                );
                recorder.fail(TrafficError::new(error.traffic_kind(), error.reason()));

                reply::error(error.status(), error.reason())
            }
        };
    }

    println!("{client} HTTPS request {method} {url}");

    let upstream_request = into_upstream_request(request, authority)
        .map(|body| upstream::boxed(controls.throttle().upload(recorder.capture_request(body))));

    match upstream.send(destination, upstream_request).await {
        Ok(response) => {
            println!("{client} HTTPS response {}", response.status());
            recorder.upstream_responded(&response);

            service::into_client_response(
                response,
                &recorder,
                &exchange,
                controls,
                &shutdown,
                &outcome.response,
            )
            .await
        }
        Err(error) => {
            // The cause names the origin's TLS session or its HTTP framing,
            // which is the developer's business and not the client's.
            proxy_diagnostics::error(
                "upstream",
                "send",
                "connection",
                &format!("{client} HTTPS request {method} {url} -> 502 ({error})"),
            );
            recorder.fail(TrafficError::new(error.traffic_kind(), error.reason()));

            reply::error(error.status(), error.reason())
        }
    }
}

/// Forwards an intercepted request a rule sent somewhere else.
///
/// The connections open to the original origin are not used: a mapped request
/// goes to a different server, and may go over plaintext where this one is
/// TLS. It gets a connection to the mapped destination instead — pooled under
/// that destination's own key, of whichever kind its scheme names and
/// whichever protocol it agrees to.
#[allow(clippy::too_many_arguments)]
async fn forward_mapped(
    client: SocketAddr,
    url: &str,
    target: &rules::RemoteTarget,
    upstream: &UpstreamManager,
    recorder: &Recorder,
    exchange: &Exchange,
    controls: &Controls,
    request: Request<ClientBody>,
    shutdown: Shutdown,
    modify: &[Transformation<ModifyResponse>],
) -> Response<ProxyBody> {
    recorder.destination(ruled::mapped_destination(target));

    // A mapped `wss://` upgrade is carried to the mapped destination as an
    // upgrade, on a connection of its own: the rule chose where it goes and
    // nothing else about it.
    if websocket::is_upgrade(&request) {
        let destination = match ruled::mapped_upstream(target) {
            Ok(destination) => destination,
            Err(error) => {
                recorder.fail(TrafficError::new(error.traffic_kind(), error.reason()));

                return reply::error(error.status(), error.reason());
            }
        };

        return match websocket::forward(
            client,
            url,
            &destination,
            upstream,
            recorder,
            exchange,
            ruled::into_mapped_request(request, target),
            controls,
            shutdown,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                proxy_diagnostics::error(
                    "upstream",
                    "websocket",
                    "connection",
                    &format!("{client} WebSocket {url} -> 502 ({error})"),
                );
                recorder.fail(TrafficError::new(error.traffic_kind(), error.reason()));

                reply::error(error.status(), error.reason())
            }
        };
    }

    let mapped = ruled::into_mapped_request(request, target)
        .map(|body| upstream::boxed(controls.throttle().upload(recorder.capture_request(body))));

    recorder.upstream_connect_started();
    match ruled::send_mapped(target, upstream, mapped).await {
        Ok(response) => {
            recorder.upstream_responded(&response);

            service::into_client_response(response, recorder, exchange, controls, &shutdown, modify)
                .await
        }
        Err(error) => {
            proxy_diagnostics::error(
                "upstream",
                "send",
                "connection",
                &format!("{client} HTTPS mapped request -> 502 ({error})"),
            );
            recorder.fail(TrafficError::new(error.traffic_kind(), error.reason()));

            reply::error(error.status(), error.reason())
        }
    }
}

/// Records a request the session refused before it was carried anywhere.
///
/// A request that never reaches the rules still happened, so it is still an
/// exchange in the traffic list — one with no response and an error saying
/// why. The recording is made here rather than up front because the request
/// that gets that far may have been edited at a breakpoint first, and what is
/// recorded is what was actually going to be sent.
fn refused<B>(
    capture: &Capture,
    client: SocketAddr,
    target: &ConnectTarget,
    url: &str,
    request: &Request<B>,
    reason: &'static str,
) {
    let recorder = capture.intercepted(
        client,
        Destination::new(target.host(), target.port()),
        url.to_owned(),
        request,
    );

    recorder.fail(TrafficError::new(TrafficErrorKind::RequestRejected, reason));
}

/// What a client is told when it spoke a version this proxy cannot carry.
const UNSUPPORTED_VERSION: &str = "this proxy speaks HTTP/1.1 and HTTP/2";

/// What a client is told when it sent a `CONNECT` inside a session.
const NESTED_CONNECT: &str = "CONNECT cannot be sent through an intercepted connection";

/// Whether a request's version is one the proxy can carry.
///
/// HTTP/1.0 and HTTP/1.1 arrive on an HTTP/1.1 session; HTTP/2 arrives on a
/// stream of an HTTP/2 one. Anything else is a version no handshake here
/// agreed to, so it is refused rather than guessed at.
fn carriable(version: Version) -> bool {
    matches!(
        version,
        Version::HTTP_10 | Version::HTTP_11 | Version::HTTP_2
    )
}

/// Rewrites an intercepted request into the proxy's canonical outgoing one.
///
/// Origin form, HTTP/1.1 shape, hop-by-hop headers gone, `Host` naming the
/// server. What happens to it after that depends on which protocol the origin
/// agreed to, and is [`Origin`]'s business rather than this function's — so
/// this is the same rewriting whether the request came off an HTTP/1.1
/// connection or an HTTP/2 stream, and whether it is going out on one or the
/// other.
///
/// Where `Host` comes from is the one place the two client protocols differ,
/// and only because they spell the same fact differently:
///
/// ```text
/// HTTP/1.1 client:  Host: example.com       ──▶ kept
/// HTTP/2   client:  :authority: example.com ──▶ becomes Host
/// neither:                                  ──▶ the CONNECT authority
/// ```
///
/// An HTTP/2 request has no `Host` — the authority is a pseudo-header, and
/// pseudo-headers are not headers — so it is read off the request target,
/// which is where the HTTP library put it. A client that sent neither is
/// given the authority its `CONNECT` named, which is the host this session
/// was opened to.
fn into_upstream_request<B>(request: Request<B>, authority: &HeaderValue) -> Request<B> {
    let (mut parts, body) = request.into_parts();

    // Before the target is reduced to origin form, because reducing it is
    // what throws the authority away.
    if !parts.headers.contains_key(HOST) {
        let named = parts
            .uri
            .authority()
            .and_then(|authority| HeaderValue::from_str(authority.as_str()).ok());

        headers::set_host(&mut parts.headers, named.as_ref().unwrap_or(authority));
    }

    // A client that sent an absolute-form target inside the session gets it
    // reduced to origin form; anything else is already origin form.
    if let Some(path_and_query) = parts.uri.path_and_query().cloned() {
        parts.uri = Uri::from(path_and_query);
    }
    parts.version = Version::HTTP_11;
    headers::strip_hop_by_hop(&mut parts.headers);

    Request::from_parts(parts, body)
}

/// The URL an intercepted request is for, in the form it is safe to log.
///
/// Assembled from the `CONNECT` target and the request path rather than taken
/// from anything the client wrote in a header or a pseudo-header, so it names
/// one host — the one the connection was opened to. The port is left out when
/// it is the one `https` implies, so the usual case reads as a plain URL.
fn url(target: &ConnectTarget, uri: &Uri) -> String {
    let host = if target.host().contains(':') {
        format!("[{}]", target.host())
    } else {
        target.host().to_owned()
    };
    let authority = if target.port() == DEFAULT_HTTPS_PORT {
        host
    } else {
        format!("{host}:{}", target.port())
    };
    let path = uri
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/");

    format!("https://{authority}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::header::HeaderMap;

    fn target(authority: &str) -> ConnectTarget {
        let uri: Uri = authority.parse().expect("valid uri");
        ConnectTarget::parse(&uri).expect("tunnellable target")
    }

    fn uri(target: &str) -> Uri {
        target.parse().expect("valid uri")
    }

    fn request(target: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .uri(target)
            .body(Empty::new())
            .expect("valid request")
    }

    /// The request an HTTP/2 client sends: an absolute target assembled from
    /// its pseudo-headers, no `Host`, and a version that says HTTP/2. This is
    /// what hyper hands over for one stream.
    fn http2_request(target: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .uri(target)
            .version(Version::HTTP_2)
            .body(Empty::new())
            .expect("valid request")
    }

    /// The request as the proxy would carry it onward, for a session opened
    /// by a `CONNECT` to `connect_target`.
    fn rewritten(request: Request<Empty<Bytes>>, connect_target: &str) -> Request<Empty<Bytes>> {
        let host = super::super::authority(&target(connect_target)).expect("a usable authority");

        into_upstream_request(request, &host)
    }

    #[test]
    fn a_logged_url_names_the_connect_host_and_the_request_path() {
        assert_eq!(
            url(&target("example.com:443"), &uri("/api/users?active=true")),
            "https://example.com/api/users?active=true"
        );
    }

    #[test]
    fn a_logged_url_keeps_a_port_that_is_not_the_default() {
        assert_eq!(
            url(&target("127.0.0.1:8443"), &uri("/")),
            "https://127.0.0.1:8443/"
        );
    }

    #[test]
    fn a_logged_url_brackets_an_ipv6_host() {
        assert_eq!(
            url(&target("[::1]:8443"), &uri("/x")),
            "https://[::1]:8443/x"
        );
    }

    #[test]
    fn a_logged_url_asks_for_the_root_when_the_request_names_no_path() {
        assert_eq!(
            url(&target("example.com:443"), &Uri::default()),
            "https://example.com/"
        );
    }

    #[test]
    fn a_logged_url_is_the_same_for_an_http_2_request() {
        // An HTTP/2 request carries its authority on the target, which the
        // logged URL must not take: the host is the one the connection was
        // opened to, whatever a stream claims.
        assert_eq!(
            url(
                &target("example.com:443"),
                &uri("https://elsewhere.invalid/api")
            ),
            "https://example.com/api"
        );
    }

    #[test]
    fn a_logged_url_does_not_repeat_credentials_from_a_request_target() {
        // An absolute-form target inside an intercepted session is unusual,
        // and could carry userinfo. The logged URL is built from the CONNECT
        // authority and the path, so there is nowhere for it to appear.
        let logged = url(
            &target("example.com:443"),
            &uri("https://alice:hunter2@example.com/x"),
        );

        assert!(!logged.contains("hunter2"), "logged a password: {logged}");
        assert!(!logged.contains("alice"), "logged a username: {logged}");
        assert_eq!(logged, "https://example.com/x");
    }

    #[test]
    fn an_absolute_form_target_reaches_the_origin_in_origin_form() {
        let request = rewritten(request("https://example.com/api?x=1"), "example.com:443");

        assert_eq!(request.uri(), "/api?x=1");
        assert_eq!(request.version(), Version::HTTP_11);
    }

    #[test]
    fn an_origin_form_target_is_passed_through_unchanged() {
        let request = rewritten(request("/api?x=1"), "example.com:443");

        assert_eq!(request.uri(), "/api?x=1");
    }

    #[test]
    fn the_clients_own_host_header_is_what_the_origin_is_told() {
        let mut request = request("/x");
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_static("www.example.com"));

        let request = rewritten(request, "example.com:443");

        assert_eq!(request.headers()[HOST], "www.example.com");
    }

    #[test]
    fn a_missing_host_header_is_filled_in_from_the_connect_target() {
        let request = rewritten(request("/x"), "example.com:443");

        assert_eq!(request.headers()[HOST], "example.com:443");
    }

    #[test]
    fn an_http_2_requests_authority_becomes_the_host_header() {
        let request = rewritten(
            http2_request("https://www.example.com/api?x=1"),
            "example.com:443",
        );

        assert_eq!(request.headers()[HOST], "www.example.com");
        assert_eq!(request.uri(), "/api?x=1");
        assert_eq!(request.version(), Version::HTTP_11);
    }

    #[test]
    fn an_http_2_requests_authority_keeps_a_port_it_named() {
        let request = rewritten(
            http2_request("https://example.com:8443/x"),
            "example.com:8443",
        );

        assert_eq!(request.headers()[HOST], "example.com:8443");
    }

    #[test]
    fn an_http_2_request_that_names_no_authority_falls_back_to_the_connect_target() {
        let request = rewritten(http2_request("/x"), "example.com:443");

        assert_eq!(request.headers()[HOST], "example.com:443");
    }

    #[test]
    fn the_host_the_origin_is_told_is_never_written_twice() {
        let request = rewritten(
            http2_request("https://www.example.com/api"),
            "example.com:443",
        );

        assert_eq!(request.headers().get_all(HOST).iter().count(), 1);
    }

    #[test]
    fn proxy_only_headers_do_not_reach_the_origin() {
        let mut request = request("/x");
        let headers: &mut HeaderMap = request.headers_mut();
        headers.insert("proxy-connection", HeaderValue::from_static("keep-alive"));
        headers.insert("proxy-authorization", HeaderValue::from_static("Basic abc"));
        headers.insert("authorization", HeaderValue::from_static("Bearer token"));

        let request = rewritten(request, "example.com:443");

        assert!(request.headers().get("proxy-connection").is_none());
        assert!(request.headers().get("proxy-authorization").is_none());
        // The client's own credentials are for the origin, not for the proxy.
        assert_eq!(request.headers()["authorization"], "Bearer token");
    }

    #[test]
    fn the_headers_http_2_forbids_do_not_reach_the_origin_either() {
        // The same stripping does both jobs: every connection-specific header
        // HTTP/2 refuses is a hop-by-hop header a proxy must not forward, so
        // there is one list and not two.
        let mut request = http2_request("https://example.com/x");
        let headers: &mut HeaderMap = request.headers_mut();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("accept", HeaderValue::from_static("*/*"));

        let request = rewritten(request, "example.com:443");

        for forbidden in ["connection", "keep-alive", "transfer-encoding", "upgrade"] {
            assert!(
                request.headers().get(forbidden).is_none(),
                "{forbidden} was forwarded"
            );
        }
        assert_eq!(request.headers()["accept"], "*/*");
    }

    #[test]
    fn the_versions_a_session_can_deliver_are_the_ones_carried() {
        assert!(carriable(Version::HTTP_10));
        assert!(carriable(Version::HTTP_11));
        assert!(carriable(Version::HTTP_2));
    }

    #[test]
    fn a_version_no_handshake_agreed_to_is_not_carried() {
        assert!(!carriable(Version::HTTP_09));
        assert!(!carriable(Version::HTTP_3));
    }
}
