//! One HTTP request, described by its parts and checked once.
//!
//! ```text
//! RequestSpec
//! ├── method    a token, as HTTP spells one
//! ├── url       absolute, http or https, with a host and no credentials
//! ├── headers   ordered, repeatable, binary-safe
//! └── body      bytes, and not necessarily text
//! ```
//!
//! The parts are private and [`RequestSpec::new`] is the only way to build
//! one, so holding a `RequestSpec` is holding a request that has already been
//! checked: the URL names a host on a scheme this proxy speaks, the method is
//! a token, and no header value carries the newline that would let a second
//! request be smuggled inside the first.
//!
//! [`RequestParts`] is the same four fields with none of that promised. It is
//! what arrives from outside — a JSON object on the IPC socket — and it turns
//! into a `RequestSpec` or into an error that says which part was wrong.

use std::fmt;

use http::uri::{Authority, Uri};
use http::Method;
use proxy_storage::{Destination, HeaderValue, Headers, SavedRequest, Scheme};
use serde::{Deserialize, Serialize};

/// The longest URL a request may name.
///
/// Longer than any URL a server will accept — the common server limit is
/// 8 KiB for the whole request line — and short enough that a URL is never
/// somewhere to put a payload.
pub const MAX_URL_LENGTH: usize = 8 * 1024;

/// The longest method name a request may use.
///
/// Every method anyone has registered is well under this; the limit is here
/// so that a method is a method rather than a place to put something else.
pub const MAX_METHOD_LENGTH: usize = 64;

/// The most headers one request may carry.
pub const MAX_HEADERS: usize = 200;

/// The longest header name a request may use.
pub const MAX_HEADER_NAME_LENGTH: usize = 256;

/// Port used when an `http` URL does not name one.
const DEFAULT_HTTP_PORT: u16 = 80;

/// Port used when an `https` URL does not name one.
const DEFAULT_HTTPS_PORT: u16 = 443;

/// A request as it arrives from outside, before anything has been checked.
///
/// The wire form of [`RequestSpec`], and the shape a client sends: the four
/// fields, with the body as base64 because JSON has no way to carry a byte
/// string.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestParts {
    /// The method, as it will be sent.
    ///
    /// Defaulted rather than required so that a message which leaves it out
    /// is refused for *not naming a method* — which is a thing a person can
    /// fix — rather than as a malformed object.
    #[serde(default)]
    pub method: String,
    /// The absolute URL to send it to.
    ///
    /// Defaulted for the same reason as the method.
    #[serde(default)]
    pub url: String,
    /// The headers, in the order they should be sent.
    #[serde(default)]
    pub headers: Headers,
    /// The body, which is bytes and need not be text.
    #[serde(default, with = "proxy_storage::base64_bytes")]
    pub body: Vec<u8>,
}

/// One request, ready to be sent.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestSpec {
    method: String,
    /// The URL, normalized: the scheme and host lowercased, a port kept only
    /// when it was named, and a path of `/` when none was given.
    url: String,
    scheme: Scheme,
    /// The host, without the brackets an IPv6 literal wears in a URL.
    host: String,
    port: u16,
    /// The request target an origin server expects: path and query only.
    path_and_query: String,
    headers: Headers,
    body: Vec<u8>,
}

impl RequestSpec {
    /// Checks `method`, `url`, `headers`, and `body`, and builds the request
    /// they describe.
    pub fn new(
        method: impl Into<String>,
        url: &str,
        headers: Headers,
        body: Vec<u8>,
    ) -> Result<Self, RequestSpecError> {
        let method = checked_method(method.into())?;
        let url = Url::parse(url)?;
        checked_headers(&headers)?;

        Ok(Self {
            method,
            url: url.text,
            scheme: url.scheme,
            host: url.host,
            port: url.port,
            path_and_query: url.path_and_query,
            headers,
            body,
        })
    }

    /// The same, from the four fields as they arrived.
    pub fn from_parts(parts: RequestParts) -> Result<Self, RequestSpecError> {
        Self::new(parts.method, &parts.url, parts.headers, parts.body)
    }

    /// The request a saved one describes.
    ///
    /// `proxy-storage` persists a saved request and this crate decides what a
    /// request may be, so this is where the two meet: the stored fields are
    /// checked here exactly as a replayed request's are, and a saved request
    /// that cannot become one is refused for the same reason and with the
    /// same words.
    ///
    /// The body is what was saved, which is as much of it as there is: a
    /// request kept from a capture the proxy cut short sends the captured
    /// bytes, because the rest was forwarded and never kept. Whether that
    /// happened is [`BodyCapture::is_truncated`](proxy_storage::BodyCapture::is_truncated),
    /// which the saved request still carries.
    pub fn from_saved(saved: &SavedRequest) -> Result<Self, RequestSpecError> {
        Self::new(
            saved.method.clone(),
            &saved.url,
            saved.headers.clone(),
            saved.body.data().to_vec(),
        )
    }

    /// A `GET` of `url`, with no headers and no body.
    pub fn get(url: &str) -> Result<Self, RequestSpecError> {
        Self::new("GET", url, Headers::new(), Vec::new())
    }

    /// The method, as it will be sent.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The URL, normalized.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether the request is plaintext or goes inside a TLS session.
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The host to connect to, without IPv6 brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port to connect to, filled in from the scheme when the URL named
    /// none.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The request target an origin server expects: path and query only.
    pub fn path_and_query(&self) -> &str {
        &self.path_and_query
    }

    /// The path, without the query.
    pub fn path(&self) -> &str {
        match self.path_and_query.split_once('?') {
            Some((path, _)) => path,
            None => &self.path_and_query,
        }
    }

    /// The `Host` header value this URL implies.
    ///
    /// The authority without its port when the port is the scheme's own, so
    /// an origin sees the `Host` it would have been sent by any other client.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        if self.port == self.scheme_default_port() {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// Where the request is going, as a captured exchange records it.
    pub fn destination(&self) -> Destination {
        Destination::new(self.host.clone(), self.port)
    }

    /// The headers, in the order they will be sent.
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// The body, which is bytes and need not be text.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body, taken rather than copied.
    ///
    /// What sending the request does with it: a body is moved into the
    /// outgoing message rather than cloned on the way.
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }

    /// The four fields, for sending across the IPC boundary.
    pub fn to_parts(&self) -> RequestParts {
        RequestParts {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body: self.body.clone(),
        }
    }

    /// The port the scheme implies.
    fn scheme_default_port(&self) -> u16 {
        match self.scheme {
            Scheme::Http => DEFAULT_HTTP_PORT,
            Scheme::Https => DEFAULT_HTTPS_PORT,
        }
    }
}

/// Metadata only: the method and the URL, and counts and sizes for the rest.
///
/// A URL is already written to logs and shown in a traffic list — and one
/// carrying credentials is refused rather than held — so it is named here.
/// A header value and a body are not, ever. See the crate documentation.
impl fmt::Debug for RequestSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestSpec")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers.len())
            .field("body", &self.body.len())
            .finish()
    }
}

/// The four fields, as [`RequestParts`] spells them.
impl Serialize for RequestSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_parts().serialize(serializer)
    }
}

/// Read back through the same checks a new request goes through.
impl<'de> Deserialize<'de> for RequestSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let parts = RequestParts::deserialize(deserializer)?;

        Self::from_parts(parts).map_err(|error| serde::de::Error::custom(error.reason()))
    }
}

/// Why a request is not one this proxy can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestSpecError {
    /// The method was empty.
    MissingMethod,
    /// The method is not an HTTP method token.
    InvalidMethod,
    /// The URL was empty.
    MissingUrl,
    /// The URL is longer than a URL may be.
    UrlTooLong,
    /// The URL could not be read as a URL at all.
    InvalidUrl,
    /// The URL named no scheme, so there is nothing to say how to reach it.
    RelativeUrl,
    /// The URL names a scheme this proxy does not speak.
    UnsupportedScheme,
    /// The URL names no host.
    MissingHost,
    /// The URL carries credentials in its authority.
    CredentialsInUrl,
    /// A header name is not an HTTP field name.
    InvalidHeaderName,
    /// A header value carries bytes a header value may not.
    InvalidHeaderValue,
    /// The request carries more headers than one request may.
    TooManyHeaders,
    /// The request authenticates to an upstream proxy, which ProxyLab has
    /// none of.
    ProxyAuthentication,
}

impl RequestSpecError {
    /// What a person is told. A fixed phrase, naming nothing that was sent.
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingMethod => "a request must name a method",
            Self::InvalidMethod => "that is not an HTTP method",
            Self::MissingUrl => "a request must name a URL",
            Self::UrlTooLong => "that URL is longer than a URL may be",
            Self::InvalidUrl => "that is not a URL",
            Self::RelativeUrl => {
                "a request URL must be absolute, starting with http:// or https://"
            }
            Self::UnsupportedScheme => "ProxyLab sends http:// and https:// requests",
            Self::MissingHost => "that URL names no host",
            Self::CredentialsInUrl => {
                "put credentials in an Authorization header rather than in the URL"
            }
            Self::InvalidHeaderName => "that is not a header name",
            Self::InvalidHeaderValue => "a header value cannot contain a line break",
            Self::TooManyHeaders => "a request may carry at most 200 headers",
            Self::ProxyAuthentication => {
                "ProxyLab does not send requests through an upstream proxy, \
                 so it cannot authenticate to one"
            }
        }
    }
}

impl fmt::Display for RequestSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for RequestSpecError {}

/// A URL that has been read apart.
struct Url {
    text: String,
    scheme: Scheme,
    host: String,
    port: u16,
    path_and_query: String,
}

impl Url {
    /// Reads `url` and checks everything a request URL has to be.
    fn parse(url: &str) -> Result<Self, RequestSpecError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(RequestSpecError::MissingUrl);
        }
        if url.len() > MAX_URL_LENGTH {
            return Err(RequestSpecError::UrlTooLong);
        }

        // A fragment is never sent: it is the client's own, and no HTTP
        // request has ever carried one. Dropped here rather than refused,
        // because a URL copied out of a browser routinely has one.
        let without_fragment = match url.split_once('#') {
            Some((before, _)) => before,
            None => url,
        };

        // The scheme and the authority are looked at before the URL is
        // parsed, so that each refusal is the specific one. `http::Uri`
        // reports "invalid format" for `file:///etc/passwd` and for
        // `http:///path` alike, and "a scheme this proxy does not speak" and
        // "that URL names no host" are two quite different things to fix.
        let Some((scheme, rest)) = without_fragment.split_once("://") else {
            return Err(RequestSpecError::RelativeUrl);
        };
        let scheme = if scheme.eq_ignore_ascii_case("http") {
            Scheme::Http
        } else if scheme.eq_ignore_ascii_case("https") {
            Scheme::Https
        } else {
            return Err(RequestSpecError::UnsupportedScheme);
        };
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') {
            return Err(RequestSpecError::MissingHost);
        }

        let uri: Uri = without_fragment
            .parse()
            .map_err(|_| RequestSpecError::InvalidUrl)?;

        let authority = uri.authority().ok_or(RequestSpecError::MissingHost)?;
        // `Authority::host` drops userinfo silently, which is exactly the
        // silence this refusal exists to prevent: a URL that carried a
        // password would otherwise be stored and logged with the password
        // invisibly gone, and sent with no credential at all.
        if authority.as_str().contains('@') {
            return Err(RequestSpecError::CredentialsInUrl);
        }

        let bracketed = authority.host();
        if bracketed.is_empty() {
            return Err(RequestSpecError::MissingHost);
        }

        // DNS names are case-insensitive, so the host is lowercased: two
        // spellings of one host are then one URL, and the text a request is
        // stored and shown under does not depend on how it was typed.
        let bracketed = bracketed.to_ascii_lowercase();
        let host = bracketed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(&bracketed)
            .to_owned();

        let named_port = authority.port_u16();
        let port = named_port.unwrap_or(match scheme {
            Scheme::Http => DEFAULT_HTTP_PORT,
            Scheme::Https => DEFAULT_HTTPS_PORT,
        });

        let path_and_query = uri
            .path_and_query()
            .map(|path| path.as_str())
            .filter(|path| !path.is_empty())
            .unwrap_or("/")
            .to_owned();

        // Rebuilt rather than echoed, so that one URL has one spelling
        // whatever was typed — and so that nothing the authority carried
        // beyond a host and a port survives into the text that is stored.
        let text = match named_port {
            Some(port) => format!("{scheme}://{bracketed}:{port}{path_and_query}"),
            None => format!("{scheme}://{bracketed}{path_and_query}"),
        };

        // Belt and braces: `Uri` has already refused the characters that
        // would let a request line be split, and this says so where a reader
        // can see it.
        debug_assert!(!text.contains(['\r', '\n', ' ']));

        Ok(Self {
            text,
            scheme,
            host,
            port,
            path_and_query,
        })
    }
}

/// Checks a method and gives it back.
///
/// Case is kept: HTTP methods are case-sensitive, so `get` is not `GET` and
/// sending one when the other was asked for would be sending a different
/// request.
fn checked_method(method: String) -> Result<String, RequestSpecError> {
    if method.is_empty() {
        return Err(RequestSpecError::MissingMethod);
    }
    if method.len() > MAX_METHOD_LENGTH {
        return Err(RequestSpecError::InvalidMethod);
    }
    // `Method::from_bytes` is the same check hyper makes when the request is
    // put on the wire, so a method accepted here is one that can be sent.
    Method::from_bytes(method.as_bytes()).map_err(|_| RequestSpecError::InvalidMethod)?;

    Ok(method)
}

/// Checks every header.
fn checked_headers(headers: &Headers) -> Result<(), RequestSpecError> {
    if headers.len() > MAX_HEADERS {
        return Err(RequestSpecError::TooManyHeaders);
    }

    for header in headers {
        // Authentication to an upstream proxy, which ProxyLab does not have
        // and does not chain to. Refused rather than dropped on the way out,
        // because a credential that silently stops being sent is a request
        // that quietly became a different one.
        if header.name.eq_ignore_ascii_case("proxy-authorization") {
            return Err(RequestSpecError::ProxyAuthentication);
        }
        checked_header_name(&header.name)?;
        checked_header_value(&header.value)?;
    }

    Ok(())
}

/// Whether a name is an HTTP field name.
fn checked_header_name(name: &str) -> Result<(), RequestSpecError> {
    if name.is_empty() || name.len() > MAX_HEADER_NAME_LENGTH {
        return Err(RequestSpecError::InvalidHeaderName);
    }
    if !name.bytes().all(is_token_byte) {
        return Err(RequestSpecError::InvalidHeaderName);
    }

    Ok(())
}

/// Whether a value is one a header may carry.
///
/// The bytes that matter are `CR`, `LF`, and `NUL`: a value carrying any of
/// them could end the header section early and put a request of somebody
/// else's choosing after it. The other control characters are refused with
/// them, because a header value is text or it is a byte string, and neither
/// has a use for `0x01`.
fn checked_header_value(value: &HeaderValue) -> Result<(), RequestSpecError> {
    let usable = value
        .as_bytes()
        .iter()
        .all(|byte| *byte == b'\t' || (0x20..0x7f).contains(byte) || *byte >= 0x80);

    if usable {
        Ok(())
    } else {
        Err(RequestSpecError::InvalidHeaderValue)
    }
}

/// Whether a byte is one of RFC 9110's `tchar`.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// The authority a URL names, for whoever needs it spelled as a URL does.
///
/// Not part of the model: it exists so that a caller rebuilding a URL from
/// the parts spells the host the way this module did.
pub fn authority_of(host: &str, port: u16) -> Option<Authority> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    authority.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(entries: &[(&str, &str)]) -> Headers {
        entries
            .iter()
            .map(|(name, value)| proxy_storage::Header::text(*name, *value))
            .collect()
    }

    fn spec(url: &str) -> Result<RequestSpec, RequestSpecError> {
        RequestSpec::get(url)
    }

    #[test]
    fn a_simple_get_is_read_apart() {
        let request = spec("https://example.com/api/users?active=true").expect("a valid request");

        assert_eq!(request.method(), "GET");
        assert_eq!(request.url(), "https://example.com/api/users?active=true");
        assert_eq!(request.scheme(), Scheme::Https);
        assert_eq!(request.host(), "example.com");
        assert_eq!(request.port(), 443);
        assert_eq!(request.path_and_query(), "/api/users?active=true");
        assert_eq!(request.path(), "/api/users");
        assert_eq!(request.host_header(), "example.com");
        assert_eq!(request.destination(), Destination::new("example.com", 443));
    }

    #[test]
    fn a_url_with_no_path_gets_one() {
        let request = spec("http://example.com").expect("a valid request");

        assert_eq!(request.url(), "http://example.com/");
        assert_eq!(request.path_and_query(), "/");
        assert_eq!(request.port(), 80);
    }

    #[test]
    fn a_named_port_is_kept_and_reaches_the_host_header() {
        let request = spec("http://example.com:8080/api").expect("a valid request");

        assert_eq!(request.url(), "http://example.com:8080/api");
        assert_eq!(request.port(), 8080);
        assert_eq!(request.host_header(), "example.com:8080");
    }

    #[test]
    fn the_scheme_and_host_are_lowercased() {
        let request = spec("HTTPS://Example.COM/Path").expect("a valid request");

        // The path keeps its case: a URL path is case-sensitive and a host
        // is not.
        assert_eq!(request.url(), "https://example.com/Path");
        assert_eq!(request.host(), "example.com");
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets_in_the_url_and_loses_them_elsewhere() {
        let request = spec("http://[::1]:8080/health").expect("a valid request");

        assert_eq!(request.url(), "http://[::1]:8080/health");
        assert_eq!(request.host(), "::1");
        assert_eq!(request.host_header(), "[::1]:8080");
        assert_eq!(request.destination(), Destination::new("::1", 8080));
    }

    #[test]
    fn a_fragment_is_dropped_because_no_request_carries_one() {
        let request = spec("https://example.com/docs?a=1#section").expect("a valid request");

        assert_eq!(request.url(), "https://example.com/docs?a=1");
    }

    #[test]
    fn a_relative_url_is_refused() {
        assert_eq!(spec("/api/users"), Err(RequestSpecError::RelativeUrl));
        assert_eq!(spec("example.com/api"), Err(RequestSpecError::RelativeUrl));
        assert_eq!(
            spec("mailto:ada@example.com"),
            Err(RequestSpecError::RelativeUrl)
        );
    }

    #[test]
    fn a_scheme_this_proxy_does_not_speak_is_refused() {
        assert_eq!(
            spec("ftp://example.com/file"),
            Err(RequestSpecError::UnsupportedScheme)
        );
        assert_eq!(
            spec("file:///etc/passwd"),
            Err(RequestSpecError::UnsupportedScheme)
        );
    }

    #[test]
    fn a_url_with_no_host_is_refused() {
        assert_eq!(spec("http:///path"), Err(RequestSpecError::MissingHost));
    }

    #[test]
    fn an_empty_url_is_refused() {
        assert_eq!(spec(""), Err(RequestSpecError::MissingUrl));
        assert_eq!(spec("   "), Err(RequestSpecError::MissingUrl));
    }

    #[test]
    fn a_url_that_is_not_a_url_is_refused() {
        assert_eq!(
            spec("http://exa mple.com/"),
            Err(RequestSpecError::InvalidUrl)
        );
        assert_eq!(
            spec("https://exam\u{7f}ple.com/"),
            Err(RequestSpecError::InvalidUrl)
        );
    }

    #[test]
    fn credentials_in_a_url_are_refused_rather_than_dropped() {
        assert_eq!(
            spec("https://ada:hunter2@example.com/api"),
            Err(RequestSpecError::CredentialsInUrl)
        );
        assert_eq!(
            spec("https://token@example.com/api"),
            Err(RequestSpecError::CredentialsInUrl)
        );
    }

    #[test]
    fn a_url_longer_than_a_url_may_be_is_refused() {
        let url = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));

        assert_eq!(spec(&url), Err(RequestSpecError::UrlTooLong));
    }

    #[test]
    fn every_ordinary_method_is_accepted() {
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
            let request = RequestSpec::new(method, "https://example.com/", Headers::new(), vec![])
                .expect("a valid request");

            assert_eq!(request.method(), method);
        }
    }

    #[test]
    fn an_unusual_but_valid_method_is_accepted() {
        let request = RequestSpec::new("PURGE", "https://example.com/", Headers::new(), vec![])
            .expect("a valid request");

        assert_eq!(request.method(), "PURGE");
    }

    #[test]
    fn a_method_that_is_not_a_token_is_refused() {
        for method in ["GET POST", "GE\tT", "GET\r\nX", "{}"] {
            assert_eq!(
                RequestSpec::new(method, "https://example.com/", Headers::new(), vec![]),
                Err(RequestSpecError::InvalidMethod),
                "{method:?} was accepted"
            );
        }
    }

    #[test]
    fn an_empty_method_is_refused() {
        assert_eq!(
            RequestSpec::new("", "https://example.com/", Headers::new(), vec![]),
            Err(RequestSpecError::MissingMethod)
        );
    }

    #[test]
    fn headers_keep_their_order_and_their_repeats() {
        let request = RequestSpec::new(
            "GET",
            "https://example.com/",
            headers(&[
                ("Accept", "application/json"),
                ("X-Trace", "one"),
                ("X-Trace", "two"),
            ]),
            vec![],
        )
        .expect("a valid request");

        let names: Vec<&str> = request.headers().iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Accept", "X-Trace", "X-Trace"]);
        let traces: Vec<&str> = request
            .headers()
            .get_all("x-trace")
            .filter_map(HeaderValue::as_text)
            .collect();
        assert_eq!(traces, ["one", "two"]);
    }

    #[test]
    fn a_header_value_with_a_line_break_is_refused() {
        for value in ["a\r\nX-Evil: yes", "a\nb", "a\0b"] {
            assert_eq!(
                RequestSpec::new(
                    "GET",
                    "https://example.com/",
                    headers(&[("X-Thing", value)]),
                    vec![]
                ),
                Err(RequestSpecError::InvalidHeaderValue),
                "{value:?} was accepted"
            );
        }
    }

    #[test]
    fn a_header_name_that_is_not_a_token_is_refused() {
        for name in ["X Thing", "X:Thing", "", "X\r\nY"] {
            assert_eq!(
                RequestSpec::new(
                    "GET",
                    "https://example.com/",
                    headers(&[(name, "value")]),
                    vec![]
                ),
                Err(RequestSpecError::InvalidHeaderName),
                "{name:?} was accepted"
            );
        }
    }

    #[test]
    fn a_header_value_that_is_not_utf8_is_kept() {
        let mut carried = Headers::new();
        carried.push("X-Binary", HeaderValue::Bytes(vec![0xc3, 0x28, 0xff]));

        let request = RequestSpec::new("GET", "https://example.com/", carried, vec![])
            .expect("a valid request");

        assert_eq!(
            request.headers().get("x-binary").map(HeaderValue::as_bytes),
            Some(&[0xc3, 0x28, 0xff][..])
        );
    }

    #[test]
    fn proxy_authorization_is_refused_rather_than_dropped_later() {
        assert_eq!(
            RequestSpec::new(
                "GET",
                "https://example.com/",
                headers(&[("Proxy-Authorization", "Basic VEVTVDpURVNU")]),
                vec![]
            ),
            Err(RequestSpecError::ProxyAuthentication)
        );
    }

    #[test]
    fn more_headers_than_a_request_may_carry_are_refused() {
        let many: Headers = (0..=MAX_HEADERS)
            .map(|index| proxy_storage::Header::text(format!("X-{index}"), "value"))
            .collect();

        assert_eq!(
            RequestSpec::new("GET", "https://example.com/", many, vec![]),
            Err(RequestSpecError::TooManyHeaders)
        );
    }

    #[test]
    fn a_body_is_bytes_and_need_not_be_text() {
        let bytes: Vec<u8> = (0..=255).collect();

        let request = RequestSpec::new(
            "POST",
            "https://example.com/upload",
            Headers::new(),
            bytes.clone(),
        )
        .expect("a valid request");

        assert_eq!(request.body(), bytes);
    }

    #[test]
    fn a_request_round_trips_through_json() {
        let request = RequestSpec::new(
            "POST",
            "https://example.com/api",
            headers(&[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer TEST_TOKEN"),
            ]),
            br#"{"name":"ada"}"#.to_vec(),
        )
        .expect("a valid request");

        let json = serde_json::to_string(&request).expect("serializes");
        let back: RequestSpec = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, request);
    }

    #[test]
    fn a_binary_body_round_trips_through_json() {
        let bytes: Vec<u8> = (0..=255).collect();
        let request = RequestSpec::new(
            "PUT",
            "https://example.com/blob",
            Headers::new(),
            bytes.clone(),
        )
        .expect("a valid request");

        let json = serde_json::to_string(&request).expect("serializes");
        let back: RequestSpec = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back.body(), bytes);
    }

    #[test]
    fn json_names_the_parts_a_client_would_send() {
        let request = RequestSpec::new(
            "POST",
            "https://example.com/api",
            headers(&[("Accept", "application/json")]),
            b"hi".to_vec(),
        )
        .expect("a valid request");

        let json = serde_json::to_value(&request).expect("serializes");

        assert_eq!(json["method"], "POST");
        assert_eq!(json["url"], "https://example.com/api");
        assert_eq!(json["headers"][0]["name"], "Accept");
        assert_eq!(json["headers"][0]["value"]["text"], "application/json");
        assert_eq!(json["body"], "aGk=");
    }

    #[test]
    fn json_that_describes_an_invalid_request_does_not_deserialize() {
        let json = r#"{"method":"GET","url":"ftp://example.com/","headers":[],"body":""}"#;

        assert!(serde_json::from_str::<RequestSpec>(json).is_err());
    }

    #[test]
    fn parts_that_name_no_url_are_refused_for_naming_no_url() {
        // Rather than as an object that could not be read: the two are
        // different things to be told, and only one of them can be fixed.
        let parts: RequestParts =
            serde_json::from_str(r#"{"method":"GET"}"#).expect("deserializes");

        assert_eq!(
            RequestSpec::from_parts(parts),
            Err(RequestSpecError::MissingUrl)
        );
    }

    #[test]
    fn parts_with_no_headers_and_no_body_deserialize() {
        let json = r#"{"method":"GET","url":"https://example.com/"}"#;
        let request: RequestSpec = serde_json::from_str(json).expect("deserializes");

        assert!(request.headers().is_empty());
        assert!(request.body().is_empty());
    }

    #[test]
    fn debug_names_the_request_without_its_headers_or_body() {
        let request = RequestSpec::new(
            "POST",
            "https://example.com/api",
            headers(&[("Authorization", "Bearer TEST_TOKEN")]),
            b"secret-payload".to_vec(),
        )
        .expect("a valid request");

        let rendered = format!("{request:?}");

        assert!(!rendered.contains("TEST_TOKEN"), "{rendered}");
        assert!(!rendered.contains("secret-payload"), "{rendered}");
        assert!(rendered.contains("https://example.com/api"), "{rendered}");
    }

    #[test]
    fn an_authority_is_spelled_the_way_a_url_spells_it() {
        assert_eq!(
            authority_of("example.com", 8080).map(|a| a.to_string()),
            Some("example.com:8080".to_owned())
        );
        assert_eq!(
            authority_of("::1", 443).map(|a| a.to_string()),
            Some("[::1]:443".to_owned())
        );
    }

    /// A saved request, as `proxy-storage` spells one.
    fn saved(url: &str) -> SavedRequest {
        let mut new = proxy_storage::NewSavedRequest::new("Create a user", "POST", url);
        new.headers = headers(&[("Authorization", "Bearer sk-secret")]);
        new.body = br#"{"name":"ada"}"#.to_vec();

        SavedRequest::create(new).expect("a saved request")
    }

    #[test]
    fn a_saved_request_becomes_the_request_it_describes() {
        let request =
            RequestSpec::from_saved(&saved("https://example.com/users")).expect("a valid request");

        assert_eq!(request.method(), "POST");
        assert_eq!(request.url(), "https://example.com/users");
        assert_eq!(request.host(), "example.com");
        assert_eq!(request.port(), 443);
        assert_eq!(
            request.headers().text("authorization"),
            Some("Bearer sk-secret")
        );
        assert_eq!(request.body(), br#"{"name":"ada"}"#);
    }

    #[test]
    fn a_saved_request_this_proxy_cannot_send_is_refused_the_same_way_any_other_is() {
        // Checked here and nowhere else: `proxy-storage` persists a saved
        // request and has no idea what a URL may be.
        let error = RequestSpec::from_saved(&saved("ftp://example.com/users"))
            .expect_err("an unusable scheme is refused");

        assert!(
            matches!(error, RequestSpecError::UnsupportedScheme),
            "{error:?}"
        );
    }

    #[test]
    fn a_saved_request_sends_as_much_of_the_body_as_was_kept() {
        let mut saved = saved("https://example.com/users");
        saved.body = proxy_storage::BodyCapture::truncated(b"the first bit".to_vec(), 5_000);

        let request = RequestSpec::from_saved(&saved).expect("a valid request");

        assert_eq!(request.body(), b"the first bit");
        assert!(saved.body.is_truncated(), "the saved request still says so");
    }
}
