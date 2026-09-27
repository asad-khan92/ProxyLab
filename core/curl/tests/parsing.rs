//! Reading the cURL commands developers actually paste.
//!
//! Every command here is one a person would have copied out of a browser's
//! network panel, a README, or a colleague's message. What is asserted is the
//! request each one describes — method, URL, headers, body — because that is
//! the whole of what importing a command means.

use proxy_curl::{parse, CurlError, CurlErrorCode, CurlNote};
use proxy_http::RequestSpecError;
use proxy_storage::HeaderValue;

/// The request a command describes, or a panic naming what went wrong.
fn request(command: &str) -> proxy_http::RequestSpec {
    parse(command)
        .unwrap_or_else(|error| panic!("{command:?} did not parse: {error}"))
        .request
}

/// Every header of `name`, in order, as text.
fn values(request: &proxy_http::RequestSpec, name: &str) -> Vec<String> {
    request
        .headers()
        .get_all(name)
        .filter_map(HeaderValue::as_text)
        .map(str::to_owned)
        .collect()
}

/// The body as text.
fn body(request: &proxy_http::RequestSpec) -> String {
    String::from_utf8(request.body().to_vec()).expect("a text body")
}

// MARK: - The shapes of a command

#[test]
fn a_bare_url_is_a_get() {
    let request = request("curl https://example.com");

    assert_eq!(request.method(), "GET");
    assert_eq!(request.url(), "https://example.com/");
    assert!(request.headers().is_empty());
    assert!(request.body().is_empty());
}

#[test]
fn an_explicit_method_is_used() {
    assert_eq!(request("curl -X GET https://example.com").method(), "GET");
    assert_eq!(
        request("curl --request DELETE https://example.com/1").method(),
        "DELETE"
    );
    assert_eq!(
        request("curl --request=PATCH https://example.com/1").method(),
        "PATCH"
    );
    // cURL lets a short option and its value be written together.
    assert_eq!(request("curl -XPUT https://example.com/1").method(), "PUT");
}

#[test]
fn an_unusual_method_is_not_refused() {
    assert_eq!(
        request("curl -X PURGE https://example.com/").method(),
        "PURGE"
    );
}

#[test]
fn the_post_a_browser_copies_out() {
    let command = r#"curl -X POST https://example.com/api \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer TEST_TOKEN' \
  -d '{"name":"John"}'"#;

    let request = request(command);

    assert_eq!(request.method(), "POST");
    assert_eq!(request.url(), "https://example.com/api");
    assert_eq!(values(&request, "content-type"), ["application/json"]);
    assert_eq!(values(&request, "authorization"), ["Bearer TEST_TOKEN"]);
    assert_eq!(body(&request), r#"{"name":"John"}"#);
}

#[test]
fn data_without_a_method_is_a_post() {
    let request = request("curl https://example.com/api -d 'a=1'");

    assert_eq!(request.method(), "POST");
    assert_eq!(body(&request), "a=1");
}

#[test]
fn a_url_given_with_the_url_option_is_the_url() {
    let request = request("curl --url https://example.com/api");

    assert_eq!(request.url(), "https://example.com/api");
}

#[test]
fn a_quoted_url_is_the_url_without_its_quotes() {
    assert_eq!(
        request("curl 'https://example.com/a?b=c&d=e'").url(),
        "https://example.com/a?b=c&d=e"
    );
    assert_eq!(
        request(r#"curl "https://example.com/a?b=c""#).url(),
        "https://example.com/a?b=c"
    );
}

#[test]
fn a_command_spread_over_lines_is_one_command() {
    let command = "curl \\\n  'https://example.com/api' \\\n  -H 'Accept: */*' \\\n  -X POST";
    let request = request(command);

    assert_eq!(request.method(), "POST");
    assert_eq!(values(&request, "accept"), ["*/*"]);
}

#[test]
fn escaped_quotes_inside_a_body_survive() {
    let request = request(r#"curl https://example.com/api -d "{\"a\":\"b\"}""#);

    assert_eq!(body(&request), r#"{"a":"b"}"#);
}

// MARK: - Headers

#[test]
fn a_header_is_split_at_its_first_colon() {
    let request = request("curl https://example.com -H 'X-When: 12:30:00'");

    assert_eq!(values(&request, "x-when"), ["12:30:00"]);
}

#[test]
fn repeated_headers_are_all_kept_in_order() {
    let command = "curl https://example.com \
                   -H 'X-Trace: one' -H 'X-Trace: two' -H 'X-Trace: three'";

    assert_eq!(
        values(&request(command), "x-trace"),
        ["one", "two", "three"]
    );
}

#[test]
fn the_whitespace_around_a_header_value_is_not_part_of_it() {
    let request = request("curl https://example.com -H 'Accept:    application/json   '");

    assert_eq!(values(&request, "accept"), ["application/json"]);
}

#[test]
fn a_header_written_with_a_semicolon_has_an_empty_value() {
    let request = request("curl https://example.com -H 'X-Empty;'");

    assert_eq!(values(&request, "x-empty"), [""]);
}

#[test]
fn a_header_with_nothing_after_its_colon_removes_rather_than_adds() {
    // cURL's own meaning: it takes a header out of what cURL would have
    // sent. ProxyLab adds none, so there is nothing to take out.
    let request = request("curl https://example.com -H 'Accept:'");

    assert!(request.headers().is_empty());
}

#[test]
fn a_header_that_is_not_name_and_value_is_refused() {
    assert_eq!(
        parse("curl https://example.com -H 'not a header'"),
        Err(CurlError::InvalidHeader)
    );
    assert_eq!(
        parse("curl https://example.com -H 'Bad Name: value'"),
        Err(CurlError::InvalidHeader)
    );
    assert_eq!(
        parse("curl https://example.com -H ': value'"),
        Err(CurlError::InvalidHeader)
    );
}

// MARK: - Bodies

#[test]
fn every_data_option_produces_a_body() {
    for option in ["-d", "--data", "--data-raw", "--data-binary"] {
        let request = request(&format!("curl https://example.com/api {option} 'a=1'"));

        assert_eq!(body(&request), "a=1", "{option}");
        assert_eq!(request.method(), "POST", "{option}");
    }
}

#[test]
fn repeated_data_arguments_are_joined_the_way_curl_joins_them() {
    let request = request("curl https://example.com/api -d 'a=1' -d 'b=2' --data 'c=3'");

    assert_eq!(body(&request), "a=1&b=2&c=3");
}

#[test]
fn data_raw_does_not_treat_an_at_sign_as_a_file() {
    let request = request("curl https://example.com/api --data-raw '@not-a-file'");

    assert_eq!(body(&request), "@not-a-file");
}

#[test]
fn data_binary_keeps_the_bytes_it_is_given() {
    let request = request(r"curl https://example.com/blob --data-binary $'\x00\x01\xff'");

    assert_eq!(request.body(), [0x00, 0x01, 0xff]);
}

#[test]
fn curls_own_content_type_is_added_for_a_body_that_names_none() {
    let parsed = parse("curl https://example.com/api -d 'a=1'").expect("parses");

    assert_eq!(
        parsed
            .request
            .headers()
            .text("content-type")
            .expect("a content type"),
        "application/x-www-form-urlencoded"
    );
    assert!(parsed.notes.contains(&CurlNote::FormContentType));
}

#[test]
fn a_content_type_the_command_names_is_not_replaced() {
    let parsed =
        parse("curl https://example.com/api -H 'Content-Type: application/json' -d '{\"a\":1}'")
            .expect("parses");

    assert_eq!(
        values(&parsed.request, "content-type"),
        ["application/json"]
    );
    assert!(!parsed.notes.contains(&CurlNote::FormContentType));
}

// MARK: - The get option

#[test]
fn the_get_option_puts_the_data_in_the_query() {
    let request = request("curl -G https://example.com/search -d 'q=rust' -d 'page=2'");

    assert_eq!(request.method(), "GET");
    assert_eq!(request.url(), "https://example.com/search?q=rust&page=2");
    assert!(request.body().is_empty());
}

#[test]
fn the_get_option_appends_to_a_query_that_is_already_there() {
    let request = request("curl --get 'https://example.com/search?a=1' -d 'b=2'");

    assert_eq!(request.url(), "https://example.com/search?a=1&b=2");
}

#[test]
fn the_get_option_with_an_explicit_method_keeps_the_method() {
    let request = request("curl -G -X HEAD https://example.com/search -d 'q=1'");

    assert_eq!(request.method(), "HEAD");
    assert_eq!(request.url(), "https://example.com/search?q=1");
}

// MARK: - The rest of the supported options

#[test]
fn compressed_becomes_the_header_curl_would_send() {
    let parsed = parse("curl --compressed https://example.com/").expect("parses");

    assert_eq!(
        parsed.request.headers().text("accept-encoding"),
        Some("gzip, deflate")
    );
    assert!(parsed.notes.contains(&CurlNote::Compressed));
}

#[test]
fn compressed_does_not_replace_an_accept_encoding_the_command_names() {
    let parsed =
        parse("curl --compressed https://example.com/ -H 'Accept-Encoding: br'").expect("parses");

    assert_eq!(values(&parsed.request, "accept-encoding"), ["br"]);
    assert!(parsed.notes.contains(&CurlNote::Compressed));
}

#[test]
fn insecure_is_accepted_and_reported_rather_than_applied() {
    for command in [
        "curl -k https://example.com/",
        "curl --insecure https://example.com/",
    ] {
        let parsed = parse(command).expect("parses");

        assert_eq!(parsed.request.url(), "https://example.com/");
        assert!(parsed.notes.contains(&CurlNote::Insecure), "{command}");
    }
}

#[test]
fn a_user_agent_becomes_the_header() {
    for command in [
        "curl -A 'ProxyLab/1.0' https://example.com/",
        "curl --user-agent 'ProxyLab/1.0' https://example.com/",
    ] {
        assert_eq!(
            request(command).headers().text("user-agent"),
            Some("ProxyLab/1.0"),
            "{command}"
        );
    }
}

#[test]
fn a_user_agent_header_the_command_names_wins() {
    let request = request("curl -A 'from-A' -H 'User-Agent: from-H' https://example.com/");

    assert_eq!(values(&request, "user-agent"), ["from-H"]);
}

#[test]
fn options_that_only_change_what_curl_prints_are_ignored_and_reported() {
    let parsed = parse("curl -sS --silent --verbose -i https://example.com/").expect("parses");

    assert_eq!(parsed.request.url(), "https://example.com/");
    assert!(parsed.request.headers().is_empty());
    assert!(parsed.notes.contains(&CurlNote::OutputOnly));
}

#[test]
fn a_path_to_curl_is_still_curl() {
    assert_eq!(
        request("/usr/bin/curl https://example.com/").url(),
        "https://example.com/"
    );
}

// MARK: - What is refused

#[test]
fn an_unsupported_option_names_itself() {
    let error = parse("curl --foo https://example.com/").expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::UnsupportedOption);
    assert_eq!(error.option(), Some("--foo"));
    assert!(error.message().contains("--foo"), "{error}");
}

#[test]
fn an_option_that_would_change_the_request_is_refused_rather_than_dropped() {
    for option in ["-L", "--location", "--data-urlencode 'a=1'", "--form 'a=1'"] {
        let error = parse(&format!("curl https://example.com/ {option}"))
            .expect_err(&format!("{option} is refused"));

        assert_eq!(error.code(), CurlErrorCode::UnsupportedOption, "{option}");
    }
}

#[test]
fn an_option_that_needs_an_upstream_proxy_is_refused_by_name() {
    for option in ["-x", "--proxy", "--proxy-user", "--preproxy", "--socks5"] {
        let error = parse(&format!(
            "curl https://example.com/ {option} 127.0.0.1:8080"
        ))
        .expect_err(&format!("{option} is refused"));

        assert_eq!(error.code(), CurlErrorCode::ProxyOption, "{option}");
        assert!(error.message().contains("upstream proxy"), "{error}");
    }
}

#[test]
fn an_option_with_no_value_is_refused() {
    let error = parse("curl https://example.com/ -H").expect_err("is refused");

    assert_eq!(error.code(), CurlErrorCode::MissingOptionValue);
    assert_eq!(error.option(), Some("-H"));
}

#[test]
fn a_command_with_no_url_is_refused() {
    assert_eq!(parse("curl -X POST"), Err(CurlError::MissingUrl));
    assert_eq!(parse("curl"), Err(CurlError::MissingUrl));
}

#[test]
fn a_command_with_two_urls_is_refused() {
    assert_eq!(
        parse("curl https://example.com/ https://other.example/"),
        Err(CurlError::MultipleUrls)
    );
    assert_eq!(
        parse("curl --url https://example.com/ https://other.example/"),
        Err(CurlError::MultipleUrls)
    );
}

#[test]
fn a_url_that_is_not_one_is_refused_by_the_request_model() {
    assert_eq!(
        parse("curl ftp://example.com/file"),
        Err(CurlError::Request(RequestSpecError::UnsupportedScheme))
    );
    assert_eq!(
        parse("curl /api/users"),
        Err(CurlError::Request(RequestSpecError::RelativeUrl))
    );
}

#[test]
fn an_invalid_method_is_refused_by_the_request_model() {
    assert_eq!(
        parse("curl -X 'GET POST' https://example.com/"),
        Err(CurlError::Request(RequestSpecError::InvalidMethod))
    );
}

#[test]
fn nothing_at_all_is_refused() {
    assert_eq!(parse(""), Err(CurlError::Empty));
    assert_eq!(parse("   \n  "), Err(CurlError::Empty));
}

#[test]
fn something_that_is_not_a_curl_command_is_refused() {
    assert_eq!(parse("wget https://example.com/"), Err(CurlError::NotCurl));
    assert_eq!(parse("https://example.com/"), Err(CurlError::NotCurl));
}

#[test]
fn a_command_that_does_not_finish_is_refused() {
    assert_eq!(
        parse("curl 'https://example.com"),
        Err(CurlError::UnterminatedQuote)
    );
}

#[test]
fn a_lone_dash_as_a_data_value_is_the_character_it_is() {
    // cURL reads standard input for `@-`, not for `-`: a `-d -` body is the
    // one character it looks like, and nothing is read.
    let request = request("curl https://example.com/api -d -");

    assert_eq!(body(&request), "-");
}

#[test]
fn a_body_read_from_a_file_is_refused_with_a_reason() {
    for command in [
        "curl https://example.com/api -d @payload.json",
        "curl https://example.com/api --data @/etc/passwd",
        "curl https://example.com/api --data-binary @-",
        "curl https://example.com/api -",
    ] {
        let error = parse(command).expect_err(&format!("{command} is refused"));

        assert_eq!(error.code(), CurlErrorCode::FileReference, "{command}");
        assert!(error.message().contains("--data-raw"), "{error}");
    }
}
