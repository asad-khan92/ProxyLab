//! One captured exchange written as one HAR entry, and read back again.
//!
//! ```text
//! TrafficExchange ──▶ Entry     export
//! Entry ──▶ TrafficExchange     import
//! ```
//!
//! This module is the whole of what HAR means to ProxyLab. Everything above
//! it is a file and a list of entries; everything below it is the traffic
//! model the proxy already has. Nothing here opens a file, and nothing here
//! knows where an exchange came from or is going.
//!
//! # A body is bytes, and HAR is text
//!
//! ```text
//! valid UTF-8   ──▶  "text": the body's own characters, no encoding
//! anything else ──▶  "text": base64, "encoding": "base64"
//! ```
//!
//! Which is the convention every HAR writer uses, and the one HAR 1.2 names
//! for `content`. Reading is the same rule backwards, so a body that went out
//! as bytes comes back as the same bytes.
//!
//! # Truncation survives the round trip without a field for it
//!
//! `size` is how long the body *was*; `text` holds as much of it as ProxyLab
//! captured. A capture cut short at the proxy's limit therefore writes a
//! `size` larger than the text it carries, and reading a file back compares
//! the two: more `size` than bytes is a truncated capture, exactly as it was
//! before it was written. No field of ProxyLab's own is added to the format
//! for it, so a HAR file this crate writes is a HAR file any other tool
//! reads.
//!
//! # What an import does not reconstruct
//!
//! The client address, and the moments the proxy measured on its own
//! monotonic clock. A HAR entry does not carry either: it has durations, and
//! a duration is not the same thing as "when the response head arrived". So
//! an imported exchange is recorded as having come from `127.0.0.1:0` — this
//! machine, on no port, which is what a replay is recorded as and is what
//! actually happened — and its timing carries the durations the entry gave.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use proxy_storage::{
    BodyCapture, CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId,
    HeaderValue, Headers, HttpVersion, Scheme, Timestamp, Timing, TrafficError, TrafficErrorKind,
    TrafficExchange,
};

use crate::model::{Content, Entry, NameValue, PostData, Request, Response, Timings};
use crate::time;

/// What `encoding` says when `text` is base64.
const BASE64_ENCODING: &str = "base64";

/// What is written beside a body the proxy's capture limit cut short.
const TRUNCATED_COMMENT: &str =
    "the body was longer than ProxyLab's capture limit, so only the captured bytes are here";

/// What is written beside an exchange that never got a response.
const NO_RESPONSE_COMMENT: &str = "ProxyLab captured no response: ";

/// The client an imported exchange is recorded as having come from.
///
/// This machine, on no port — the same thing a replay is recorded as, and for
/// the same reason: it is what actually happened.
const IMPORTED_CLIENT: &str = "127.0.0.1:0";

/// Writes one captured exchange as one HAR entry.
pub fn entry(exchange: &TrafficExchange) -> Entry {
    Entry {
        started_date_time: time::format(exchange.timing.started_at),
        time: exchange.timing.duration_ms().unwrap_or(-1.0),
        request: request(&exchange.request),
        response: match &exchange.response {
            Some(response) => self::response(response),
            None => no_response(),
        },
        cache: crate::model::Cache {},
        timings: timings(&exchange.timing),
        comment: exchange
            .error
            .as_ref()
            .map(|error| format!("{NO_RESPONSE_COMMENT}{}", error.summary())),
    }
}

/// Reads one HAR entry back as a captured exchange, or says it is not one.
///
/// An entry with no method and no URL is not an exchange, and is the one
/// thing refused: everything else a HAR file may leave out has an answer that
/// loses nothing a person was going to read.
///
/// `body_limit` is the most of any one body that is kept, which is the
/// proxy's own capture limit: a HAR file is written by something else and its
/// bodies are bounded by nothing ProxyLab chose.
pub fn exchange(entry: &Entry, body_limit: usize) -> Option<TrafficExchange> {
    let method = entry.request.method.trim();
    let url = entry.request.url.trim();
    if method.is_empty() || url.is_empty() {
        return None;
    }

    let captured = CapturedRequest {
        method: method.to_owned(),
        url: url.to_owned(),
        version: version_from(&entry.request.http_version),
        headers: headers_from(&entry.request.headers),
        body: request_body(&entry.request, body_limit),
    };

    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo {
            client: IMPORTED_CLIENT.parse().expect("a valid socket address"),
            scheme: scheme_of(url),
            destination: destination_of(url),
        },
        captured,
    );

    exchange.timing = timing_from(entry);

    // A status of zero is how this crate writes an exchange that never got a
    // response, and is what Chrome writes for a request that failed. Either
    // way it is not a status, and inventing one would tell a reader the
    // origin answered when it did not.
    if entry.response.status == 0 {
        exchange.error = Some(TrafficError::of(TrafficErrorKind::Incomplete));
    } else {
        exchange.response = Some(CapturedResponse {
            status: entry.response.status,
            reason: match entry.response.status_text.trim() {
                "" => None,
                text => Some(text.to_owned()),
            },
            version: version_from(&entry.response.http_version),
            headers: headers_from(&entry.response.headers),
            body: response_body(&entry.response, body_limit),
        });
    }

    Some(exchange)
}

// MARK: - Writing

/// The request half of an entry.
fn request(captured: &CapturedRequest) -> Request {
    Request {
        method: captured.method.clone(),
        url: captured.url.clone(),
        http_version: captured.version.to_string(),
        cookies: Vec::new(),
        headers: headers(&captured.headers),
        query_string: query(&captured.url),
        post_data: post_data(captured),
        headers_size: -1,
        body_size: size(&captured.body),
    }
}

/// The response half of an entry.
fn response(captured: &CapturedResponse) -> Response {
    let (text, encoding) = body(&captured.body);

    Response {
        status: captured.status,
        status_text: captured.reason.clone().unwrap_or_default(),
        http_version: captured.version.to_string(),
        cookies: Vec::new(),
        headers: headers(&captured.headers),
        content: Content {
            size: size(&captured.body),
            mime_type: content_type(&captured.headers),
            text,
            encoding,
            comment: truncation_comment(&captured.body),
        },
        redirect_url: captured
            .headers
            .text("location")
            .unwrap_or_default()
            .to_owned(),
        headers_size: -1,
        body_size: size(&captured.body),
    }
}

/// The response half of an entry that has no response.
fn no_response() -> Response {
    Response {
        status: 0,
        status_text: String::new(),
        http_version: String::new(),
        cookies: Vec::new(),
        headers: Vec::new(),
        content: Content {
            size: 0,
            mime_type: String::new(),
            text: None,
            encoding: None,
            comment: None,
        },
        redirect_url: String::new(),
        headers_size: -1,
        body_size: -1,
    }
}

/// The request's body, or nothing when it had none.
fn post_data(captured: &CapturedRequest) -> Option<PostData> {
    if captured.body.is_empty() {
        return None;
    }

    let (text, encoding) = body(&captured.body);

    Some(PostData {
        mime_type: content_type(&captured.headers),
        params: Vec::new(),
        text,
        encoding,
        comment: truncation_comment(&captured.body),
    })
}

/// A captured body as the text and encoding HAR carries it with.
fn body(capture: &BodyCapture) -> (Option<String>, Option<String>) {
    if capture.captured_len() == 0 {
        return (None, None);
    }

    match capture.as_text() {
        Some(text) => (Some(text.to_owned()), None),
        None => (
            Some(BASE64.encode(capture.data())),
            Some(BASE64_ENCODING.to_owned()),
        ),
    }
}

/// How long a body was, as HAR writes a size.
fn size(capture: &BodyCapture) -> i64 {
    capture.size().min(i64::MAX as u64) as i64
}

/// What is said about a capture that was cut short, if it was.
fn truncation_comment(capture: &BodyCapture) -> Option<String> {
    capture.is_truncated().then(|| TRUNCATED_COMMENT.to_owned())
}

/// The headers, in the order they were sent.
///
/// A value that is not text is written as base64 with a comment saying so.
/// There is nowhere in HAR to say it properly — a header is a name and a
/// string — and dropping the header entirely would be an exported request
/// that is not the request that was made.
fn headers(headers: &Headers) -> Vec<NameValue> {
    headers
        .iter()
        .map(|header| match &header.value {
            HeaderValue::Text(text) => NameValue::new(&header.name, text),
            HeaderValue::Bytes(bytes) => NameValue {
                name: header.name.clone(),
                value: BASE64.encode(bytes),
                comment: Some("the value was not text, and is written here as base64".to_owned()),
            },
        })
        .collect()
}

/// What a message says its body is.
fn content_type(headers: &Headers) -> String {
    headers.text("content-type").unwrap_or_default().to_owned()
}

/// The query, read out of the URL and decoded.
///
/// HAR wants the parameters as a person would read them, so `%20` is a space
/// and `+` is a space. A parameter with no `=` is a name with an empty value,
/// which is what it is.
fn query(url: &str) -> Vec<NameValue> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = query.split('#').next().unwrap_or_default();

    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => NameValue::new(decode(name), decode(value)),
            None => NameValue::new(decode(pair), ""),
        })
        .collect()
}

/// One query parameter, with its percent-escapes and its pluses undone.
///
/// A `%` that is not followed by two hexadecimal digits is kept as a `%`:
/// what is being read is a URL somebody's client sent, and refusing to show
/// a parameter because it is spelled loosely would be showing less than was
/// captured.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// One hexadecimal digit's value.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The timings of one exchange.
///
/// `send` is zero rather than unmeasured: the request was written, and HAR
/// requires the three of `send`, `wait` and `receive` to be numbers. What
/// ProxyLab does not separate out — blocked, DNS, SSL — stays `-1`, which is
/// what HAR says "not measured" is.
///
/// The three that are written add up to `time` whenever the exchange
/// finished, because HAR says they must: a reader that draws a waterfall
/// from them gets a bar the same length as the duration beside it.
fn timings(timing: &Timing) -> Timings {
    let total = timing.completed_micros;
    let connect = timing.upstream_connect_micros;
    let head = timing.upstream_response_micros;

    let (connect_ms, wait_ms, receive_ms) = match (connect, head, total) {
        // Everything measured: the connection, the wait for the head, and
        // the body after it.
        (Some(connect), Some(head), Some(total)) => (
            ms(connect),
            ms(head.saturating_sub(connect)),
            ms(total.saturating_sub(head)),
        ),
        // An intercepted request, whose connection was opened by the CONNECT
        // before there was a request to attribute it to.
        (None, Some(head), Some(total)) => (-1.0, ms(head), ms(total.saturating_sub(head))),
        // Answered by the proxy itself — blocked, or mapped to a local file
        // — so there was no origin to wait for and nothing to receive.
        (_, None, Some(total)) => (-1.0, ms(total), 0.0),
        // Never finished.
        (_, _, None) => (-1.0, 0.0, 0.0),
    };

    Timings {
        blocked: -1.0,
        dns: -1.0,
        connect: connect_ms,
        send: 0.0,
        wait: wait_ms,
        receive: receive_ms,
        ssl: -1.0,
    }
}

/// Microseconds as the milliseconds HAR counts in.
fn ms(micros: u64) -> f64 {
    micros as f64 / 1_000.0
}

// MARK: - Reading

/// The headers of one half of an entry.
fn headers_from(values: &[NameValue]) -> Headers {
    let mut headers = Headers::new();

    for value in values {
        if value.name.trim().is_empty() {
            continue;
        }

        headers.push(&value.name, HeaderValue::Text(value.value.clone()));
    }

    headers
}

/// The request's captured body.
fn request_body(request: &Request, limit: usize) -> BodyCapture {
    let Some(data) = request
        .post_data
        .as_ref()
        .map(|post| decode_body(post.text.as_deref(), post.encoding.as_deref()))
    else {
        return BodyCapture::empty();
    };

    capture(data, request.body_size, limit)
}

/// The response's captured body.
fn response_body(response: &Response, limit: usize) -> BodyCapture {
    let data = decode_body(
        response.content.text.as_deref(),
        response.content.encoding.as_deref(),
    );
    // `content.size` is the body's length; `bodySize` is what came over the
    // wire, which for a compressed response is smaller. The content's own
    // size is the one that describes the bytes in `text`.
    capture(data, response.content.size, limit)
}

/// The bytes a HAR body carries.
///
/// Base64 that does not decode falls back to the characters as they stand:
/// the file says the body was bytes and this one is not the base64 it claims
/// to be, and showing what is there beats losing the entry.
fn decode_body(text: Option<&str>, encoding: Option<&str>) -> Vec<u8> {
    let Some(text) = text else {
        return Vec::new();
    };

    match encoding {
        Some(encoding) if encoding.eq_ignore_ascii_case(BASE64_ENCODING) => BASE64
            .decode(text.trim())
            .unwrap_or_else(|_| text.as_bytes().to_vec()),
        _ => text.as_bytes().to_vec(),
    }
}

/// A capture holding `data`, of a body that was `declared` bytes long.
///
/// Three ways it can come out. The body may be longer than what is here,
/// which is a capture the writer cut short and is kept as one; it may be
/// longer than this build will hold, which is a capture *this* read cuts
/// short; or the two may agree, which is a whole body.
fn capture(data: Vec<u8>, declared: i64, limit: usize) -> BodyCapture {
    let held = data.len();
    let declared = if declared > 0 { declared as u64 } else { 0 };
    let size = declared.max(held as u64);

    if held > limit {
        return BodyCapture::truncated(data[..limit].to_vec(), size);
    }
    if size > held as u64 {
        return BodyCapture::truncated(data, size);
    }

    BodyCapture::complete(data)
}

/// The timing an entry describes.
///
/// The wall-clock moment is the entry's own. The offsets are rebuilt from the
/// durations beside it, which is the only thing a HAR file has: `connect` and
/// `wait` add up to when the head arrived, and `time` is when it finished.
/// An entry whose timings were never measured still gets `time`, because that
/// is the one a traffic list shows.
fn timing_from(entry: &Entry) -> Timing {
    let connect = positive(entry.timings.connect);
    let wait = positive(entry.timings.wait);
    let receive = positive(entry.timings.receive);
    let total = positive(entry.time).or_else(|| {
        let measured = connect.unwrap_or(0) + wait.unwrap_or(0) + receive.unwrap_or(0);
        (measured > 0).then_some(measured)
    });

    let head = match (connect, wait) {
        (None, None) => None,
        (connect, wait) => Some(connect.unwrap_or(0) + wait.unwrap_or(0)),
    };

    Timing {
        started_at: time::parse(&entry.started_date_time).unwrap_or_else(Timestamp::now),
        upstream_connect_micros: connect,
        // Never past the end: a file whose durations do not add up would
        // otherwise produce an exchange whose head arrived after it finished.
        upstream_response_micros: match (head, total) {
            (Some(head), Some(total)) => Some(head.min(total)),
            (head, _) => head,
        },
        completed_micros: total,
    }
}

/// A HAR duration in microseconds, or nothing when it was not measured.
fn positive(ms: f64) -> Option<u64> {
    (ms.is_finite() && ms > 0.0).then_some((ms * 1_000.0) as u64)
}

/// Which scheme a URL names.
fn scheme_of(url: &str) -> Scheme {
    if url.len() >= 6 && url[..6].eq_ignore_ascii_case("https:") {
        Scheme::Https
    } else {
        Scheme::Http
    }
}

/// The host and port a URL names, when it names one.
///
/// Parsed here rather than with the `http` crate's URL type, which this
/// crate would otherwise not need: what is wanted is the authority of a URL
/// that was already sent once, and a URL that will not parse is an entry to
/// show with no destination rather than an entry to lose.
fn destination_of(url: &str) -> Option<Destination> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;
    // Credentials in a URL's authority are not kept: the URL is what a
    // traffic list shows, and the destination is what it is filtered on.
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);

    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 literal, whose colons are not a port separator.
        Some(rest) => {
            let (host, rest) = rest.split_once(']')?;
            (host, rest.strip_prefix(':').and_then(|p| p.parse().ok()))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()),
            None => (authority, None),
        },
    };

    if host.is_empty() {
        return None;
    }

    Some(Destination::new(
        host.to_ascii_lowercase(),
        port.unwrap_or(match scheme_of(url) {
            Scheme::Https => 443,
            Scheme::Http => 80,
        }),
    ))
}

/// The HTTP version an entry names.
///
/// Generous, because the spelling is not agreed on: ProxyLab writes
/// `HTTP/2.0`, Chrome writes `h2`, and other tools write `HTTP/2`. Anything
/// this build does not recognise is [`HttpVersion::Unknown`], which the model
/// already has for exactly this.
fn version_from(text: &str) -> HttpVersion {
    let text = text.trim();

    if text.eq_ignore_ascii_case("HTTP/0.9") {
        HttpVersion::Http09
    } else if text.eq_ignore_ascii_case("HTTP/1.0") {
        HttpVersion::Http10
    } else if text.eq_ignore_ascii_case("HTTP/1.1") || text.eq_ignore_ascii_case("http/1") {
        HttpVersion::Http11
    } else if text.eq_ignore_ascii_case("HTTP/2.0")
        || text.eq_ignore_ascii_case("HTTP/2")
        || text.eq_ignore_ascii_case("h2")
        || text.eq_ignore_ascii_case("h2c")
    {
        HttpVersion::Http2
    } else if text.eq_ignore_ascii_case("HTTP/3.0")
        || text.eq_ignore_ascii_case("HTTP/3")
        || text.eq_ignore_ascii_case("h3")
    {
        HttpVersion::Http3
    } else {
        HttpVersion::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::DEFAULT_MAX_CAPTURE_BODY_SIZE;

    const LIMIT: usize = DEFAULT_MAX_CAPTURE_BODY_SIZE;

    fn exchange_fixture() -> TrafficExchange {
        let mut request = CapturedRequest::new(
            "POST",
            "https://example.com/api/users?active=true&q=a%20b",
            HttpVersion::Http11,
        );
        request
            .headers
            .push("Content-Type", HeaderValue::Text("application/json".into()));
        request.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        request.body = BodyCapture::complete(br#"{"name":"ada"}"#.to_vec());

        let mut response = CapturedResponse::new(201, HttpVersion::Http2);
        response.reason = Some("Created".to_owned());
        response
            .headers
            .push("Content-Type", HeaderValue::Text("application/json".into()));
        response.body = BodyCapture::complete(br#"{"id":1}"#.to_vec());

        let mut exchange = TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::https(
                "127.0.0.1:54321".parse().expect("an address"),
                Some(Destination::new("example.com", 443)),
            ),
            request,
        );
        exchange.response = Some(response);
        exchange.timing = Timing {
            started_at: Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            },
            upstream_connect_micros: Some(1_000),
            upstream_response_micros: Some(3_000),
            completed_micros: Some(3_500),
        };

        exchange
    }

    #[test]
    fn an_entry_carries_the_request_and_the_response() {
        let written = entry(&exchange_fixture());

        assert_eq!(written.request.method, "POST");
        assert_eq!(
            written.request.url,
            "https://example.com/api/users?active=true&q=a%20b"
        );
        assert_eq!(written.request.http_version, "HTTP/1.1");
        assert_eq!(written.response.status, 201);
        assert_eq!(written.response.status_text, "Created");
        assert_eq!(written.response.http_version, "HTTP/2.0");
        assert_eq!(written.started_date_time, "2023-11-14T22:13:20.000Z");
        assert_eq!(written.time, 3.5);
    }

    #[test]
    fn the_headers_keep_their_order_and_their_values() {
        let written = entry(&exchange_fixture());

        assert_eq!(
            written.request.headers,
            vec![
                NameValue::new("Content-Type", "application/json"),
                NameValue::new("Authorization", "Bearer sk-secret"),
            ]
        );
    }

    #[test]
    fn the_query_is_read_out_of_the_url_and_decoded() {
        let written = entry(&exchange_fixture());

        assert_eq!(
            written.request.query_string,
            vec![NameValue::new("active", "true"), NameValue::new("q", "a b")]
        );
    }

    #[test]
    fn a_text_body_is_written_as_text() {
        let written = entry(&exchange_fixture());

        let post = written.request.post_data.expect("a request body");
        assert_eq!(post.text.as_deref(), Some(r#"{"name":"ada"}"#));
        assert_eq!(post.encoding, None);
        assert_eq!(post.mime_type, "application/json");
        assert_eq!(
            written.response.content.text.as_deref(),
            Some(r#"{"id":1}"#)
        );
        assert_eq!(written.response.content.encoding, None);
    }

    #[test]
    fn a_body_that_is_not_text_is_written_as_base64() {
        let mut exchange = exchange_fixture();
        let bytes: Vec<u8> = (0..=255).collect();
        exchange.request.body = BodyCapture::complete(bytes.clone());
        exchange.response.as_mut().expect("a response").body = BodyCapture::complete(bytes.clone());

        let written = entry(&exchange);

        let post = written.request.post_data.expect("a request body");
        assert_eq!(post.encoding.as_deref(), Some("base64"));
        assert_eq!(
            BASE64.decode(post.text.expect("text")).expect("decodes"),
            bytes
        );
        assert_eq!(written.response.content.encoding.as_deref(), Some("base64"));
    }

    #[test]
    fn a_truncated_capture_says_how_long_the_body_was() {
        let mut exchange = exchange_fixture();
        exchange.response.as_mut().expect("a response").body =
            BodyCapture::truncated(b"the first bit".to_vec(), 5_000);

        let written = entry(&exchange);

        assert_eq!(written.response.content.size, 5_000);
        assert_eq!(
            written.response.content.text.as_deref(),
            Some("the first bit")
        );
        assert!(written.response.content.comment.is_some());
    }

    #[test]
    fn an_exchange_with_no_response_is_written_with_a_status_of_zero() {
        let mut exchange = exchange_fixture();
        exchange.response = None;
        exchange.error = Some(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        let written = entry(&exchange);

        assert_eq!(written.response.status, 0);
        assert_eq!(written.response.content.size, 0);
        assert!(written
            .comment
            .expect("a comment")
            .contains("could not connect"));
    }

    #[test]
    fn the_timings_add_up_to_the_duration() {
        let written = entry(&exchange_fixture());

        assert_eq!(written.timings.connect, 1.0);
        assert_eq!(written.timings.wait, 2.0);
        assert_eq!(written.timings.receive, 0.5);
        assert_eq!(written.timings.send, 0.0);
        assert_eq!(
            written.timings.connect
                + written.timings.send
                + written.timings.wait
                + written.timings.receive,
            written.time
        );
    }

    #[test]
    fn an_intercepted_exchange_has_no_connect_timing_and_still_adds_up() {
        let mut exchange = exchange_fixture();
        exchange.timing.upstream_connect_micros = None;

        let written = entry(&exchange);

        assert_eq!(written.timings.connect, -1.0);
        assert_eq!(written.timings.wait + written.timings.receive, written.time);
    }

    #[test]
    fn an_exchange_the_proxy_answered_itself_waits_for_the_whole_of_it() {
        let mut exchange = exchange_fixture();
        exchange.timing.upstream_connect_micros = None;
        exchange.timing.upstream_response_micros = None;

        let written = entry(&exchange);

        assert_eq!(written.timings.wait, 3.5);
        assert_eq!(written.timings.receive, 0.0);
    }

    #[test]
    fn a_location_header_becomes_the_redirect_url() {
        let mut exchange = exchange_fixture();
        let response = exchange.response.as_mut().expect("a response");
        response.status = 302;
        response.headers.push(
            "Location",
            HeaderValue::Text("https://example.com/next".into()),
        );

        assert_eq!(
            entry(&exchange).response.redirect_url,
            "https://example.com/next"
        );
    }

    #[test]
    fn an_exchange_read_back_is_the_exchange_that_was_written() {
        let original = exchange_fixture();
        let back = exchange(&entry(&original), LIMIT).expect("an exchange");

        assert_eq!(back.request.method, original.request.method);
        assert_eq!(back.request.url, original.request.url);
        assert_eq!(back.request.version, original.request.version);
        assert_eq!(back.request.headers, original.request.headers);
        assert_eq!(back.request.body, original.request.body);

        let response = back.response.expect("a response");
        let expected = original.response.expect("a response");
        assert_eq!(response.status, expected.status);
        assert_eq!(response.reason, expected.reason);
        assert_eq!(response.version, expected.version);
        assert_eq!(response.headers, expected.headers);
        assert_eq!(response.body, expected.body);

        assert_eq!(back.timing.started_at, original.timing.started_at);
        assert_eq!(
            back.timing.completed_micros,
            original.timing.completed_micros
        );
        assert_eq!(
            back.timing.upstream_connect_micros,
            original.timing.upstream_connect_micros
        );
        assert_eq!(
            back.timing.upstream_response_micros,
            original.timing.upstream_response_micros
        );
        assert_eq!(back.connection.scheme, original.connection.scheme);
        assert_eq!(back.connection.destination, original.connection.destination);
    }

    #[test]
    fn a_binary_body_survives_the_round_trip_byte_for_byte() {
        let mut original = exchange_fixture();
        let bytes: Vec<u8> = (0..=255).cycle().take(4_096).collect();
        original.request.body = BodyCapture::complete(bytes.clone());
        original.response.as_mut().expect("a response").body = BodyCapture::complete(bytes.clone());

        let back = exchange(&entry(&original), LIMIT).expect("an exchange");

        assert_eq!(back.request.body.data(), bytes);
        assert_eq!(back.response.expect("a response").body.data(), bytes);
    }

    #[test]
    fn a_truncated_capture_is_still_truncated_after_a_round_trip() {
        let mut original = exchange_fixture();
        original.response.as_mut().expect("a response").body =
            BodyCapture::truncated(b"the first bit".to_vec(), 5_000);

        let back = exchange(&entry(&original), LIMIT).expect("an exchange");
        let body = back.response.expect("a response").body;

        assert!(body.is_truncated());
        assert_eq!(body.size(), 5_000);
        assert_eq!(body.data(), b"the first bit");
    }

    #[test]
    fn a_failed_exchange_comes_back_with_no_response() {
        let mut original = exchange_fixture();
        original.response = None;
        original.error = Some(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        let back = exchange(&entry(&original), LIMIT).expect("an exchange");

        assert!(back.response.is_none());
        assert!(back.error.is_some());
        assert!(!back.succeeded());
    }

    #[test]
    fn a_body_longer_than_the_limit_is_cut_short_and_says_so() {
        let mut original = exchange_fixture();
        original.response.as_mut().expect("a response").body =
            BodyCapture::complete(vec![b'x'; 1_000]);

        let back = exchange(&entry(&original), 64).expect("an exchange");
        let body = back.response.expect("a response").body;

        assert_eq!(body.captured_len(), 64);
        assert_eq!(body.size(), 1_000);
        assert!(body.is_truncated());
    }

    #[test]
    fn an_entry_with_no_method_or_no_url_is_not_an_exchange() {
        let mut without_method = entry(&exchange_fixture());
        without_method.request.method = String::new();
        assert!(exchange(&without_method, LIMIT).is_none());

        let mut without_url = entry(&exchange_fixture());
        without_url.request.url = "  ".to_owned();
        assert!(exchange(&without_url, LIMIT).is_none());
    }

    #[test]
    fn an_entry_from_another_tool_is_read_with_what_it_has() {
        // No timings, no body, a version spelled Chrome's way, and no reason
        // phrase.
        let entry = Entry {
            started_date_time: "2026-09-20T10:11:12.345Z".to_owned(),
            time: 25.0,
            request: Request {
                method: "GET".to_owned(),
                url: "http://example.test:8080/thing".to_owned(),
                http_version: "h2".to_owned(),
                headers: vec![NameValue::new("Accept", "*/*")],
                ..Request::default()
            },
            response: Response {
                status: 204,
                http_version: "h2".to_owned(),
                ..Response::default()
            },
            ..Entry::default()
        };

        let back = exchange(&entry, LIMIT).expect("an exchange");

        assert_eq!(back.request.version, HttpVersion::Http2);
        assert_eq!(
            back.connection.destination,
            Some(Destination::new("example.test", 8080))
        );
        assert_eq!(back.connection.scheme, Scheme::Http);
        assert_eq!(back.status(), Some(204));
        assert_eq!(back.response.expect("a response").reason, None);
        assert_eq!(back.timing.duration_ms(), Some(25.0));
        assert!(back.request.body.is_empty());
    }

    #[test]
    fn a_destination_is_read_out_of_every_shape_of_url() {
        let cases = [
            (
                "https://example.com/x",
                Some(Destination::new("example.com", 443)),
            ),
            (
                "http://example.com/x",
                Some(Destination::new("example.com", 80)),
            ),
            (
                "https://EXAMPLE.com",
                Some(Destination::new("example.com", 443)),
            ),
            (
                "http://example.com:8080",
                Some(Destination::new("example.com", 8080)),
            ),
            ("https://[::1]:8443/x", Some(Destination::new("::1", 8443))),
            ("https://[::1]/x", Some(Destination::new("::1", 443))),
            (
                "https://ada@example.com/x",
                Some(Destination::new("example.com", 443)),
            ),
            ("not a url", None),
            ("https:///x", None),
        ];

        for (url, expected) in cases {
            let mut entry = Entry::default();
            entry.request.method = "GET".to_owned();
            entry.request.url = url.to_owned();

            let back = exchange(&entry, LIMIT).expect("an exchange");
            assert_eq!(back.connection.destination, expected, "{url}");
        }
    }

    #[test]
    fn every_version_this_build_writes_is_one_it_reads() {
        for version in [
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
            HttpVersion::Http2,
            HttpVersion::Http3,
        ] {
            assert_eq!(version_from(&version.to_string()), version);
        }

        assert_eq!(version_from("something else"), HttpVersion::Unknown);
        assert_eq!(version_from(""), HttpVersion::Unknown);
    }

    #[test]
    fn a_header_value_that_is_not_text_is_written_as_base64_with_a_note() {
        let mut headers = Headers::new();
        headers.push("X-Thing", HeaderValue::Bytes(vec![0xff, 0xfe]));

        let written = self::headers(&headers);

        assert_eq!(written[0].value, BASE64.encode([0xff, 0xfe]));
        assert!(written[0].comment.is_some());
    }

    #[test]
    fn a_header_with_no_name_is_not_read_as_a_header() {
        let read = headers_from(&[NameValue::new(" ", "x"), NameValue::new("A", "b")]);

        assert_eq!(read.len(), 1);
        assert_eq!(read.text("a"), Some("b"));
    }

    #[test]
    fn a_query_parameter_with_no_value_is_a_name_with_an_empty_one() {
        assert_eq!(
            query("https://example.com/?flag&a=1"),
            vec![NameValue::new("flag", ""), NameValue::new("a", "1")]
        );
    }

    #[test]
    fn a_url_with_no_query_has_no_parameters() {
        assert!(query("https://example.com/path").is_empty());
        assert!(query("https://example.com/path?").is_empty());
    }

    #[test]
    fn a_loose_percent_escape_is_kept_as_it_stands() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%zzb"), "a%zzb");
        assert_eq!(decode("a%2Fb"), "a/b");
    }

    #[test]
    fn a_base64_body_that_does_not_decode_is_kept_as_the_text_it_is() {
        assert_eq!(
            decode_body(Some("not base64!"), Some("base64")),
            b"not base64!"
        );
    }

    #[test]
    fn nothing_written_names_the_header_a_capture_had_and_the_reader_did_not() {
        // The one thing that must not happen to an export: a credential
        // being dropped. The point of a HAR file is that it reproduces the
        // traffic.
        let written = entry(&exchange_fixture());
        let names: Vec<&str> = written
            .request
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect();

        assert!(names.contains(&"Authorization"), "{names:?}");
        assert_eq!(
            written.request.headers[1].value, "Bearer sk-secret",
            "the credential was not written exactly"
        );
    }
}
