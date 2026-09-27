//! Stopping one exchange, showing it, and carrying out what was decided.
//!
//! ```text
//! request ──▶ does a breakpoint match?
//!                 │ no ─────────────────────────────▶ carried on, untouched
//!                 │ yes
//!                 ▼
//!             read the start of the body
//!                 ▼
//!             tell whoever is watching ──▶ wait ──▶ continue ──▶ apply the edit
//!                                                   cancel   ──▶ nothing is sent
//! ```
//!
//! Two functions, one per half of an exchange, and everything either of them
//! does to a message happens here rather than in the pipeline that calls
//! them. The pipeline asks; this decides what the message becomes.
//!
//! # What an edit may change, and what it may not
//!
//! A request's method, URL, headers, and body; a response's status, headers,
//! and body. What a URL edit *means* depends on which route the request took,
//! and it is not the same on both:
//!
//! ```text
//! http://example.com/a?b   absolute  ──▶ the whole URL is replaced,
//!                                        host and port included
//! /a?b                     origin    ──▶ the path and query are replaced,
//!                                        and the host is not
//! ```
//!
//! An intercepted request is the second. It is inside a TLS session opened to
//! one host, verified against that host's certificate, and answered with a
//! certificate minted for it; sending it to a different host would be a
//! different connection to a different server, which is what a Map Remote
//! rule is for. So the host in an edited URL is ignored there, and the path
//! and query are taken.
//!
//! # A body is bytes
//!
//! An edited body replaces the bytes on the wire, and `Content-Length` is set
//! to match. Nothing is decompressed, re-compressed, or re-encoded: a
//! response that arrived gzipped is shown gzipped, and an edit to it is the
//! gzip a client will try to read. What a breakpoint offers is the message,
//! not a view of it.
//!
//! A body that was too long to be shown in full cannot be replaced — see
//! [`stage`](super::stage) — and an edit that tries is reported and ignored
//! rather than silently throwing the rest of the body away.

use hyper::header::{HeaderName, HeaderValue, CONTENT_LENGTH, TRANSFER_ENCODING};
use hyper::http::uri::PathAndQuery;
use hyper::{HeaderMap, Request, Response, StatusCode, Uri};
use proxy_ipc::{
    BreakpointDirection, PauseDecision, PauseId, PausedExchange, PausedRequest, PausedResponse,
    PausedWebSocketMessage, RequestEdit, ResponseEdit, MAX_BREAKPOINT_BODY_BYTES,
};
use proxy_storage::{BodyCapture, Headers, StreamDirection};
use tokio::time::Duration;

use super::breakpoint::Breakpoints;
use super::stage::StagedBody;
use super::{Exchange, TargetForm};
use crate::capture::message;
use crate::shutdown::Shutdown;

/// How long the start of a body is waited for before the pause is shown.
///
/// A body that has not finished arriving by then is shown as far as it got
/// and delivered in full afterwards. The number exists for the bodies that
/// never finish arriving at all: a server-sent event stream is open for as
/// long as the origin feels like it, and a pause that waited for the end of
/// one would never appear.
const STAGE_WINDOW: Duration = Duration::from_secs(2);

/// What a paused request came to.
pub enum RequestOutcome<B: hyper::body::Body> {
    /// Send this upstream. The client's own request, or the edited one.
    Carry(Request<StagedBody<B>>),
    /// Send nothing: the developer stopped it here.
    ///
    /// The request comes back anyway, and unedited, so the exchange is still
    /// captured. A request that was cancelled is traffic — it happened, and
    /// the developer stopped it — and a request that simply vanished from the
    /// list would be the proxy hiding what it did.
    Cancelled(Request<StagedBody<B>>),
}

/// What a paused response came to.
pub enum ResponseOutcome<B: hyper::body::Body> {
    /// Deliver this.
    Deliver {
        /// The origin's own response, or the edited one.
        response: Response<StagedBody<B>>,
        /// Whether it is the edited one, so the capture knows to record the
        /// head again rather than doing it for every response that goes past.
        edited: bool,
    },
    /// Deliver nothing: the developer stopped it here.
    Cancelled,
}

/// Holds `request` still if a breakpoint matches, and applies what was
/// decided.
///
/// Returns as soon as there is something to do with the request, which for
/// the usual case — no breakpoint, or nobody watching — is immediately, with
/// the request wrapped and not read from.
pub async fn request<B>(
    breakpoints: &Breakpoints,
    exchange: &Exchange,
    request: Request<B>,
    shutdown: &Shutdown,
) -> RequestOutcome<B>
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    let Some(breakpoint_id) =
        breakpoints.matching(BreakpointDirection::Request, &exchange.context())
    else {
        return RequestOutcome::Carry(request.map(StagedBody::unread));
    };

    let (mut parts, body) = request.into_parts();
    let declared = declared_length(&parts.headers);
    let mut staged = StagedBody::read(body, MAX_BREAKPOINT_BODY_BYTES, STAGE_WINDOW).await;

    let pause_id = PauseId::next();
    let paused = PausedExchange {
        pause_id,
        breakpoint_id,
        direction: BreakpointDirection::Request,
        method: parts.method.as_str().to_owned(),
        url: exchange.url.clone(),
        request: Some(PausedRequest {
            method: parts.method.as_str().to_owned(),
            url: exchange.url.clone(),
            headers: message::headers(&parts.headers),
            body: captured(&staged, declared),
        }),
        response: None,
        websocket: None,
    };

    let Some(pending) = breakpoints.begin(paused) else {
        return RequestOutcome::Carry(Request::from_parts(parts, staged));
    };
    println!(
        "Breakpoint {breakpoint_id} paused {} {}",
        parts.method, exchange.url
    );

    let decision = pending.wait(breakpoints, shutdown).await;
    let edit = match decision {
        PauseDecision::Cancel => {
            println!("Breakpoint {breakpoint_id} cancelled {}", exchange.url);

            return RequestOutcome::Cancelled(Request::from_parts(parts, staged));
        }
        PauseDecision::Continue { request, .. } => request,
    };

    if let Some(edit) = edit {
        apply_to_request(&mut parts, &mut staged, edit, exchange);
    }

    RequestOutcome::Carry(Request::from_parts(parts, staged))
}

/// Holds `response` still if a breakpoint matches, and applies what was
/// decided.
///
/// `exchange` describes the *request* that produced it, because that is what
/// a breakpoint matches on: a response has no host, path, or method of its
/// own.
pub async fn response<B>(
    breakpoints: &Breakpoints,
    exchange: &Exchange,
    response: Response<B>,
    shutdown: &Shutdown,
) -> ResponseOutcome<B>
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    let Some(breakpoint_id) =
        breakpoints.matching(BreakpointDirection::Response, &exchange.context())
    else {
        return ResponseOutcome::Deliver {
            response: response.map(StagedBody::unread),
            edited: false,
        };
    };

    let (mut parts, body) = response.into_parts();
    let declared = declared_length(&parts.headers);
    let mut staged = StagedBody::read(body, MAX_BREAKPOINT_BODY_BYTES, STAGE_WINDOW).await;

    let pause_id = PauseId::next();
    let paused = PausedExchange {
        pause_id,
        breakpoint_id,
        direction: BreakpointDirection::Response,
        method: exchange.method.clone(),
        url: exchange.url.clone(),
        request: None,
        response: Some(PausedResponse {
            status: parts.status.as_u16(),
            headers: message::headers(&parts.headers),
            body: captured(&staged, declared),
        }),
        websocket: None,
    };

    let Some(pending) = breakpoints.begin(paused) else {
        return ResponseOutcome::Deliver {
            response: Response::from_parts(parts, staged),
            edited: false,
        };
    };
    println!(
        "Breakpoint {breakpoint_id} paused {} from {}",
        parts.status, exchange.url
    );

    let decision = pending.wait(breakpoints, shutdown).await;
    let edit = match decision {
        PauseDecision::Cancel => {
            println!(
                "Breakpoint {breakpoint_id} cancelled the response from {}",
                exchange.url
            );

            return ResponseOutcome::Cancelled;
        }
        PauseDecision::Continue { response, .. } => response,
    };

    let edited = edit.is_some();
    if let Some(edit) = edit {
        apply_to_response(&mut parts, &mut staged, edit);
    }

    ResponseOutcome::Deliver {
        response: Response::from_parts(parts, staged),
        edited,
    }
}

/// What a paused WebSocket message came to.
#[derive(Debug)]
pub enum WebSocketOutcome {
    /// Forward these bytes — the original frames, or a re-encoded edit.
    Forward(Vec<u8>),
    /// Drop this message. The connection stays open.
    Drop,
}

/// Holds a text WebSocket message still if a breakpoint matches.
///
/// Binary messages are never paused. A message nobody is watching is
/// forwarded as the original frames, byte for byte.
pub async fn websocket_text(
    breakpoints: &Breakpoints,
    exchange: &Exchange,
    direction: StreamDirection,
    payload: &[u8],
    original: Vec<u8>,
    shutdown: &Shutdown,
) -> WebSocketOutcome {
    let Some(breakpoint_id) =
        breakpoints.matching(BreakpointDirection::WebSocket, &exchange.context())
    else {
        return WebSocketOutcome::Forward(original);
    };

    let shown = if payload.len() <= MAX_BREAKPOINT_BODY_BYTES {
        BodyCapture::complete(payload.to_vec())
    } else {
        BodyCapture::truncated(
            payload[..MAX_BREAKPOINT_BODY_BYTES].to_vec(),
            payload.len() as u64,
        )
    };

    let pause_id = PauseId::next();
    let paused = PausedExchange {
        pause_id,
        breakpoint_id,
        direction: BreakpointDirection::WebSocket,
        method: exchange.method.clone(),
        url: exchange.url.clone(),
        request: None,
        response: None,
        websocket: Some(PausedWebSocketMessage {
            direction,
            payload: shown.clone(),
        }),
    };

    let Some(pending) = breakpoints.begin(paused) else {
        return WebSocketOutcome::Forward(original);
    };
    println!(
        "Breakpoint {breakpoint_id} paused a WebSocket message on {}",
        exchange.url
    );

    let decision = pending.wait(breakpoints, shutdown).await;
    match decision {
        PauseDecision::Cancel => {
            println!(
                "Breakpoint {breakpoint_id} dropped a WebSocket message on {}",
                exchange.url
            );
            WebSocketOutcome::Drop
        }
        PauseDecision::Continue { websocket, .. } => {
            let Some(edit) = websocket.and_then(|edit| edit.payload) else {
                return WebSocketOutcome::Forward(original);
            };
            if shown.is_truncated() {
                eprintln!(
                    "Breakpoint: the edited WebSocket payload was ignored; the original was \
                     longer than could be shown, and the original was sent"
                );
                return WebSocketOutcome::Forward(original);
            }
            let mask = direction == StreamDirection::ClientToServer;
            WebSocketOutcome::Forward(crate::websocket::encode_text(&edit, mask))
        }
    }
}

/// Applies a request edit to the message that is about to be sent.
fn apply_to_request<B>(
    parts: &mut hyper::http::request::Parts,
    staged: &mut StagedBody<B>,
    edit: RequestEdit,
    exchange: &Exchange,
) where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    if let Some(method) = edit
        .method
        .as_deref()
        .and_then(|method| method.parse().ok())
    {
        parts.method = method;
    }
    if let Some(url) = &edit.url {
        if let Some(uri) = edited_uri(url, exchange.form) {
            parts.uri = uri;
        }
    }
    if let Some(headers) = edit.headers {
        parts.headers = header_map(&headers);
    }
    if let Some(body) = edit.body {
        replace_body(staged, body, &mut parts.headers, "request");
    }
}

/// Applies a response edit to the message that is about to be delivered.
fn apply_to_response<B>(
    parts: &mut hyper::http::response::Parts,
    staged: &mut StagedBody<B>,
    edit: ResponseEdit,
) where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    if let Some(status) = edit
        .status
        .and_then(|status| StatusCode::from_u16(status).ok())
    {
        parts.status = status;
        // The reason phrase the origin wrote described the status the origin
        // sent. Dropping it leaves the standard phrase for the edited status,
        // which is the only honest answer.
        parts.extensions.remove::<hyper::ext::ReasonPhrase>();
    }
    if let Some(headers) = edit.headers {
        parts.headers = header_map(&headers);
    }
    if let Some(body) = edit.body {
        replace_body(staged, body, &mut parts.headers, "response");
    }
}

/// Replaces a staged body, and makes the framing headers say so.
///
/// A body that was too long to be shown is left alone and the attempt is
/// reported: what was on screen was the first part of it, and replacing all
/// of it with an edit of that part would throw the rest away.
fn replace_body<B>(staged: &mut StagedBody<B>, body: Vec<u8>, headers: &mut HeaderMap, half: &str)
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    if !staged.is_complete() {
        eprintln!(
            "Breakpoint: the edited {half} body was ignored; the original was longer than \
             could be shown, and the original was sent"
        );

        return;
    }

    // Set rather than adjusted: the message now carries exactly these bytes,
    // and a `Transfer-Encoding` describing how the old ones were framed is
    // not a description of anything.
    headers.remove(TRANSFER_ENCODING);
    if let Ok(length) = HeaderValue::from_str(&body.len().to_string()) {
        headers.insert(CONTENT_LENGTH, length);
    }

    staged.replace(body);
}

/// The URI an edited URL becomes, in the form this route uses.
///
/// `None` for a URL that is not one, which leaves the message as it was: an
/// edit that cannot be carried out is not a reason to send a request nobody
/// wrote.
fn edited_uri(url: &str, form: TargetForm) -> Option<Uri> {
    let uri: Uri = url.parse().ok()?;

    match form {
        TargetForm::Absolute => Some(uri),
        // See the module documentation: inside an intercepted session the
        // host is the one the connection was opened to, and only the path and
        // query are the request's to change.
        TargetForm::Origin => {
            let path = uri
                .path_and_query()
                .cloned()
                .or_else(|| url.parse::<PathAndQuery>().ok())?;

            Some(Uri::from(path))
        }
    }
}

/// The captured model's headers as the header section of a message.
///
/// A name or a value the HTTP library will not accept is dropped rather than
/// refused: both were checked at the IPC boundary, so anything left here is a
/// disagreement between two libraries and not something the developer can
/// act on — and a header that cannot be sent must not become a request that
/// cannot be sent.
fn header_map(headers: &Headers) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(headers.len());

    for header in headers {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name.as_bytes()),
            HeaderValue::from_bytes(header.value.as_bytes()),
        ) else {
            eprintln!("Breakpoint: an edited header could not be sent and was left out");
            continue;
        };

        map.append(name, value);
    }

    map
}

/// How long the message says its body is, when it says.
fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// As much of a staged body as a client is shown, and how much there was.
///
/// A body that ended is shown whole. One that did not is shown as far as the
/// limit and marked truncated, with the length the message declared when it
/// declared one — which is the only place the real length can come from.
fn captured<B>(staged: &StagedBody<B>, declared: Option<u64>) -> BodyCapture
where
    B: hyper::body::Body<Data = hyper::body::Bytes> + Unpin,
{
    let mut data = staged.data();

    if staged.is_complete() {
        return BodyCapture::complete(data);
    }

    data.truncate(MAX_BREAKPOINT_BODY_BYTES);
    let size = declared.unwrap_or(0).max(data.len() as u64);

    BodyCapture::truncated(data, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use http_body_util::{BodyExt, Full};
    use hyper::body::Bytes;
    use proxy_ipc::{BreakpointControl, NewBreakpoint, PauseDecision, PauseNotice, PauseStream};
    use proxy_storage::HeaderValue as CapturedValue;
    use rules::RuleMatcher;

    use crate::controls::Exchange;

    fn breakpoints() -> Arc<Breakpoints> {
        Arc::new(Breakpoints::new())
    }

    fn watching(breakpoints: &Arc<Breakpoints>) -> PauseStream {
        PauseStream::attach(Arc::clone(breakpoints) as Arc<dyn BreakpointControl>)
    }

    fn exchange() -> Exchange {
        Exchange::forwarded(
            "api.example.com",
            "/orders",
            "POST",
            "http://api.example.com/orders",
        )
    }

    fn outgoing(body: &[u8]) -> Request<Full<Bytes>> {
        Request::builder()
            .method("POST")
            .uri("http://api.example.com/orders")
            .header("content-type", "application/json")
            .header("content-length", body.len().to_string())
            .body(Full::new(Bytes::copy_from_slice(body)))
            .expect("a valid request")
    }

    fn answered(status: u16, body: &[u8]) -> Response<Full<Bytes>> {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::copy_from_slice(body)))
            .expect("a valid response")
    }

    fn on_requests() -> NewBreakpoint {
        NewBreakpoint::new(
            BreakpointDirection::Request,
            RuleMatcher::any().with_host("api.example.com"),
        )
    }

    fn on_responses() -> NewBreakpoint {
        NewBreakpoint::new(
            BreakpointDirection::Response,
            RuleMatcher::any().with_host("api.example.com"),
        )
    }

    /// The pause a watcher is told about next.
    async fn next_pause(watcher: &mut PauseStream) -> PausedExchange {
        match watcher.recv().await.expect("the stream is healthy") {
            PauseNotice::Paused(paused) => paused,
            notice => panic!("expected a pause: {notice:?}"),
        }
    }

    async fn body_of<B>(body: B) -> Vec<u8>
    where
        B: hyper::body::Body<Data = Bytes> + Unpin,
        B::Error: std::fmt::Debug,
    {
        body.collect()
            .await
            .expect("the body completed")
            .to_bytes()
            .to_vec()
    }

    fn edited(headers: &[(&str, &str)]) -> Headers {
        let mut edited = Headers::new();
        for (name, value) in headers {
            edited.push(*name, CapturedValue::Text((*value).to_owned()));
        }

        edited
    }

    #[tokio::test]
    async fn a_request_nothing_matches_is_carried_untouched() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);

        let outcome = request(
            &breakpoints,
            &exchange(),
            outgoing(b"{}"),
            &Shutdown::never(),
        )
        .await;

        match outcome {
            RequestOutcome::Carry(request) => {
                assert_eq!(request.method(), "POST");
                assert_eq!(body_of(request.into_body()).await, b"{}");
            }
            RequestOutcome::Cancelled(_) => panic!("nothing should have been cancelled"),
        }
    }

    #[tokio::test]
    async fn a_paused_request_is_shown_with_everything_that_may_be_edited() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_requests());

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                request(
                    &breakpoints,
                    &exchange(),
                    outgoing(b"{\"id\":1}"),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        let shown = paused.request.as_ref().expect("a request pause");

        assert_eq!(shown.method, "POST");
        assert_eq!(shown.url, "http://api.example.com/orders");
        assert_eq!(shown.headers.text("content-type"), Some("application/json"));
        assert_eq!(shown.body.data(), b"{\"id\":1}");
        assert!(!shown.body.is_truncated());

        breakpoints.resolve(paused.pause_id, PauseDecision::unchanged());
        let outcome = pausing.await.expect("the pause finished");

        match outcome {
            RequestOutcome::Carry(request) => {
                assert_eq!(body_of(request.into_body()).await, b"{\"id\":1}");
            }
            RequestOutcome::Cancelled(_) => panic!("nothing should have been cancelled"),
        }
    }

    #[tokio::test]
    async fn an_edited_request_is_what_goes_upstream() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_requests());

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                request(
                    &breakpoints,
                    &exchange(),
                    outgoing(b"{}"),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        breakpoints.resolve(
            paused.pause_id,
            PauseDecision::Continue {
                request: Some(RequestEdit {
                    method: Some("PUT".to_owned()),
                    url: Some("http://api.example.com/v2/orders?dry=1".to_owned()),
                    headers: Some(edited(&[("x-edited", "yes")])),
                    body: Some(b"{\"edited\":true}".to_vec()),
                }),
                response: None,
                websocket: None,
            },
        );

        match pausing.await.expect("the pause finished") {
            RequestOutcome::Carry(request) => {
                assert_eq!(request.method(), "PUT");
                assert_eq!(request.uri(), "http://api.example.com/v2/orders?dry=1");
                assert_eq!(request.headers()["x-edited"], "yes");
                assert_eq!(request.headers()[CONTENT_LENGTH], "15");
                assert!(request.headers().get("content-type").is_none());
                assert_eq!(body_of(request.into_body()).await, b"{\"edited\":true}");
            }
            RequestOutcome::Cancelled(_) => panic!("nothing should have been cancelled"),
        }
    }

    #[tokio::test]
    async fn an_intercepted_request_keeps_the_host_its_session_was_opened_to() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_requests());
        let exchange = Exchange::intercepted(
            "api.example.com",
            "/orders",
            "POST",
            "https://api.example.com/orders",
        );

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                let mut request = outgoing(b"{}");
                *request.uri_mut() = "/orders".parse().expect("a valid target");

                self::request(&breakpoints, &exchange, request, &Shutdown::never()).await
            })
        };

        let paused = next_pause(&mut watcher).await;
        breakpoints.resolve(
            paused.pause_id,
            PauseDecision::Continue {
                request: Some(RequestEdit {
                    url: Some("https://elsewhere.invalid/v2/orders".to_owned()),
                    ..RequestEdit::default()
                }),
                response: None,
                websocket: None,
            },
        );

        match pausing.await.expect("the pause finished") {
            RequestOutcome::Carry(request) => {
                assert_eq!(request.uri(), "/v2/orders");
                assert_eq!(request.uri().host(), None);
            }
            RequestOutcome::Cancelled(_) => panic!("nothing should have been cancelled"),
        }
    }

    #[tokio::test]
    async fn a_cancelled_request_is_not_carried() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_requests());

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                request(
                    &breakpoints,
                    &exchange(),
                    outgoing(b"{}"),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        breakpoints.resolve(paused.pause_id, PauseDecision::Cancel);

        assert!(matches!(
            pausing.await.expect("the pause finished"),
            RequestOutcome::Cancelled(_)
        ));
    }

    #[tokio::test]
    async fn an_edited_response_is_what_reaches_the_client() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_responses());

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                response(
                    &breakpoints,
                    &exchange(),
                    answered(200, b"{\"ok\":true}"),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        let shown = paused.response.as_ref().expect("a response pause");
        assert_eq!(shown.status, 200);
        assert_eq!(shown.body.data(), b"{\"ok\":true}");
        assert!(paused.request.is_none());

        breakpoints.resolve(
            paused.pause_id,
            PauseDecision::Continue {
                request: None,
                response: Some(ResponseEdit {
                    status: Some(503),
                    headers: Some(edited(&[("retry-after", "30")])),
                    body: Some(b"nope".to_vec()),
                }),
                websocket: None,
            },
        );

        match pausing.await.expect("the pause finished") {
            ResponseOutcome::Deliver { response, edited } => {
                assert!(edited);
                assert_eq!(response.status(), 503);
                assert_eq!(response.headers()["retry-after"], "30");
                assert_eq!(response.headers()[CONTENT_LENGTH], "4");
                assert_eq!(body_of(response.into_body()).await, b"nope");
            }
            ResponseOutcome::Cancelled => panic!("nothing should have been cancelled"),
        }
    }

    #[tokio::test]
    async fn a_cancelled_response_is_not_delivered() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_responses());

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            tokio::spawn(async move {
                response(
                    &breakpoints,
                    &exchange(),
                    answered(200, b"{}"),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        breakpoints.resolve(paused.pause_id, PauseDecision::Cancel);

        assert!(matches!(
            pausing.await.expect("the pause finished"),
            ResponseOutcome::Cancelled
        ));
    }

    #[tokio::test]
    async fn a_body_too_long_to_show_is_marked_and_still_sent_in_full() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        breakpoints.create(on_requests());
        let long = vec![b'x'; MAX_BREAKPOINT_BODY_BYTES * 2];

        let pausing = {
            let breakpoints = Arc::clone(&breakpoints);
            let long = long.clone();
            tokio::spawn(async move {
                request(
                    &breakpoints,
                    &exchange(),
                    outgoing(&long),
                    &Shutdown::never(),
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        let shown = paused.request.as_ref().expect("a request pause");
        assert!(shown.body.is_truncated());
        assert_eq!(shown.body.captured_len(), MAX_BREAKPOINT_BODY_BYTES);
        assert_eq!(shown.body.size(), long.len() as u64);

        // An edit for it is ignored rather than throwing the rest away.
        breakpoints.resolve(
            paused.pause_id,
            PauseDecision::Continue {
                request: Some(RequestEdit {
                    body: Some(b"short".to_vec()),
                    ..RequestEdit::default()
                }),
                response: None,
                websocket: None,
            },
        );

        match pausing.await.expect("the pause finished") {
            RequestOutcome::Carry(request) => {
                assert_eq!(body_of(request.into_body()).await, long);
            }
            RequestOutcome::Cancelled(_) => panic!("nothing should have been cancelled"),
        }
    }

    #[test]
    fn an_edited_url_that_is_not_one_leaves_the_target_alone() {
        assert!(edited_uri("not a url", TargetForm::Absolute).is_none());
        assert_eq!(
            edited_uri("http://example.com/a?b=1", TargetForm::Absolute)
                .expect("a URL")
                .to_string(),
            "http://example.com/a?b=1"
        );
        assert_eq!(
            edited_uri("/a?b=1", TargetForm::Origin)
                .expect("a URL")
                .to_string(),
            "/a?b=1"
        );
    }

    #[test]
    fn a_declared_length_is_read_when_the_message_declares_one() {
        let mut headers = HeaderMap::new();
        assert_eq!(declared_length(&headers), None);

        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("1234"));
        assert_eq!(declared_length(&headers), Some(1234));

        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("not a number"));
        assert_eq!(declared_length(&headers), None);
    }

    #[test]
    fn a_header_that_could_not_be_sent_is_left_out_rather_than_fatal() {
        let mut headers = Headers::new();
        headers.push("x-fine", CapturedValue::Text("yes".to_owned()));
        headers.push("not a name", CapturedValue::Text("no".to_owned()));

        let map = header_map(&headers);

        assert_eq!(map.len(), 1);
        assert_eq!(map["x-fine"], "yes");
    }

    fn on_websocket() -> NewBreakpoint {
        NewBreakpoint::new(
            BreakpointDirection::WebSocket,
            RuleMatcher::any().with_host("api.example.com"),
        )
    }

    #[tokio::test]
    async fn an_edited_text_message_is_the_one_that_is_forwarded() {
        let breakpoints = breakpoints();
        breakpoints.create(on_websocket());
        let mut watcher = watching(&breakpoints);
        let original = vec![0x81, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let shutdown = crate::shutdown::Shutdown::never();
        let exchange = exchange();

        let forwarding = {
            let breakpoints = Arc::clone(&breakpoints);
            let exchange = exchange.clone();
            let shutdown = shutdown.clone();
            let original = original.clone();
            tokio::spawn(async move {
                websocket_text(
                    &breakpoints,
                    &exchange,
                    proxy_storage::StreamDirection::ClientToServer,
                    b"hello",
                    original,
                    &shutdown,
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        assert_eq!(paused.direction, BreakpointDirection::WebSocket);
        assert_eq!(
            paused
                .websocket
                .as_ref()
                .expect("a websocket pause")
                .payload
                .data(),
            b"hello"
        );

        breakpoints.resolve(
            paused.pause_id,
            PauseDecision::Continue {
                request: None,
                response: None,
                websocket: Some(proxy_ipc::WebSocketEdit {
                    payload: Some(b"edited".to_vec()),
                }),
            },
        );

        match forwarding.await.expect("the pause finished") {
            WebSocketOutcome::Forward(bytes) => {
                let (frame, consumed) =
                    crate::websocket::frame::decode(&bytes).expect("an edited frame");
                assert_eq!(consumed, bytes.len());
                assert_eq!(frame.opcode, crate::websocket::frame::Opcode::Text);
                assert_eq!(frame.payload, b"edited");
            }
            other => panic!("expected a forward: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelling_a_text_message_drops_it_and_leaves_the_connection() {
        let breakpoints = breakpoints();
        breakpoints.create(on_websocket());
        let mut watcher = watching(&breakpoints);
        let original = vec![0x81, 0x05, b'h', b'e', b'l', b'l', b'o'];
        let shutdown = crate::shutdown::Shutdown::never();
        let exchange = exchange();

        let forwarding = {
            let breakpoints = Arc::clone(&breakpoints);
            let exchange = exchange.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                websocket_text(
                    &breakpoints,
                    &exchange,
                    proxy_storage::StreamDirection::ServerToClient,
                    b"hello",
                    original,
                    &shutdown,
                )
                .await
            })
        };

        let paused = next_pause(&mut watcher).await;
        breakpoints.resolve(paused.pause_id, PauseDecision::Cancel);

        assert!(matches!(
            forwarding.await.expect("the pause finished"),
            WebSocketOutcome::Drop
        ));
    }
}
