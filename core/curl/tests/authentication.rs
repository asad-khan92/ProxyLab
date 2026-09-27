//! Authentication is request data, and nothing else.
//!
//! ProxyLab has no login, no OAuth flow, no token refresh, no credential
//! store, and no idea what any of these values mean. An `Authorization`
//! header is a header; a `Cookie` is a header; an API key in a header of a
//! service's own naming is a header. They are copied out exactly, imported
//! exactly, and sent exactly, and an expired one comes back from the server
//! as whatever the server says about it.
//!
//! These tests are written with values that are obviously not credentials —
//! `TEST_TOKEN`, `TEST_SESSION` — so that nothing in this repository, its
//! logs, or its CI output ever looks like a real one.

use proxy_curl::{from_captured, from_request, parse, CurlErrorCode, CurlNote};
use proxy_http::{RequestSpec, RequestSpecError};
use proxy_storage::{BodyCapture, CapturedRequest, HeaderValue, Headers, HttpVersion};

/// The headers of the request a command describes.
fn imported(command: &str) -> Headers {
    parse(command)
        .unwrap_or_else(|error| panic!("{command:?} did not parse: {error}"))
        .request
        .headers()
        .clone()
}

#[test]
fn an_authorization_header_survives_being_written_and_read() {
    let headers = imported("curl https://example.com/ -H 'Authorization: Bearer TEST_TOKEN'");

    assert_eq!(headers.text("authorization"), Some("Bearer TEST_TOKEN"));
}

#[test]
fn a_cookie_survives_being_written_and_read() {
    let headers =
        imported("curl https://example.com/ -H 'Cookie: session=TEST_SESSION; theme=dark'");

    assert_eq!(
        headers.text("cookie"),
        Some("session=TEST_SESSION; theme=dark")
    );
}

#[test]
fn an_api_key_header_survives_being_written_and_read() {
    for (name, value) in [
        ("X-API-Key", "TEST_API_KEY"),
        ("X-Auth-Token", "TEST_AUTH_TOKEN"),
        ("X-Acme-Signature", "TEST_SIGNATURE"),
    ] {
        let headers = imported(&format!("curl https://example.com/ -H '{name}: {value}'"));

        assert_eq!(headers.text(name), Some(value), "{name}");
    }
}

#[test]
fn every_authentication_header_survives_the_whole_round_trip() {
    let original = RequestSpec::new(
        "POST",
        "https://api.example.com/v1/thing",
        [
            proxy_storage::Header::text("Authorization", "Bearer TEST_TOKEN"),
            proxy_storage::Header::text("Cookie", "session=TEST_SESSION"),
            proxy_storage::Header::text("X-API-Key", "TEST_API_KEY"),
            proxy_storage::Header::text("X-Auth-Token", "TEST_AUTH_TOKEN"),
            proxy_storage::Header::text("Content-Type", "application/json"),
        ]
        .into_iter()
        .collect(),
        br#"{"a":1}"#.to_vec(),
    )
    .expect("a valid request");

    let command = from_request(&original).command;
    let back = parse(&command)
        .expect("the generated command parses")
        .request;

    assert_eq!(back, original);
}

#[test]
fn basic_authentication_becomes_the_header_curl_would_send() {
    let headers = imported("curl https://example.com/ -u 'TEST_USER:TEST_PASSWORD'");

    // base64("TEST_USER:TEST_PASSWORD")
    assert_eq!(
        headers.text("authorization"),
        Some("Basic VEVTVF9VU0VSOlRFU1RfUEFTU1dPUkQ=")
    );
}

#[test]
fn basic_authentication_with_an_empty_password_is_the_common_api_key_form() {
    let headers = imported("curl https://example.com/ --user 'TEST_API_KEY:'");

    // base64("TEST_API_KEY:")
    assert_eq!(
        headers.text("authorization"),
        Some("Basic VEVTVF9BUElfS0VZOg==")
    );
}

#[test]
fn a_user_with_no_password_is_refused_rather_than_guessed_at() {
    // cURL would ask at the terminal. ProxyLab has none, and sending an
    // empty password nobody wrote would be sending a different credential.
    let error = parse("curl https://example.com/ -u TEST_USER").expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::InvalidUser);
    assert!(!error.message().contains("TEST_USER"), "{error}");
}

#[test]
fn an_authorization_header_the_command_names_wins_over_user() {
    let headers =
        imported("curl https://example.com/ -u 'a:b' -H 'Authorization: Bearer TEST_TOKEN'");

    let values: Vec<&str> = headers
        .get_all("authorization")
        .filter_map(HeaderValue::as_text)
        .collect();
    assert_eq!(values, ["Bearer TEST_TOKEN"]);
}

#[test]
fn credentials_in_a_url_move_into_a_header_and_out_of_the_url() {
    let parsed = parse("curl https://TEST_USER:TEST_PASSWORD@example.com/api").expect("parses");

    assert_eq!(parsed.request.url(), "https://example.com/api");
    assert_eq!(
        parsed.request.headers().text("authorization"),
        Some("Basic VEVTVF9VU0VSOlRFU1RfUEFTU1dPUkQ=")
    );
    assert!(parsed.notes.contains(&CurlNote::CredentialsMoved));
    // The URL is the one thing that is logged and shown in a list, so this
    // is the assertion that matters most here.
    assert!(!parsed.request.url().contains("TEST_PASSWORD"));
}

#[test]
fn a_user_option_wins_over_credentials_in_the_url() {
    let parsed =
        parse("curl https://from-url:x@example.com/ -u 'TEST_USER:TEST_PASSWORD'").expect("parses");

    assert_eq!(
        parsed.request.headers().text("authorization"),
        Some("Basic VEVTVF9VU0VSOlRFU1RfUEFTU1dPUkQ=")
    );
}

// MARK: - Proxy authentication is a different thing

#[test]
fn proxy_authorization_is_refused_on_import() {
    // Authentication to an upstream proxy. ProxyLab chains to none, so
    // importing this would be importing a request that quietly stopped
    // carrying a credential.
    let error = parse("curl https://example.com/ -H 'Proxy-Authorization: Basic VEVTVA=='")
        .expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::InvalidRequest);
    assert_eq!(
        error.reason(),
        RequestSpecError::ProxyAuthentication.reason()
    );
}

#[test]
fn proxy_authorization_is_not_written_into_a_copied_command() {
    // It described the hop to ProxyLab, not the request to the origin, so it
    // is not part of reproducing the request — and leaving it out is what
    // makes a copied command one that can be imported again.
    let mut captured = CapturedRequest::new("GET", "https://example.com/", HttpVersion::Http11);
    captured.headers.push(
        "Proxy-Authorization",
        HeaderValue::Text("Basic VEVTVA==".into()),
    );
    captured.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer TEST_TOKEN".into()),
    );

    let command = from_captured(&captured).command;

    assert!(!command.contains("Proxy-Authorization"), "{command}");
    assert!(
        command.contains("Authorization: Bearer TEST_TOKEN"),
        "{command}"
    );
    assert!(parse(&command).is_ok(), "{command}");
}

// MARK: - Nothing is ever logged

#[test]
fn no_debug_rendering_anywhere_shows_a_credential() {
    let mut captured = CapturedRequest::new("POST", "https://example.com/api", HttpVersion::Http11);
    captured.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer TEST_TOKEN".into()),
    );
    captured
        .headers
        .push("Cookie", HeaderValue::Text("session=TEST_SESSION".into()));
    captured.body = BodyCapture::complete(br#"{"password":"TEST_PASSWORD"}"#.to_vec());

    let written = from_captured(&captured);
    let parsed = parse(&written.command).expect("parses");

    let renderings = [
        format!("{written:?}"),
        format!("{:?}", parsed.request),
        format!("{:?}", captured),
        format!("{:?}", captured.headers),
    ];

    for rendered in renderings {
        for secret in ["TEST_TOKEN", "TEST_SESSION", "TEST_PASSWORD"] {
            assert!(
                !rendered.contains(secret),
                "{secret} appeared in {rendered}"
            );
        }
    }
}

#[test]
fn no_parse_failure_reports_a_credential() {
    let commands = [
        "curl https://example.com/ -H 'Authorization: Bearer TEST_TOKEN' --nope",
        "curl https://example.com/ -H 'Cookie: session=TEST_SESSION' --nope",
        "curl https://TEST_USER:TEST_PASSWORD@example.com/ --nope",
        "curl https://example.com/ -u TEST_USER",
    ];

    for command in commands {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        for secret in ["TEST_TOKEN", "TEST_SESSION", "TEST_PASSWORD", "TEST_USER"] {
            assert!(
                !error.message().contains(secret),
                "{command} reported {secret}: {error}"
            );
        }
    }
}
