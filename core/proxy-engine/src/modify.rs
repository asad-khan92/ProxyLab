//! Carrying out Modify Request and Modify Response rules.
//!
//! ```text
//! client ─▶ request breakpoint ─▶ rules ─▶ Modify Request ─▶ origin
//!                                                              │
//! client ◀─ response breakpoint ◀─ Modify Response ◀───────────┘
//! ```
//!
//! What a transformation changes is decided in `rules`, which is pure. This
//! module is the other half, as `http1::ruled` is for the decisions: it puts
//! an edit onto a hyper message, reads a body when an edit needs the whole of
//! one, and keeps the framing honest afterwards.
//!
//! # Where, exactly
//!
//! **A request is transformed only if it is going to an origin** — its own,
//! or the one a Map Remote names. A request the proxy answers itself (Block,
//! Map Local, Redirect) is recorded as the client sent it, because a
//! transformation changes what is *sent*, and nothing was.
//!
//! It happens after a request breakpoint, and so to the request a developer
//! may have just edited; the rules were matched against that same request.
//! Nearest the client is the person, and between the person and the origin
//! are the rules — on the way out and, mirrored, on the way back: **a response
//! is transformed before a response breakpoint shows it**, so what a
//! developer holds still is what the client is about to be given.
//!
//! Only responses from an origin are transformed — the same responses a
//! response breakpoint stops, for the same reason: every one of them passes
//! through `service::into_client_response`, and a response the proxy
//! generated was already written by a rule.
//!
//! **A WebSocket handshake is never transformed**, in either direction. Its
//! headers and its `101` are what the two ends agreed on, and a rule that
//! changed them would break a tunnel it cannot see into.
//!
//! # A body
//!
//! A replacement needs nothing from the body it replaces, and the original is
//! not read. A JSON field edit needs all of it, so the body is read first — up
//! to [`BODY_LIMIT`] bytes and for up to [`BODY_WINDOW`] — and a body that
//! is larger, slower, compressed, or not a JSON object is sent exactly as it
//! was, with a line in the log saying so. An event stream is the usual body
//! that is slower: it never ends, so nothing is edited and all of it is
//! forwarded.
//!
//! Whatever replaces a body is sent with a `Content-Length` of its own, and a
//! replacement drops `Content-Encoding`: the text a rule carries is not
//! gzipped, and a header saying it was would be a body no client can read.
//!
//! # Nothing is logged but which rule
//!
//! A header a rule sets is as likely as not an `Authorization`. The log says
//! which rules transformed an exchange and never what they wrote.

use hyper::body::{Body, Bytes};
use hyper::header::{HeaderName, HeaderValue, CONTENT_ENCODING, CONTENT_LENGTH, TRANSFER_ENCODING};
use hyper::http::uri::PathAndQuery;
use hyper::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use rules::{apply_query, BodyEdit, HeaderEdit, ModifyRequest, ModifyResponse, Transformation};
use tokio::time::Duration;

use crate::controls::StagedBody;

/// The most of a body that is read to apply a JSON field edit to it.
///
/// A JSON document is edited whole, so it is held whole: this bounds what one
/// exchange can make the proxy hold for the purpose. An API response larger
/// than this is sent unedited.
pub const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// How long the rest of a body is waited for, for the same edit.
///
/// A body that has not ended by then — an event stream, or an origin that is
/// taking its time — is sent unedited and in full.
pub const BODY_WINDOW: Duration = Duration::from_secs(5);

/// Applies `transformations`, in order, to a request about to go upstream.
///
/// Returns the request and whether any transformation applied to it, so the
/// caller knows to record the head again.
pub async fn request<B>(
    request: Request<StagedBody<B>>,
    transformations: &[Transformation<ModifyRequest>],
) -> (Request<StagedBody<B>>, bool)
where
    B: Body<Data = Bytes> + Unpin,
{
    if transformations.is_empty() {
        return (request, false);
    }

    let (mut parts, mut body) = request.into_parts();

    for transformation in transformations {
        let change = &transformation.change;

        if let Some(method) = &change.method {
            // Checked as a token when the rule was written.
            if let Ok(method) = Method::from_bytes(method.as_bytes()) {
                parts.method = method;
            }
        }

        if change.path.is_some() || !change.query.is_empty() {
            let path = match &change.path {
                Some(edit) => edit.apply(parts.uri.path()),
                None => parts.uri.path().to_owned(),
            };
            let query = apply_query(parts.uri.query(), &change.query);

            match with_path_and_query(&parts.uri, &path, query.as_deref()) {
                Some(uri) => parts.uri = uri,
                None => eprintln!(
                    "Rule {} left a request's URL alone: the edited one is not a URL",
                    transformation.rule
                ),
            }
        }

        apply_headers(&mut parts.headers, &change.headers);

        if let Some(edit) = &change.body {
            apply_body(
                &mut body,
                &mut parts.headers,
                edit,
                transformation,
                "request",
            )
            .await;
        }
    }

    println!("{}", applied_report("request", transformations));

    (Request::from_parts(parts, body), true)
}

/// Applies `transformations`, in order, to a response on its way back.
///
/// `method` is the request's, because a response to `HEAD` has no body to
/// edit whatever its headers say.
pub async fn response<B>(
    response: Response<B>,
    transformations: &[Transformation<ModifyResponse>],
    method: &str,
) -> (Response<StagedBody<B>>, bool)
where
    B: Body<Data = Bytes> + Unpin,
{
    let (mut parts, body) = response.into_parts();
    let mut body = StagedBody::unread(body);

    if transformations.is_empty() {
        return (Response::from_parts(parts, body), false);
    }

    for transformation in transformations {
        let change = &transformation.change;

        if let Some(status) = change
            .status
            .and_then(|status| StatusCode::from_u16(status).ok())
        {
            parts.status = status;
            // The origin's reason phrase described the origin's status.
            parts.extensions.remove::<hyper::ext::ReasonPhrase>();
        }

        apply_headers(&mut parts.headers, &change.headers);

        if let Some(edit) = &change.body {
            if carries_body(method, parts.status) {
                apply_body(
                    &mut body,
                    &mut parts.headers,
                    edit,
                    transformation,
                    "response",
                )
                .await;
            } else {
                eprintln!(
                    "Rule {} left a response body alone: a {} to {method} has none",
                    transformation.rule, parts.status
                );
            }
        }
    }

    println!("{}", applied_report("response", transformations));

    (Response::from_parts(parts, body), true)
}

/// The same URI with a different path and query, scheme and authority kept.
///
/// `None` for a path and query that together are not one, which leaves the
/// request as it was: an edit that cannot be carried out is not a reason to
/// send a request nobody wrote.
fn with_path_and_query(uri: &Uri, path: &str, query: Option<&str>) -> Option<Uri> {
    let written = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };

    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(written.parse::<PathAndQuery>().ok()?);

    Uri::from_parts(parts).ok()
}

/// Applies header edits, in order.
///
/// Names and values were checked when the rule was written. One the HTTP
/// library still will not take is left out rather than fatal, which is what a
/// breakpoint's edited header gets too.
fn apply_headers(headers: &mut HeaderMap, edits: &[HeaderEdit]) {
    for edit in edits {
        let Ok(name) = HeaderName::from_bytes(edit.name().as_bytes()) else {
            continue;
        };

        match edit {
            HeaderEdit::Set { value, .. } => {
                if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                    headers.insert(name, value);
                }
            }
            HeaderEdit::Add { value, .. } => {
                if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                    headers.append(name, value);
                }
            }
            HeaderEdit::Remove { .. } => {
                headers.remove(name);
            }
        }
    }
}

/// Applies one body edit, or says in the log why it did not.
async fn apply_body<B, T>(
    body: &mut StagedBody<B>,
    headers: &mut HeaderMap,
    edit: &BodyEdit,
    transformation: &Transformation<T>,
    half: &str,
) where
    B: Body<Data = Bytes> + Unpin,
{
    let original = if edit.reads_body() {
        if is_encoded(headers) {
            eprintln!(
                "Rule {} left a {half} body alone: it is compressed, and a JSON edit reads JSON",
                transformation.rule
            );
            return;
        }

        body.read_more(BODY_LIMIT, BODY_WINDOW).await;
        if !body.is_complete() {
            eprintln!(
                "Rule {} left a {half} body alone: it was longer or slower than an edit waits for, \
                 and was sent in full",
                transformation.rule
            );
            return;
        }

        body.data()
    } else {
        Vec::new()
    };

    let Some(edited) = edit.apply(&original) else {
        eprintln!(
            "Rule {} left a {half} body alone: it is not a JSON object with that field's path",
            transformation.rule
        );
        return;
    };

    // What replaced the body is exactly these bytes, unencoded, and framed by
    // their own length.
    if matches!(edit, BodyEdit::Replace { .. }) {
        headers.remove(CONTENT_ENCODING);
    }
    headers.remove(TRANSFER_ENCODING);
    headers.insert(CONTENT_LENGTH, HeaderValue::from(edited.len()));

    body.replace(edited);
}

/// Whether a body is encoded as something other than itself.
fn is_encoded(headers: &HeaderMap) -> bool {
    headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
}

/// Whether a response with `status`, to a request with `method`, has a body.
fn carries_body(method: &str, status: StatusCode) -> bool {
    !(method.eq_ignore_ascii_case("HEAD")
        || status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED)
}

/// The log line saying which rules transformed a message: their ids, and
/// nothing they wrote.
fn applied_report<T>(half: &str, transformations: &[Transformation<T>]) -> String {
    let rules: Vec<String> = transformations
        .iter()
        .map(|transformation| transformation.rule.to_string())
        .collect();

    format!("Rules {} modified the {half}", rules.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::{BodyExt, Full};
    use rules::{NewRule, PathEdit, QueryEdit, Rule, RuleAction, RuleMatcher};

    fn transformation<T>(change: T) -> Transformation<T> {
        let rule =
            Rule::create(NewRule::new("t", RuleMatcher::any(), RuleAction::Allow)).expect("a rule");

        Transformation {
            rule: rule.id,
            change,
        }
    }

    fn outgoing(uri: &str, body: &'static [u8]) -> Request<StagedBody<Full<Bytes>>> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header("x-remove", "gone")
            .header("content-length", body.len())
            .body(StagedBody::unread(Full::new(Bytes::from_static(body))))
            .expect("a request")
    }

    fn answered(
        status: u16,
        headers: &[(&str, &str)],
        body: &'static [u8],
    ) -> Response<Full<Bytes>> {
        let mut builder = Response::builder().status(status);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }

        builder
            .body(Full::new(Bytes::from_static(body)))
            .expect("a response")
    }

    async fn body_of<B>(body: B) -> Vec<u8>
    where
        B: Body<Data = Bytes>,
        B::Error: std::fmt::Debug,
    {
        body.collect().await.expect("a body").to_bytes().to_vec()
    }

    #[tokio::test]
    async fn a_request_is_given_its_method_path_query_and_headers() {
        let change = ModifyRequest {
            method: Some("POST".into()),
            path: Some(PathEdit::ReplacePrefix {
                from: "/v1/".into(),
                to: "/v2/".into(),
            }),
            query: vec![QueryEdit::Set {
                name: "env".into(),
                value: "staging".into(),
            }],
            headers: vec![
                HeaderEdit::Set {
                    name: "Authorization".into(),
                    value: "Bearer t".into(),
                },
                HeaderEdit::Remove {
                    name: "X-Remove".into(),
                },
            ],
            body: None,
        };

        let (request, changed) = request(
            outgoing("http://api.example.com/v1/users?env=prod&a=1", b""),
            &[transformation(change)],
        )
        .await;

        assert!(changed);
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.uri().to_string(),
            "http://api.example.com/v2/users?env=staging&a=1"
        );
        assert_eq!(request.headers()["authorization"], "Bearer t");
        assert!(request.headers().get("x-remove").is_none());
    }

    #[tokio::test]
    async fn an_origin_form_target_stays_in_origin_form() {
        let change = ModifyRequest {
            path: Some(PathEdit::Set { path: "/b".into() }),
            ..ModifyRequest::default()
        };

        let (request, _) = request(outgoing("/a?x=1", b""), &[transformation(change)]).await;

        assert_eq!(request.uri().to_string(), "/b?x=1");
    }

    #[tokio::test]
    async fn a_replaced_body_is_framed_by_its_own_length() {
        let change = ModifyRequest {
            body: Some(BodyEdit::Replace {
                text: "replaced".into(),
            }),
            ..ModifyRequest::default()
        };

        let (request, _) = request(
            outgoing("/a", b"the original body"),
            &[transformation(change)],
        )
        .await;

        assert_eq!(request.headers()["content-length"], "8");
        assert_eq!(body_of(request.into_body()).await, b"replaced");
    }

    #[tokio::test]
    async fn a_json_field_is_set_in_a_request_body() {
        let change = ModifyRequest {
            body: Some(BodyEdit::SetJson {
                path: "env".into(),
                value: "\"staging\"".into(),
            }),
            ..ModifyRequest::default()
        };

        let (request, _) = request(
            outgoing("/a", br#"{"env":"prod","id":7}"#),
            &[transformation(change)],
        )
        .await;

        assert_eq!(
            body_of(request.into_body()).await,
            br#"{"env":"staging","id":7}"#
        );
    }

    #[tokio::test]
    async fn later_transformations_see_what_earlier_ones_did() {
        let first = ModifyRequest {
            headers: vec![HeaderEdit::Set {
                name: "X-Env".into(),
                value: "low".into(),
            }],
            ..ModifyRequest::default()
        };
        let second = ModifyRequest {
            headers: vec![HeaderEdit::Set {
                name: "X-Env".into(),
                value: "high".into(),
            }],
            ..ModifyRequest::default()
        };

        let (request, _) = request(
            outgoing("/a", b""),
            &[transformation(first), transformation(second)],
        )
        .await;

        assert_eq!(request.headers()["x-env"], "high");
    }

    #[tokio::test]
    async fn no_transformations_is_the_request_untouched() {
        let (request, changed) = request(outgoing("/a", b"body"), &[]).await;

        assert!(!changed);
        assert_eq!(body_of(request.into_body()).await, b"body");
    }

    #[tokio::test]
    async fn a_response_is_given_its_status_headers_and_body() {
        let change = ModifyResponse {
            status: Some(404),
            headers: vec![HeaderEdit::Add {
                name: "X-Rule".into(),
                value: "1".into(),
            }],
            body: Some(BodyEdit::Replace {
                text: r#"{"error":"gone"}"#.into(),
            }),
        };

        let (response, changed) = response(
            answered(200, &[("content-encoding", "gzip")], b"\x1f\x8b gzipped"),
            &[transformation(change)],
            "GET",
        )
        .await;

        assert!(changed);
        assert_eq!(response.status(), 404);
        assert_eq!(response.headers()["x-rule"], "1");
        assert!(response.headers().get("content-encoding").is_none());
        assert_eq!(response.headers()["content-length"], "16");
        assert_eq!(body_of(response.into_body()).await, br#"{"error":"gone"}"#);
    }

    #[tokio::test]
    async fn a_compressed_body_is_not_edited_as_json() {
        let change = ModifyResponse {
            body: Some(BodyEdit::SetJson {
                path: "a".into(),
                value: "1".into(),
            }),
            ..ModifyResponse::default()
        };

        let (response, _) = response(
            answered(200, &[("content-encoding", "gzip")], b"\x1f\x8b gzipped"),
            &[transformation(change)],
            "GET",
        )
        .await;

        assert_eq!(body_of(response.into_body()).await, b"\x1f\x8b gzipped");
    }

    #[tokio::test]
    async fn a_response_that_has_no_body_is_not_given_one() {
        let change = ModifyResponse {
            body: Some(BodyEdit::Replace {
                text: "body".into(),
            }),
            ..ModifyResponse::default()
        };

        let (to_head, _) = response(
            answered(200, &[], b""),
            &[transformation(change.clone())],
            "HEAD",
        )
        .await;
        let (no_content, _) =
            response(answered(204, &[], b""), &[transformation(change)], "GET").await;

        assert!(to_head.headers().get("content-length").is_none());
        assert!(no_content.headers().get("content-length").is_none());
        assert!(body_of(no_content.into_body()).await.is_empty());
    }
}
