//! A pasted cURL command is somebody else's text.
//!
//! These tests exist for one property: importing a command must never do
//! what running it would do. Not part of it, not a harmless-looking part of
//! it, and not the part inside the quotes.
//!
//! Two of them are written as the strongest check available to a test — the
//! effect of the command, if it ran, would be a file on disk, so the test
//! parses the command and then asserts the file is not there.

use std::path::PathBuf;

use proxy_curl::{parse, CurlError, CurlErrorCode};

/// A path under the system temporary directory that nothing else uses.
fn marker(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("proxylab-curl-{name}-{}", std::process::id()))
}

#[test]
fn command_substitution_in_a_url_creates_no_file() {
    let path = marker("substitution");
    let _ = std::fs::remove_file(&path);

    // Inside single quotes, so a shell would pass these characters through
    // to cURL unchanged — and a shell is exactly what ProxyLab is not. The
    // command has a space in it and so is not a URL, which is a refusal;
    // what matters is that the refusal is the only thing that happened.
    let command = format!("curl 'https://example.com/$(touch {})'", path.display());
    let _ = parse(&command);

    assert!(
        !path.exists(),
        "importing a cURL command created {}",
        path.display()
    );
}

#[test]
fn command_substitution_that_is_a_valid_url_stays_text() {
    let request = parse("curl 'https://example.com/$(whoami)'")
        .expect("parses")
        .request;

    // The characters survive as the characters they are, and the request is
    // a request for a path that looks like that.
    assert_eq!(request.url(), "https://example.com/$(whoami)");
    assert_eq!(request.path(), "/$(whoami)");
}

#[test]
fn command_substitution_outside_quotes_creates_no_file_and_is_refused() {
    let path = marker("unquoted");
    let _ = std::fs::remove_file(&path);

    let command = format!("curl https://example.com/$(touch {})", path.display());
    let error = parse(&command).expect_err("is refused");

    assert!(
        !path.exists(),
        "importing a cURL command created {}",
        path.display()
    );
    assert_eq!(error.code(), CurlErrorCode::ShellExpansion);
}

#[test]
fn a_backtick_is_refused_rather_than_run() {
    let error = parse("curl https://example.com/`whoami`").expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::ShellExpansion);
}

#[test]
fn a_variable_is_refused_rather_than_read() {
    for command in [
        r#"curl https://example.com/ -H "Authorization: Bearer $TOKEN""#,
        "curl $URL",
        "curl ${URL}",
        "curl https://example.com/$HOME",
    ] {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        assert_eq!(error.code(), CurlErrorCode::ShellExpansion, "{command}");
    }
}

#[test]
fn a_variable_inside_single_quotes_is_the_text_it_is() {
    let request = parse("curl https://example.com/ -H 'Authorization: Bearer $TOKEN'")
        .expect("parses")
        .request;

    assert_eq!(
        request.headers().text("authorization"),
        Some("Bearer $TOKEN")
    );
}

#[test]
fn a_second_command_after_a_separator_is_refused() {
    for command in [
        "curl https://example.com/; rm -rf /tmp/nothing",
        "curl https://example.com/ && echo hello",
        "curl https://example.com/ || echo hello",
        "curl https://example.com/ | sh",
        "curl https://example.com/ & ",
    ] {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        assert_eq!(error.code(), CurlErrorCode::ShellMetacharacter, "{command}");
    }
}

#[test]
fn a_redirection_is_refused() {
    for command in [
        "curl https://example.com/ > /tmp/proxylab-should-not-exist",
        "curl https://example.com/ < /etc/passwd",
        "curl https://example.com/ 2>/dev/null",
    ] {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        assert_eq!(error.code(), CurlErrorCode::ShellMetacharacter, "{command}");
    }
}

#[test]
fn shell_metacharacters_inside_quotes_are_part_of_the_url() {
    let request = parse("curl 'https://example.com/?a=1&b=2;c=3|d=4'")
        .expect("parses")
        .request;

    assert_eq!(request.url(), "https://example.com/?a=1&b=2;c=3|d=4");
}

#[test]
fn a_body_named_as_a_local_file_is_never_read() {
    // The file exists, and is still not read: this is a refusal by rule and
    // not one that happens to be a missing file.
    let path = std::env::temp_dir().join(format!("proxylab-curl-body-{}", std::process::id()));
    std::fs::write(&path, b"SECRET PAYLOAD").expect("writes the file");

    let command = format!("curl https://example.com/api -d @{}", path.display());
    let error = parse(&command).expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::FileReference);
    assert!(!error.message().contains("SECRET"), "{error}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_header_cannot_carry_a_second_request_into_the_request() {
    // A header value with a line break in it is how a request gets split in
    // two. The lexer will happily produce one; the request model refuses it.
    let command = "curl https://example.com/ -H $'X-Thing: a\\r\\nX-Evil: yes'";
    let error = parse(command).expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::InvalidRequest);
}

#[test]
fn a_url_cannot_carry_a_line_break() {
    let error =
        parse(r"curl $'https://example.com/a\r\nHost: evil.example'").expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::InvalidRequest);
}

#[test]
fn no_error_message_repeats_anything_but_an_option_name() {
    // Every value in this command is something that must not come back out
    // in a message that gets shown, copied, and pasted into a bug report.
    let commands = [
        "curl 'https://example.com/?token=SUPERSECRET' --nope",
        "curl https://example.com/ -H 'Authorization: Bearer SUPERSECRET' --nope",
        "curl https://example.com/ -d 'password=SUPERSECRET' --nope",
        "curl 'https://SUPERSECRET@example.com/' -X 'not a method'",
        "curl 'https://example.com/$(SUPERSECRET)'x --nope",
    ];

    for command in commands {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        assert!(
            !error.message().contains("SUPERSECRET"),
            "{command} produced {error}"
        );
    }
}

#[test]
fn a_command_that_does_not_parse_never_leaves_a_partial_request() {
    // There is no half-built request to be had: parsing either produces one
    // whole checked request or an error, and never both.
    let outcome: Result<_, CurlError> = parse("curl --nope https://example.com/");

    assert!(outcome.is_err());
}
