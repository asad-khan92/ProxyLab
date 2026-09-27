//! Turning hyper's messages into ProxyLab's traffic model.
//!
//! One direction only: hyper types come in, model types go out. Nothing in
//! `proxy-storage` knows what an HTTP library is, and this module is why it
//! does not have to.

use hyper::header::HeaderMap;
use hyper::{Request, Response, Uri, Version};
use proxy_storage::{
    BodyCapture, CapturedRequest, CapturedResponse, Destination, Header, Headers, HttpVersion,
};

/// The port an `http` URL does not need to name.
const DEFAULT_HTTP_PORT: u16 = 80;

/// The port an `https` URL does not need to name.
const DEFAULT_HTTPS_PORT: u16 = 443;

/// The captured form of a header section.
///
/// Every header is kept, in the order it arrived, with repeated names
/// repeated: a message that sent `Set-Cookie` three times captures three
/// headers.
pub fn headers(map: &HeaderMap) -> Headers {
    map.iter()
        .map(|(name, value)| Header::from_bytes(name.as_str(), value.as_bytes()))
        .collect()
}

/// The captured form of a protocol version.
pub fn version(version: Version) -> HttpVersion {
    match version {
        Version::HTTP_09 => HttpVersion::Http09,
        Version::HTTP_10 => HttpVersion::Http10,
        Version::HTTP_11 => HttpVersion::Http11,
        Version::HTTP_2 => HttpVersion::Http2,
        Version::HTTP_3 => HttpVersion::Http3,
        _ => HttpVersion::Unknown,
    }
}

/// The captured head of a request, with its body still to come.
///
/// `url` is supplied rather than read off the request because the two routes
/// through the proxy spell it differently: a forwarded request carries an
/// absolute URL, and an intercepted HTTPS request carries only a path, whose
/// host is the one its `CONNECT` named.
pub fn request_head<B>(request: &Request<B>, url: String) -> CapturedRequest {
    CapturedRequest {
        method: request.method().as_str().to_owned(),
        url,
        version: version(request.version()),
        headers: headers(request.headers()),
        body: BodyCapture::empty(),
    }
}

/// The captured head of a response, with its body still to come.
pub fn response_head<B>(response: &Response<B>) -> CapturedResponse {
    CapturedResponse {
        status: response.status().as_u16(),
        reason: reason(response),
        version: version(response.version()),
        headers: headers(response.headers()),
        body: BodyCapture::empty(),
    }
}

/// The reason phrase a response is best described by.
///
/// The phrase the origin actually wrote, when the HTTP library kept it, and
/// otherwise the standard phrase for the status code. A status code with no
/// standard phrase and no phrase on the wire has none.
fn reason<B>(response: &Response<B>) -> Option<String> {
    response
        .extensions()
        .get::<hyper::ext::ReasonPhrase>()
        .and_then(|phrase| std::str::from_utf8(phrase.as_bytes()).ok())
        .map(str::to_owned)
        .or_else(|| response.status().canonical_reason().map(str::to_owned))
}

/// The URL a forwarded request was for, in the form it is safe to keep.
///
/// Rebuilt from the request target rather than copied from it, so that
/// userinfo — `http://user:password@example.com/` — is left behind. A client
/// should not put credentials there, and a capture that is going to be shown,
/// exported, and stored must not repeat them if it does.
///
/// A target that is not an absolute URL has no host to rebuild from and is
/// kept as it was sent; the proxy refuses such a request anyway, and the
/// capture should say what was asked for.
pub fn forwarded_url(uri: &Uri) -> String {
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return uri.to_string();
    };

    let mut url = format!("{scheme}://{}", authority.host());
    if let Some(port) = authority.port_u16() {
        url.push_str(&format!(":{port}"));
    }
    url.push_str(path_and_query(uri));

    url
}

/// The path and query of a request target, or `/` when it names none.
pub fn path_and_query(uri: &Uri) -> &str {
    uri.path_and_query()
        .map(|path| path.as_str())
        .filter(|path| !path.is_empty())
        .unwrap_or("/")
}

/// The server an absolute-form request target names.
///
/// `None` when the target names no host, or names one on a scheme with no
/// port to assume — which is a request the proxy is about to refuse.
pub fn destination(uri: &Uri) -> Option<Destination> {
    let authority = uri.authority()?;
    let host = authority.host();
    if host.is_empty() {
        return None;
    }

    // The brackets around an IPv6 literal belong to the URL, not to the host.
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);

    let port = authority.port_u16().or_else(|| match uri.scheme_str() {
        Some(scheme) if scheme.eq_ignore_ascii_case("http") => Some(DEFAULT_HTTP_PORT),
        Some(scheme) if scheme.eq_ignore_ascii_case("https") => Some(DEFAULT_HTTPS_PORT),
        _ => None,
    })?;

    Some(Destination::new(host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    use hyper::header::{HeaderValue, SET_COOKIE};
    use hyper::StatusCode;
    use proxy_storage::HeaderValue as CapturedHeaderValue;

    fn uri(target: &str) -> Uri {
        target.parse().expect("valid uri")
    }

    #[test]
    fn every_header_is_captured_in_the_order_it_arrived() {
        let mut map = HeaderMap::new();
        map.append(SET_COOKIE, HeaderValue::from_static("a=1"));
        map.append(SET_COOKIE, HeaderValue::from_static("b=2"));
        map.insert("accept", HeaderValue::from_static("*/*"));

        let captured = headers(&map);

        assert_eq!(captured.len(), 3);
        let cookies: Vec<Option<&str>> = captured
            .get_all("set-cookie")
            .map(CapturedHeaderValue::as_text)
            .collect();
        assert_eq!(cookies, [Some("a=1"), Some("b=2")]);
        assert_eq!(captured.text("accept"), Some("*/*"));
    }

    #[test]
    fn a_header_value_that_is_not_text_is_captured_as_bytes() {
        let mut map = HeaderMap::new();
        map.insert(
            "x-raw",
            HeaderValue::from_bytes(&[0xff, 0xfe]).expect("valid header value"),
        );

        let captured = headers(&map);

        assert_eq!(
            captured.get("x-raw"),
            Some(&CapturedHeaderValue::Bytes(vec![0xff, 0xfe]))
        );
    }

    #[test]
    fn every_version_has_a_captured_form() {
        assert_eq!(version(Version::HTTP_10), HttpVersion::Http10);
        assert_eq!(version(Version::HTTP_11), HttpVersion::Http11);
        assert_eq!(version(Version::HTTP_2), HttpVersion::Http2);
        assert_eq!(version(Version::HTTP_3), HttpVersion::Http3);
        assert_eq!(version(Version::HTTP_09), HttpVersion::Http09);
    }

    #[test]
    fn a_request_head_carries_its_method_url_version_and_headers() {
        let request = Request::builder()
            .method("POST")
            .uri("http://example.com/api")
            .header("content-type", "application/json")
            .body(())
            .expect("valid request");

        let captured = request_head(&request, "http://example.com/api".to_owned());

        assert_eq!(captured.method, "POST");
        assert_eq!(captured.url, "http://example.com/api");
        assert_eq!(captured.version, HttpVersion::Http11);
        assert_eq!(
            captured.headers.text("content-type"),
            Some("application/json")
        );
        assert!(captured.body.is_empty());
    }

    #[test]
    fn a_response_head_carries_its_status_reason_and_headers() {
        let response = Response::builder()
            .status(StatusCode::CREATED)
            .header("location", "/users/1")
            .body(())
            .expect("valid response");

        let captured = response_head(&response);

        assert_eq!(captured.status, 201);
        assert_eq!(captured.reason.as_deref(), Some("Created"));
        assert_eq!(captured.version, HttpVersion::Http11);
        assert_eq!(captured.headers.text("location"), Some("/users/1"));
        assert!(captured.body.is_empty());
    }

    #[test]
    fn a_reason_phrase_the_origin_wrote_is_kept_over_the_standard_one() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(())
            .expect("valid response");
        response
            .extensions_mut()
            .insert(hyper::ext::ReasonPhrase::from_static(b"Splendid"));

        assert_eq!(response_head(&response).reason.as_deref(), Some("Splendid"));
    }

    #[test]
    fn a_status_code_with_no_standard_phrase_has_no_reason() {
        let response = Response::builder()
            .status(599)
            .body(())
            .expect("valid response");

        assert_eq!(response_head(&response).reason, None);
    }

    #[test]
    fn a_forwarded_url_keeps_the_host_port_path_and_query() {
        assert_eq!(
            forwarded_url(&uri("http://example.com:8080/api/users?active=true")),
            "http://example.com:8080/api/users?active=true"
        );
    }

    #[test]
    fn a_forwarded_url_leaves_out_a_port_the_client_left_out() {
        assert_eq!(
            forwarded_url(&uri("http://example.com/x")),
            "http://example.com/x"
        );
    }

    #[test]
    fn a_forwarded_url_asks_for_the_root_when_the_target_names_no_path() {
        assert_eq!(
            forwarded_url(&uri("http://example.com")),
            "http://example.com/"
        );
    }

    #[test]
    fn a_forwarded_url_does_not_repeat_credentials_from_the_request_target() {
        let url = forwarded_url(&uri("http://alice:hunter2@example.com/x"));

        assert!(!url.contains("hunter2"), "captured a password: {url}");
        assert!(!url.contains("alice"), "captured a username: {url}");
        assert_eq!(url, "http://example.com/x");
    }

    #[test]
    fn a_forwarded_url_brackets_an_ipv6_host() {
        assert_eq!(
            forwarded_url(&uri("http://[::1]:8080/x")),
            "http://[::1]:8080/x"
        );
    }

    #[test]
    fn a_target_that_is_not_an_absolute_url_is_captured_as_it_was_sent() {
        assert_eq!(forwarded_url(&uri("/index.html")), "/index.html");
    }

    #[test]
    fn a_destination_takes_the_port_the_scheme_implies() {
        assert_eq!(
            destination(&uri("http://example.com/x")),
            Some(Destination::new("example.com", 80))
        );
        assert_eq!(
            destination(&uri("https://example.com/x")),
            Some(Destination::new("example.com", 443))
        );
    }

    #[test]
    fn a_destination_prefers_the_port_the_client_named() {
        assert_eq!(
            destination(&uri("http://example.com:8080/x")),
            Some(Destination::new("example.com", 8080))
        );
    }

    #[test]
    fn a_destination_drops_the_brackets_of_an_ipv6_literal() {
        assert_eq!(
            destination(&uri("http://[::1]:8080/x")),
            Some(Destination::new("::1", 8080))
        );
    }

    #[test]
    fn a_target_with_no_host_has_no_destination() {
        assert_eq!(destination(&uri("/index.html")), None);
    }

    #[test]
    fn a_scheme_with_no_port_to_assume_has_no_destination() {
        assert_eq!(destination(&uri("ftp://example.com/x")), None);
    }
}
