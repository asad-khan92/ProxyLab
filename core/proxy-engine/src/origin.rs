//! Shaping the canonical request for the protocol that will carry it.
//!
//! Everything above this module builds one shape of request: origin form,
//! HTTP/1.1, hop-by-hop headers stripped, `Host` naming the server it is for.
//! That is the proxy's canonical outgoing request, and it is what a forwarded
//! plaintext request, an intercepted HTTPS request, a request a rule sent
//! somewhere else, and a replayed request all arrive at.
//!
//! Which protocol it actually goes out on is [`crate::upstream`]'s answer,
//! not this module's. This is where the request is put into the shape that
//! answer calls for:
//!
//! ```text
//!  canonical request (HTTP/1.1 shape)
//!               │
//!      ┌────────┴────────┐
//!      ▼                 ▼
//!  HTTP/1.1          HTTP/2
//!  as it stands      Host ──▶ :authority, absolute URL
//! ```
//!
//! # Why the canonical form is the HTTP/1.1 one
//!
//! Because something has to be canonical, and this one already was. HTTP/2's
//! `:method`, `:scheme`, `:authority` and `:path` are the same four facts an
//! HTTP/1.1 request line and `Host` header carry, so translating between them
//! is lossless in both directions; picking the form every existing caller
//! already produces means HTTP/1.1 forwarding is untouched by HTTP/2
//! existing, and the translation lives in one function instead of at every
//! call site.
//!
//! # Why it translates back
//!
//! Because a request that was shaped for one connection may have to be sent
//! on another. A connection that refuses a request without writing it — a
//! pooled HTTP/2 connection that has since had a `GOAWAY`, say — hands the
//! request back, and the connection opened to replace it may speak the other
//! protocol. [`Http1Shape`] is the two facts the rewriting changed, kept so
//! they can be changed back; see [`crate::upstream::manager`], which is the
//! only place that needs them.

use hyper::header::{HeaderValue, HOST};
use hyper::http::uri::{Authority, Parts, PathAndQuery, Scheme};
use hyper::{Request, Uri, Version};

/// What [`into_http2`] changes, kept so it can be changed back.
///
/// The request target and the `Host` header, which are the same fact in two
/// spellings. Both are cheap to hold — a `Uri` and a `HeaderValue` are each a
/// handle onto bytes that already exist — so keeping them costs nothing a
/// request was not already carrying.
pub(crate) struct Http1Shape {
    uri: Uri,
    host: Option<HeaderValue>,
}

impl Http1Shape {
    /// Puts `request` back into the shape it had before it was rewritten.
    pub(crate) fn restore<B>(self, request: Request<B>) -> Request<B> {
        let (mut parts, body) = request.into_parts();

        parts.uri = self.uri;
        parts.version = Version::HTTP_11;
        match self.host {
            Some(host) => {
                parts.headers.insert(HOST, host);
            }
            None => {
                parts.headers.remove(HOST);
            }
        }

        Request::from_parts(parts, body)
    }
}

/// Remembers the canonical shape of `request`, before it is rewritten.
pub(crate) fn http1_shape<B>(request: &Request<B>) -> Http1Shape {
    Http1Shape {
        uri: request.uri().clone(),
        host: request.headers().get(HOST).cloned(),
    }
}

/// Rewrites the canonical request into the HTTP/2 one.
///
/// Three things change, and they are the same three facts said a different
/// way.
///
/// The request target becomes an absolute URL, because that is what the HTTP
/// library reads `:scheme`, `:authority` and `:path` off. The authority is the
/// request's own `Host` where it has one — which is where an intercepted
/// client's `:authority`, or an HTTP/1.1 client's `Host`, has already been
/// put — the target's own authority next, and `authority` when the request
/// names no host at all.
///
/// `Host` is then removed, because `:authority` is where that fact now lives
/// and sending it twice is how the two come to disagree.
///
/// The version is corrected, so the message says what it is.
///
/// Nothing else is touched. The headers have already been through the proxy's
/// hop-by-hop stripping, which removes exactly the headers HTTP/2 forbids, so
/// there is no second filter here — and the HTTP library strips them again on
/// the way out regardless.
pub(crate) fn into_http2<B>(
    request: Request<B>,
    scheme: &Scheme,
    authority: &Authority,
) -> Request<B> {
    let (mut parts, body) = request.into_parts();

    let named = host_authority(&parts.headers)
        .or_else(|| parts.uri.authority().cloned())
        .unwrap_or_else(|| authority.clone());

    let mut target = Parts::default();
    target.scheme = Some(scheme.clone());
    target.authority = Some(named);
    target.path_and_query = Some(
        parts
            .uri
            .path_and_query()
            .filter(|path| !path.as_str().is_empty())
            .cloned()
            .unwrap_or_else(|| PathAndQuery::from_static("/")),
    );

    // Assembled from three pieces that are each already valid, so this cannot
    // fail; a build where it somehow does sends the target it had rather than
    // panicking on the request path.
    if let Ok(absolute) = Uri::from_parts(target) {
        parts.uri = absolute;
    }
    parts.headers.remove(HOST);
    parts.version = Version::HTTP_2;

    Request::from_parts(parts, body)
}

/// The `Host` header as an authority, if it is one.
fn host_authority(headers: &hyper::header::HeaderMap) -> Option<Authority> {
    headers
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse().ok())
}

/// The scheme an intercepted or otherwise TLS-protected origin is reached on.
pub fn https() -> Scheme {
    Scheme::HTTPS
}

/// The authority `host` and `port` name, for the requests that do not say.
///
/// `None` for a host that cannot be written as one, which is a destination
/// that has been validated as a host and still cannot be named in a URL.
pub fn authority(host: &str, port: u16) -> Option<Authority> {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };

    format!("{host}:{port}").parse().ok()
}

/// The authority a `Host` header value names, if it names one.
pub fn authority_of(host: &HeaderValue) -> Option<Authority> {
    host.to_str().ok().and_then(|host| host.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::header::HeaderMap;

    /// The scheme and authority an origin connection was opened for.
    ///
    /// Every test here is about the rewriting, which happens before anything
    /// is sent, so a sender would only be in the way.
    fn reaching(authority: &str) -> (Scheme, Authority) {
        (Scheme::HTTPS, authority.parse().expect("valid authority"))
    }

    /// `request` as it would be sent on an HTTP/2 connection to `authority`.
    fn rewritten(authority: &str, request: Request<Empty<Bytes>>) -> Request<Empty<Bytes>> {
        let (scheme, authority) = reaching(authority);

        into_http2(request, &scheme, &authority)
    }

    fn request(target: &str, headers: &[(&str, &str)]) -> Request<Empty<Bytes>> {
        let mut request = Request::builder()
            .uri(target)
            .version(Version::HTTP_11)
            .body(Empty::new())
            .expect("valid request");

        for (name, value) in headers {
            request.headers_mut().insert(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }

        request
    }

    #[test]
    fn the_host_header_becomes_the_authority_of_an_absolute_url() {
        let sent = rewritten(
            "fallback.test:443",
            request("/api/users?active=true", &[("host", "example.com")]),
        );

        assert_eq!(sent.uri(), "https://example.com/api/users?active=true");
    }

    #[test]
    fn the_host_header_does_not_travel_alongside_the_authority() {
        let sent = rewritten(
            "fallback.test:443",
            request("/x", &[("host", "example.com")]),
        );

        assert!(sent.headers().get(HOST).is_none());
    }

    #[test]
    fn a_host_that_names_a_port_keeps_it() {
        let sent = rewritten(
            "fallback.test:443",
            request("/x", &[("host", "example.com:8443")]),
        );

        assert_eq!(sent.uri(), "https://example.com:8443/x");
    }

    #[test]
    fn a_request_that_names_no_host_is_sent_to_the_one_this_connection_reaches() {
        let sent = rewritten("example.com:443", request("/x", &[]));

        assert_eq!(sent.uri(), "https://example.com:443/x");
    }

    #[test]
    fn an_authority_already_on_the_target_is_used_when_there_is_no_host_header() {
        let sent = rewritten("fallback.test:443", request("https://named.example/x", &[]));

        assert_eq!(sent.uri(), "https://named.example/x");
    }

    #[test]
    fn a_request_that_names_no_path_asks_for_the_root() {
        let sent = rewritten("example.com:443", request("/", &[]));

        assert_eq!(sent.uri().path(), "/");
    }

    #[test]
    fn the_rewritten_request_says_it_is_http_2() {
        let sent = rewritten("example.com:443", request("/x", &[]));

        assert_eq!(sent.version(), Version::HTTP_2);
    }

    #[test]
    fn the_method_and_the_end_to_end_headers_are_the_requests_own() {
        let mut built = request(
            "/orders",
            &[
                ("host", "example.com"),
                ("authorization", "Bearer token"),
                ("content-type", "application/json"),
            ],
        );
        *built.method_mut() = hyper::Method::POST;

        let sent = rewritten("fallback.test:443", built);

        assert_eq!(sent.method(), hyper::Method::POST);
        assert_eq!(sent.headers()["authorization"], "Bearer token");
        assert_eq!(sent.headers()["content-type"], "application/json");
    }

    #[test]
    fn a_host_header_that_is_not_an_authority_falls_back_to_the_connection() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("not a host"));

        assert!(host_authority(&headers).is_none());
    }

    #[test]
    fn an_authority_is_built_from_a_host_and_a_port() {
        assert_eq!(
            authority("example.com", 443).map(|a| a.to_string()),
            Some("example.com:443".to_owned())
        );
    }

    #[test]
    fn an_ipv6_authority_is_bracketed_exactly_once() {
        assert_eq!(
            authority("::1", 8443).map(|a| a.to_string()),
            Some("[::1]:8443".to_owned())
        );
        assert_eq!(
            authority("[::1]", 8443).map(|a| a.to_string()),
            Some("[::1]:8443".to_owned())
        );
    }

    #[test]
    fn something_that_is_not_a_host_is_not_an_authority() {
        assert_eq!(authority("not a host", 443), None);
    }

    #[test]
    fn a_host_header_reads_back_as_the_authority_it_names() {
        assert_eq!(
            authority_of(&HeaderValue::from_static("example.com:8080")).map(|a| a.to_string()),
            Some("example.com:8080".to_owned())
        );
        assert_eq!(authority_of(&HeaderValue::from_static("not a host")), None);
    }

    #[test]
    fn a_request_rewritten_for_http_2_can_be_put_back_as_it_was() {
        let original = request("/api/users?active=true", &[("host", "example.com")]);
        let shape = http1_shape(&original);

        let restored = shape.restore(rewritten("fallback.test:443", original));

        assert_eq!(restored.uri(), "/api/users?active=true");
        assert_eq!(restored.headers()[HOST], "example.com");
        assert_eq!(restored.version(), Version::HTTP_11);
    }

    #[test]
    fn a_request_that_had_no_host_gets_none_back() {
        let original = request("/x", &[]);
        let shape = http1_shape(&original);

        let restored = shape.restore(rewritten("example.com:443", original));

        assert_eq!(restored.uri(), "/x");
        assert!(restored.headers().get(HOST).is_none());
    }

    #[test]
    fn a_host_the_request_could_not_name_falls_back_to_the_connections_own() {
        let mut built = request("/x", &[]);
        built.headers_mut().insert(
            HOST,
            HeaderValue::from_bytes(b"not a host").expect("a header value"),
        );

        let sent = rewritten("example.com:443", built);

        assert_eq!(sent.uri(), "https://example.com:443/x");
    }
}
