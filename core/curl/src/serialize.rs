//! Writing a request down as the cURL command that would send it again.
//!
//! ```text
//! POST https://example.com/api          curl 'https://example.com/api' \
//! Content-Type: application/json   ──▶    -X POST \
//!                                         -H 'Content-Type: application/json' \
//! {"name":"ada"}                          --data-raw '{"name":"ada"}'
//! ```
//!
//! The output is **deterministic**: the same request produces the same
//! command, byte for byte, every time. Headers keep the order and the
//! repetitions the message had, and nothing is sorted, deduplicated, or
//! tidied on the way.
//!
//! # Quoting
//!
//! Everything is quoted, always — the URL with no special characters in it as
//! much as the body full of them. A single-quoted shell string is literal
//! throughout, so the only thing that needs escaping inside one is the quote
//! itself, and `'` becomes `'\''` the way it does everywhere else.
//!
//! A value that cannot be a single-quoted string — bytes that are not text,
//! or a control character that would be invisible in the output — is written
//! as `$'…'`, which is *quoting* and not substitution. So a body that is a
//! protocol buffer or a gzip stream survives being copied, pasted, and
//! imported again.
//!
//! # What is left out
//!
//! The headers that describe one hop rather than the message: `Connection`,
//! `Transfer-Encoding`, `Proxy-Authorization`, and the rest of RFC 9110's
//! hop-by-hop list. With them go `Content-Length`, which cURL computes from
//! the body it is given, and `Host`, which cURL takes from the URL — an
//! explicit one would be the same value written twice and wrong the moment
//! the URL is edited.
//!
//! Everything else is kept, `Authorization` and `Cookie` included. The point
//! of Copy as cURL is to send the request again, and a reproduction that
//! drops the credential reproduces nothing.
//!
//! # Nothing is logged
//!
//! This module writes a string and returns it. It does not print, and
//! [`CurlCommand`] renders its length rather than its text through [`Debug`],
//! so a generated command cannot reach a log by being formatted into one.

use std::fmt;

use proxy_http::RequestSpec;
use proxy_storage::{CapturedRequest, HeaderValue, Headers};

/// Headers a generated command leaves out.
///
/// The hop-by-hop headers of RFC 9110, plus `proxy-connection` — the
/// widespread non-standard spelling — and the two cURL works out for itself.
/// See the module documentation.
const OMITTED: &[&str] = &[
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The note put above a command whose body was not captured in full.
const TRUNCATED_NOTE: &str =
    "# ProxyLab captured only the beginning of this request body, so the command below \
     sends less than the original did.";

/// A cURL command, and whether it says everything the request did.
#[derive(Clone, PartialEq, Eq)]
pub struct CurlCommand {
    /// The command itself.
    pub command: String,
    /// Whether the captured body it was written from was truncated.
    ///
    /// `true` means the command sends less than the original request did,
    /// because ProxyLab's capture limit stopped before the body ended.
    pub truncated: bool,
}

/// The length and the truncation flag, never the command.
///
/// A generated command carries whatever the request carried — an
/// `Authorization` header, a session cookie, a body — which is the point of
/// it and is exactly why it is not something to format into a log line.
impl fmt::Debug for CurlCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurlCommand")
            .field("command", &self.command.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// The command that would send a captured request again.
///
/// The body is as much of it as the capture kept; a request whose body was
/// truncated produces a command that says so, in a comment above it and in
/// [`CurlCommand::truncated`].
pub fn from_captured(request: &CapturedRequest) -> CurlCommand {
    let truncated = request.body.is_truncated();
    let command = write(
        &request.method,
        &request.url,
        &request.headers,
        request.body.data(),
        truncated,
    );

    CurlCommand { command, truncated }
}

/// The command that would send a request that has not been sent yet.
pub fn from_request(request: &RequestSpec) -> CurlCommand {
    CurlCommand {
        command: write(
            request.method(),
            request.url(),
            request.headers(),
            request.body(),
            false,
        ),
        truncated: false,
    }
}

/// Writes the command.
fn write(method: &str, url: &str, headers: &Headers, body: &[u8], truncated: bool) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("curl {}", quote(url.as_bytes())));

    // Left out only for the one case cURL would get right on its own: a GET
    // with no body. Every other method is spelled out, so a command never
    // depends on cURL's own rule about what `--data` implies.
    if !(method == "GET" && body.is_empty()) {
        lines.push(format!("-X {}", method_word(method)));
    }

    for header in headers {
        if OMITTED
            .iter()
            .any(|omitted| header.name.eq_ignore_ascii_case(omitted))
        {
            continue;
        }

        lines.push(format!(
            "-H {}",
            quote(&header_line(header.name.as_str(), &header.value))
        ));
    }

    if !body.is_empty() {
        // `--data-raw` rather than `--data`, because `--data` reads a body
        // beginning with `@` out of a file, and a body that happens to begin
        // with `@` is a body and not a file name.
        lines.push(format!("--data-raw {}", quote(body)));
    }

    let mut command = String::new();
    if truncated {
        command.push_str(TRUNCATED_NOTE);
        command.push('\n');
    }
    command.push_str(&lines.join(" \\\n  "));

    command
}

/// The method as it is written on a `-X`.
///
/// A method is an HTTP token — no space, no quote, nothing a shell reads —
/// so it is written plainly, and `-X POST` reads as `-X POST`. A stored
/// exchange could in principle have been written into the database by hand
/// with something else in it, and that is quoted like anything else.
fn method_word(method: &str) -> String {
    let plain = !method.is_empty()
        && method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'*'));

    if plain {
        method.to_owned()
    } else {
        quote(method.as_bytes())
    }
}

/// One header, as it is written on a `-H`.
fn header_line(name: &str, value: &HeaderValue) -> Vec<u8> {
    let mut line = Vec::with_capacity(name.len() + 2 + value.len());

    line.extend_from_slice(name.as_bytes());
    line.extend_from_slice(b": ");
    line.extend_from_slice(value.as_bytes());

    line
}

/// Quotes bytes so that a shell hands them back exactly.
///
/// A single-quoted string when the bytes are text a person can read, and
/// `$'…'` when they are not — see the module documentation.
fn quote(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if is_plainly_quotable(text) => single_quoted(text),
        _ => ansi_c_quoted(bytes),
    }
}

/// Whether text can go inside `'…'` and come back as itself.
///
/// A tab, a newline, and a carriage return are all legal inside a
/// single-quoted string, and a JSON body that was sent pretty-printed is
/// exactly that. What is not allowed is a control character that would be
/// invisible in the output — a command nobody can read is a command nobody
/// can check.
fn is_plainly_quotable(text: &str) -> bool {
    text.chars().all(|character| {
        character == '\t' || character == '\n' || character == '\r' || !character.is_control()
    })
}

/// Text inside `'…'`, with the one escape a single-quoted string has.
fn single_quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);

    quoted.push('\'');
    for character in text.chars() {
        if character == '\'' {
            // A single-quoted string cannot contain a single quote, so the
            // string is closed, an escaped quote is written, and it is
            // opened again. Every shell reads this the same way.
            quoted.push_str(r"'\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');

    quoted
}

/// Bytes inside `$'…'`, where every one of them can be written exactly.
///
/// Hexadecimal escapes are always two digits, so a byte written `\x41`
/// followed by a literal `B` cannot be read as three hex digits.
fn ansi_c_quoted(bytes: &[u8]) -> String {
    let mut quoted = String::with_capacity(bytes.len() * 2 + 3);

    quoted.push_str("$'");
    for byte in bytes {
        match byte {
            b'\\' => quoted.push_str(r"\\"),
            b'\'' => quoted.push_str(r"\'"),
            b'\n' => quoted.push_str(r"\n"),
            b'\r' => quoted.push_str(r"\r"),
            b'\t' => quoted.push_str(r"\t"),
            0x20..=0x7e => quoted.push(char::from(*byte)),
            other => quoted.push_str(&format!(r"\x{other:02x}")),
        }
    }
    quoted.push('\'');

    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::{BodyCapture, Header, HttpVersion};

    fn request(method: &str, url: &str) -> CapturedRequest {
        CapturedRequest::new(method, url, HttpVersion::Http11)
    }

    fn command(request: &CapturedRequest) -> String {
        from_captured(request).command
    }

    #[test]
    fn a_get_is_the_url_and_nothing_else() {
        assert_eq!(
            command(&request("GET", "https://example.com/api")),
            "curl 'https://example.com/api'"
        );
    }

    #[test]
    fn a_method_that_is_not_get_is_spelled_out() {
        assert_eq!(
            command(&request("DELETE", "https://example.com/api/1")),
            "curl 'https://example.com/api/1' \\\n  -X DELETE"
        );
    }

    #[test]
    fn headers_keep_their_order_and_their_repeats() {
        let mut captured = request("GET", "https://example.com/api");
        captured
            .headers
            .push("Accept", HeaderValue::Text("application/json".into()));
        captured.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer TEST_TOKEN".into()),
        );
        captured
            .headers
            .push("X-Trace", HeaderValue::Text("one".into()));
        captured
            .headers
            .push("X-Trace", HeaderValue::Text("two".into()));

        assert_eq!(
            command(&captured),
            "curl 'https://example.com/api' \\\n  \
             -H 'Accept: application/json' \\\n  \
             -H 'Authorization: Bearer TEST_TOKEN' \\\n  \
             -H 'X-Trace: one' \\\n  \
             -H 'X-Trace: two'"
        );
    }

    #[test]
    fn a_post_with_a_body_carries_both() {
        let mut captured = request("POST", "https://example.com/api");
        captured
            .headers
            .push("Content-Type", HeaderValue::Text("application/json".into()));
        captured.body = BodyCapture::complete(br#"{"name":"ada"}"#.to_vec());

        assert_eq!(
            command(&captured),
            "curl 'https://example.com/api' \\\n  \
             -X POST \\\n  \
             -H 'Content-Type: application/json' \\\n  \
             --data-raw '{\"name\":\"ada\"}'"
        );
    }

    #[test]
    fn the_headers_curl_works_out_for_itself_are_left_out() {
        let mut captured = request("POST", "https://example.com/api");
        captured
            .headers
            .push("Host", HeaderValue::Text("example.com".into()));
        captured
            .headers
            .push("Content-Length", HeaderValue::Text("2".into()));
        captured
            .headers
            .push("Connection", HeaderValue::Text("keep-alive".into()));
        captured
            .headers
            .push("Transfer-Encoding", HeaderValue::Text("chunked".into()));
        captured.headers.push(
            "Proxy-Authorization",
            HeaderValue::Text("Basic VEVTVA==".into()),
        );
        captured
            .headers
            .push("Accept", HeaderValue::Text("*/*".into()));
        captured.body = BodyCapture::complete(b"hi".to_vec());

        let written = command(&captured);

        assert!(!written.contains("Host:"), "{written}");
        assert!(!written.contains("Content-Length"), "{written}");
        assert!(!written.contains("Connection"), "{written}");
        assert!(!written.contains("Transfer-Encoding"), "{written}");
        assert!(!written.contains("Proxy-Authorization"), "{written}");
        assert!(written.contains("-H 'Accept: */*'"), "{written}");
    }

    #[test]
    fn a_quote_in_a_value_is_escaped_the_way_a_shell_reads_it() {
        let mut captured = request("POST", "https://example.com/api");
        captured.body = BodyCapture::complete(br#"{"it's":"ada's"}"#.to_vec());

        assert_eq!(
            command(&captured),
            "curl 'https://example.com/api' \\\n  -X POST \\\n  \
             --data-raw '{\"it'\\''s\":\"ada'\\''s\"}'"
        );
    }

    #[test]
    fn spaces_and_shell_characters_are_quoted_rather_than_escaped() {
        let mut captured = request("GET", "https://example.com/search?q=a%20b&r=1");
        captured
            .headers
            .push("X-Note", HeaderValue::Text("a b; c | d $(e) `f`".into()));

        let written = command(&captured);

        assert!(
            written.contains("-H 'X-Note: a b; c | d $(e) `f`'"),
            "{written}"
        );
    }

    #[test]
    fn a_multiline_body_stays_inside_one_quoted_string() {
        let mut captured = request("POST", "https://example.com/api");
        captured.body = BodyCapture::complete(b"{\n  \"a\": 1\n}".to_vec());

        assert_eq!(
            command(&captured),
            "curl 'https://example.com/api' \\\n  -X POST \\\n  \
             --data-raw '{\n  \"a\": 1\n}'"
        );
    }

    #[test]
    fn a_body_that_is_not_text_is_written_as_bytes() {
        let mut captured = request("PUT", "https://example.com/blob");
        captured.body = BodyCapture::complete(vec![0x00, 0x1f, 0xff, b'A']);

        assert_eq!(
            command(&captured),
            "curl 'https://example.com/blob' \\\n  -X PUT \\\n  \
             --data-raw $'\\x00\\x1f\\xffA'"
        );
    }

    #[test]
    fn a_header_value_that_is_not_text_is_written_as_bytes() {
        let mut captured = request("GET", "https://example.com/");
        captured
            .headers
            .push("X-Binary", HeaderValue::Bytes(vec![0xc3, 0x28]));

        assert!(
            command(&captured).contains(r"-H $'X-Binary: \xc3('"),
            "{}",
            command(&captured)
        );
    }

    #[test]
    fn a_truncated_body_says_so_above_the_command() {
        let mut captured = request("POST", "https://example.com/upload");
        captured.body = BodyCapture::truncated(b"the first part".to_vec(), 10_000);

        let written = from_captured(&captured);

        assert!(written.truncated);
        assert!(
            written.command.starts_with("# ProxyLab captured only"),
            "{written:?}"
        );
        assert!(written
            .command
            .contains("\ncurl 'https://example.com/upload'"));
    }

    #[test]
    fn a_complete_body_says_nothing_above_the_command() {
        let mut captured = request("POST", "https://example.com/upload");
        captured.body = BodyCapture::complete(b"all of it".to_vec());

        let written = from_captured(&captured);

        assert!(!written.truncated);
        assert!(written.command.starts_with("curl "), "{written:?}");
    }

    #[test]
    fn the_same_request_always_produces_the_same_command() {
        let mut captured = request("POST", "https://example.com/api");
        captured.headers = [
            Header::text("B", "2"),
            Header::text("A", "1"),
            Header::text("B", "3"),
        ]
        .into_iter()
        .collect();
        captured.body = BodyCapture::complete(b"body".to_vec());

        let first = command(&captured);
        for _ in 0..16 {
            assert_eq!(command(&captured), first);
        }
        // The header order is the message's, not alphabetical.
        assert!(first.find("'B: 2'") < first.find("'A: 1'"));
        assert!(first.find("'A: 1'") < first.find("'B: 3'"));
    }

    #[test]
    fn a_generated_command_never_renders_through_debug() {
        let mut captured = request("GET", "https://example.com/");
        captured.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer TEST_TOKEN".into()),
        );

        let rendered = format!("{:?}", from_captured(&captured));

        assert!(!rendered.contains("TEST_TOKEN"), "{rendered}");
        assert!(!rendered.contains("example.com"), "{rendered}");
    }

    #[test]
    fn a_request_that_has_not_been_sent_is_written_the_same_way() {
        let request = RequestSpec::new(
            "POST",
            "https://example.com/api",
            [Header::text("Accept", "application/json")]
                .into_iter()
                .collect(),
            b"hi".to_vec(),
        )
        .expect("a valid request");

        assert_eq!(
            from_request(&request).command,
            "curl 'https://example.com/api' \\\n  -X POST \\\n  \
             -H 'Accept: application/json' \\\n  --data-raw 'hi'"
        );
    }
}
