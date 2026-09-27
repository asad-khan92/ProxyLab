//! A request written as cURL and read back is the same request.
//!
//! ```text
//! RequestSpec ──▶ Copy as cURL ──▶ Import cURL ──▶ RequestSpec
//! ```
//!
//! Method, URL, headers, and body survive that trip. Not the *command* —
//! there is no promise that a command someone else wrote comes back spelled
//! the way they wrote it — but the request the command describes, which is
//! the thing anybody actually cares about when they copy a request out of
//! one window and into another.
//!
//! What does not survive is stated here too: the headers a generated command
//! deliberately leaves out, because cURL works them out from the URL and the
//! body it is given.

use proxy_curl::{from_captured, from_request, parse};
use proxy_http::RequestSpec;
use proxy_storage::{BodyCapture, CapturedRequest, Header, HeaderValue, Headers, HttpVersion};

/// The request a request comes back as, having been written and read.
fn round_trip(request: &RequestSpec) -> RequestSpec {
    let command = from_request(request).command;

    parse(&command)
        .unwrap_or_else(|error| panic!("the generated command did not parse: {error}\n{command}"))
        .request
}

fn headers(entries: &[(&str, &str)]) -> Headers {
    entries
        .iter()
        .map(|(name, value)| Header::text(*name, *value))
        .collect()
}

fn request(method: &str, url: &str, entries: &[(&str, &str)], body: &[u8]) -> RequestSpec {
    RequestSpec::new(method, url, headers(entries), body.to_vec()).expect("a valid request")
}

#[test]
fn a_get_survives() {
    let original = request("GET", "https://example.com/api/users?active=true", &[], b"");

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_get_with_headers_survives() {
    let original = request(
        "GET",
        "https://example.com/api",
        &[
            ("Accept", "application/json"),
            ("Authorization", "Bearer TEST_TOKEN"),
            ("Cookie", "session=TEST_SESSION"),
        ],
        b"",
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_post_with_a_json_body_survives() {
    let original = request(
        "POST",
        "https://example.com/api/users",
        &[("Content-Type", "application/json")],
        br#"{"name":"Ada Lovelace","roles":["admin","user"]}"#,
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn every_ordinary_method_survives() {
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        let original = request(method, "https://example.com/thing", &[], b"");

        assert_eq!(round_trip(&original).method(), method);
    }
}

#[test]
fn duplicate_headers_survive_in_order() {
    let original = request(
        "GET",
        "https://example.com/",
        &[
            ("X-Trace", "first"),
            ("Accept", "*/*"),
            ("X-Trace", "second"),
        ],
        b"",
    );

    let back = round_trip(&original);

    let names: Vec<&str> = back.headers().iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["X-Trace", "Accept", "X-Trace"]);
    let traces: Vec<&str> = back
        .headers()
        .get_all("x-trace")
        .filter_map(HeaderValue::as_text)
        .collect();
    assert_eq!(traces, ["first", "second"]);
}

#[test]
fn a_body_full_of_quotes_survives() {
    let original = request(
        "POST",
        "https://example.com/api",
        &[("Content-Type", "application/json")],
        br#"{"it's":"a \"quoted\" thing",'single':"$(not a command)"}"#,
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_header_value_full_of_shell_characters_survives() {
    let original = request(
        "GET",
        "https://example.com/",
        &[("X-Note", "a b ; c | d & e $(f) `g` 'h' \"i\" \\j")],
        b"",
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_url_with_spaces_and_quotes_encoded_in_it_survives() {
    let original = request(
        "GET",
        "https://example.com/search?q=a%20b&r=%27quoted%27&s=%24%28cmd%29",
        &[],
        b"",
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_multiline_body_survives() {
    let original = request(
        "PUT",
        "https://example.com/config",
        &[("Content-Type", "text/yaml")],
        b"a: 1\nb:\n  - x\n  - y\n",
    );

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_body_that_is_not_text_survives_byte_for_byte() {
    let bytes: Vec<u8> = (0..=255).collect();
    let original = request(
        "POST",
        "https://example.com/blob",
        &[("Content-Type", "application/octet-stream")],
        &bytes,
    );

    let back = round_trip(&original);

    assert_eq!(back.body(), bytes);
    assert_eq!(back, original);
}

#[test]
fn a_body_that_begins_with_an_at_sign_survives_as_a_body() {
    let original = request("POST", "https://example.com/api", &[], b"@not-a-file-name");
    let back = round_trip(&original);

    assert_eq!(back.method(), "POST");
    assert_eq!(back.url(), original.url());
    assert_eq!(back.body(), b"@not-a-file-name");
}

#[test]
fn a_body_with_no_content_type_gains_the_one_curl_would_send() {
    // The one thing a round trip does not preserve, and it is cURL's own
    // rule rather than this crate's: running `curl --data-raw …` sends
    // `application/x-www-form-urlencoded` when the command names no content
    // type, so an imported command describes a request that carries it. A
    // command that wants none says so the way cURL does, with `-H 'Name:'`.
    let original = request("POST", "https://example.com/api", &[], b"raw");

    let with_default = round_trip(&original);
    assert_eq!(
        with_default.headers().text("content-type"),
        Some("application/x-www-form-urlencoded")
    );

    let without = parse("curl https://example.com/api --data-raw 'raw' -H 'Content-Type:'")
        .expect("parses")
        .request;
    assert!(without.headers().is_empty());
    assert_eq!(without.body(), b"raw");
}

#[test]
fn a_named_port_survives() {
    let original = request("GET", "http://localhost:8080/health", &[], b"");

    assert_eq!(round_trip(&original), original);
}

#[test]
fn an_ipv6_host_survives() {
    let original = request("GET", "http://[::1]:8080/health", &[], b"");

    assert_eq!(round_trip(&original), original);
}

#[test]
fn a_header_value_that_is_not_text_survives() {
    let mut carried = Headers::new();
    carried.push("X-Binary", HeaderValue::Bytes(vec![0xc3, 0x28, 0xff]));
    let original = RequestSpec::new("GET", "https://example.com/", carried, Vec::new())
        .expect("a valid request");

    assert_eq!(round_trip(&original), original);
}

// MARK: - A captured exchange, copied and imported

#[test]
fn a_captured_request_comes_back_as_the_request_it_was() {
    let mut captured = CapturedRequest::new(
        "POST",
        "https://api.example.com/v1/users",
        HttpVersion::Http11,
    );
    captured
        .headers
        .push("Content-Type", HeaderValue::Text("application/json".into()));
    captured.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer TEST_TOKEN".into()),
    );
    captured
        .headers
        .push("Cookie", HeaderValue::Text("session=TEST_SESSION".into()));
    captured.body = BodyCapture::complete(br#"{"name":"Ada"}"#.to_vec());

    let command = from_captured(&captured).command;
    let back = parse(&command)
        .expect("the generated command parses")
        .request;

    assert_eq!(back.method(), "POST");
    assert_eq!(back.url(), "https://api.example.com/v1/users");
    assert_eq!(
        back.headers().text("content-type"),
        Some("application/json")
    );
    assert_eq!(
        back.headers().text("authorization"),
        Some("Bearer TEST_TOKEN")
    );
    assert_eq!(back.headers().text("cookie"), Some("session=TEST_SESSION"));
    assert_eq!(back.body(), br#"{"name":"Ada"}"#);
}

#[test]
fn the_headers_curl_recomputes_do_not_come_back() {
    // Stated rather than lamented: `Host` comes from the URL and
    // `Content-Length` from the body, so writing them into the command would
    // be writing the same fact twice — and wrong the moment the URL or the
    // body is edited in the replay editor.
    let mut captured = CapturedRequest::new("POST", "https://example.com/api", HttpVersion::Http11);
    captured
        .headers
        .push("Host", HeaderValue::Text("example.com".into()));
    captured
        .headers
        .push("Content-Length", HeaderValue::Text("2".into()));
    captured
        .headers
        .push("Accept", HeaderValue::Text("*/*".into()));
    captured.body = BodyCapture::complete(b"hi".to_vec());

    let command = from_captured(&captured).command;
    let back = parse(&command)
        .expect("the generated command parses")
        .request;

    assert!(!back.headers().contains("host"));
    assert!(!back.headers().contains("content-length"));
    assert_eq!(back.headers().text("accept"), Some("*/*"));
    // What the request is actually for is unchanged: the URL still names the
    // host, and the body is still the body.
    assert_eq!(back.host_header(), "example.com");
    assert_eq!(back.body(), b"hi");
}

#[test]
fn a_truncated_capture_still_produces_a_command_that_parses() {
    let mut captured =
        CapturedRequest::new("POST", "https://example.com/upload", HttpVersion::Http11);
    captured.body = BodyCapture::truncated(b"the first part of it".to_vec(), 1_000_000);

    let written = from_captured(&captured);
    assert!(written.truncated);

    // The note above the command is a shell comment, so the command it is
    // above is still a command — and this crate reads it back.
    let back = parse(&written.command)
        .expect("the generated command parses")
        .request;

    assert_eq!(back.body(), b"the first part of it");
}
