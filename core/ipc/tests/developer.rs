//! The cURL and replay operations, over a real socket.
//!
//! Every test here connects to a real IPC server on a socket of its own and
//! speaks the protocol by hand, because what is under test is the bytes on
//! the wire. The proxy is not running: a replay reaches a
//! [`RecordingReplayer`] that records what it was asked to send, which is
//! exactly the boundary `proxy-engine` implements. What the engine does with
//! a request once it has one is tested there, against real origin servers.

mod support;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use proxy_storage::{BodyCapture, HeaderValue, TrafficErrorKind};
use serde_json::{json, Value};
use support::{exchange, request, request_with, TestServer};

/// The `result` of a response that worked, or a panic saying what came back.
fn result(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["version"], 1, "{response}");

    &response["result"]
}

/// The `error` of a response that did not work.
fn error(response: &Value) -> &Value {
    assert_eq!(response["ok"], false, "{response}");

    &response["error"]
}

/// A `parse_curl` request for `command`.
fn parse_curl(id: &str, command: &str) -> Value {
    request_with(id, "parse_curl", json!({"curl": command}))
}

/// The headers of a parsed or replayed request, as `name: value` pairs.
fn header_pairs(request: &Value) -> Vec<(String, String)> {
    request["headers"]
        .as_array()
        .expect("headers are an array")
        .iter()
        .map(|header| {
            (
                header["name"].as_str().expect("a name").to_owned(),
                header["value"]["text"]
                    .as_str()
                    .expect("a text value")
                    .to_owned(),
            )
        })
        .collect()
}

// MARK: - parse_curl

#[tokio::test(flavor = "multi_thread")]
async fn a_curl_command_is_read_as_the_request_it_describes() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&parse_curl(
            "1",
            "curl -X POST https://example.com/api \
             -H 'Content-Type: application/json' \
             -H 'Authorization: Bearer TEST_TOKEN' \
             -d '{\"name\":\"Ada\"}'",
        ))
        .await;

    assert_eq!(response["type"], "parse_curl");
    let parsed = &result(&response)["request"];
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["url"], "https://example.com/api");
    assert_eq!(
        header_pairs(parsed),
        [
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("Authorization".to_owned(), "Bearer TEST_TOKEN".to_owned()),
        ]
    );
    assert_eq!(parsed["body"], STANDARD.encode(br#"{"name":"Ada"}"#));
    assert!(result(&response).get("problem").is_none(), "{response}");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parsed_command_carries_what_the_parser_has_to_say_about_it() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&parse_curl(
            "1",
            "curl -k --compressed https://example.com/",
        ))
        .await;

    let notes = result(&response)["notes"]
        .as_array()
        .expect("notes are an array");
    let names: Vec<&str> = notes
        .iter()
        .map(|note| note.as_str().expect("a name"))
        .collect();

    assert!(names.contains(&"compressed"), "{response}");
    assert!(names.contains(&"insecure"), "{response}");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_does_not_parse_is_a_result_and_not_an_error() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&parse_curl("1", "curl --nope https://example.com/"))
        .await;

    // The operation was carried out; the answer is that the command names an
    // option this build does not implement.
    let problem = &result(&response)["problem"];
    assert_eq!(problem["code"], "unsupported_option");
    assert_eq!(problem["option"], "--nope");
    assert!(
        problem["message"]
            .as_str()
            .expect("a message")
            .contains("--nope"),
        "{response}"
    );
    assert!(result(&response).get("request").is_none(), "{response}");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_kind_of_bad_command_comes_back_with_the_code_for_it() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for (command, code) in [
        ("", "empty"),
        ("wget https://example.com/", "not_curl"),
        ("curl 'https://example.com", "unterminated_quote"),
        ("curl https://example.com/$(whoami)", "shell_expansion"),
        ("curl https://example.com/ | sh", "shell_metacharacter"),
        ("curl -X POST", "missing_url"),
        ("curl a https://example.com/ b", "multiple_urls"),
        ("curl https://example.com/ -H 'nope'", "invalid_header"),
        (
            "curl https://example.com/ -d @/etc/passwd",
            "file_reference",
        ),
        ("curl https://example.com/ -u nobody", "invalid_user"),
        ("curl -x 127.0.0.1:1 https://example.com/", "proxy_option"),
        ("curl ftp://example.com/", "invalid_request"),
    ] {
        let response = client.request(&parse_curl("1", command)).await;
        let problem = &result(&response)["problem"];

        assert_eq!(problem["code"], code, "{command:?} gave {response}");
    }

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parse_curl_without_its_parameter_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("1", "parse_curl")).await;

    assert_eq!(error(&response)["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parse_curl_with_a_parameter_this_build_does_not_know_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "parse_curl",
            json!({"curl": "curl https://example.com/", "execute": true}),
        ))
        .await;

    assert_eq!(error(&response)["code"], "invalid_parameters");

    server.stop().await;
}

// MARK: - curl_from_traffic

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_request_is_written_out_as_a_command() {
    let server = TestServer::start().await;
    let mut stored = exchange("example.com", "/api/users", 200);
    stored.request.method = "POST".to_owned();
    stored
        .request
        .headers
        .push("Content-Type", HeaderValue::Text("application/json".into()));
    stored.request.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer TEST_TOKEN".into()),
    );
    stored.request.body = BodyCapture::complete(br#"{"a":1}"#.to_vec());
    let id = server.store(&stored);

    let mut client = server.connect().await;
    let response = client
        .request(&request_with(
            "1",
            "curl_from_traffic",
            json!({"exchange_id": id.as_text()}),
        ))
        .await;

    assert_eq!(response["type"], "curl_from_traffic");
    let generated = result(&response);
    let command = generated["curl"].as_str().expect("a command");

    assert!(
        command.starts_with("curl 'http://example.com/api/users'"),
        "{command}"
    );
    assert!(command.contains("-X POST"), "{command}");
    assert!(
        command.contains("-H 'Content-Type: application/json'"),
        "{command}"
    );
    // Reproduction is the point: the credential is in the command.
    assert!(
        command.contains("-H 'Authorization: Bearer TEST_TOKEN'"),
        "{command}"
    );
    assert!(command.contains(r#"--data-raw '{"a":1}'"#), "{command}");
    assert_eq!(generated["truncated"], false);

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_from_a_truncated_capture_says_so() {
    let server = TestServer::start().await;
    let mut stored = exchange("example.com", "/upload", 200);
    stored.request.method = "POST".to_owned();
    stored.request.body = BodyCapture::truncated(b"the first part".to_vec(), 9_000);
    let id = server.store(&stored);

    let mut client = server.connect().await;
    let response = client
        .request(&request_with(
            "1",
            "curl_from_traffic",
            json!({"exchange_id": id.as_text()}),
        ))
        .await;

    assert_eq!(result(&response)["truncated"], true);
    assert!(
        result(&response)["curl"]
            .as_str()
            .expect("a command")
            .starts_with('#'),
        "{response}"
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_for_an_exchange_that_is_not_there_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "curl_from_traffic",
            json!({"exchange_id": proxy_storage::ExchangeId::next().as_text()}),
        ))
        .await;

    assert_eq!(error(&response)["code"], "not_found");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_for_something_that_is_not_an_exchange_id_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "curl_from_traffic",
            json!({"exchange_id": "not-an-id"}),
        ))
        .await;

    assert_eq!(error(&response)["code"], "invalid_exchange_id");

    server.stop().await;
}

// MARK: - replay_request

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_is_sent_exactly_as_it_was_asked_for() {
    let (server, replayer) = TestServer::replaying().await;
    replayer.answer_with(201);
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({
                "method": "POST",
                "url": "https://example.com/api/users",
                "headers": [
                    {"name": "Content-Type", "value": {"text": "application/json"}},
                    {"name": "Authorization", "value": {"text": "Bearer TEST_TOKEN"}},
                ],
                "body": STANDARD.encode(br#"{"name":"Ada"}"#),
            }),
        ))
        .await;

    assert_eq!(response["type"], "replay_request");
    let outcome = result(&response);
    assert_eq!(outcome["status"], 201);
    assert!(outcome["exchange_id"].is_string(), "{response}");
    assert_eq!(outcome["error"], Value::Null);

    let sent = replayer.only_request();
    assert_eq!(sent.method(), "POST");
    assert_eq!(sent.url(), "https://example.com/api/users");
    assert_eq!(
        sent.headers().text("content-type"),
        Some("application/json")
    );
    assert_eq!(
        sent.headers().text("authorization"),
        Some("Bearer TEST_TOKEN")
    );
    assert_eq!(sent.body(), br#"{"name":"Ada"}"#);

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_with_no_headers_and_no_body_is_a_get() {
    let (server, replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({"method": "GET", "url": "http://example.com/health"}),
        ))
        .await;

    assert_eq!(result(&response)["status"], 200);

    let sent = replayer.only_request();
    assert!(sent.headers().is_empty());
    assert!(sent.body().is_empty());

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_replay_comes_back_with_an_exchange_id_of_its_own() {
    let (server, _replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let replay = json!({"method": "GET", "url": "http://example.com/"});
    let first = client
        .request(&request_with("1", "replay_request", replay.clone()))
        .await;
    let second = client
        .request(&request_with("2", "replay_request", replay))
        .await;

    assert_ne!(
        result(&first)["exchange_id"],
        result(&second)["exchange_id"]
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_that_failed_says_why_and_is_still_an_exchange() {
    let (server, replayer) = TestServer::replaying().await;
    replayer.fail_with(TrafficErrorKind::UpstreamConnect);
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({"method": "GET", "url": "http://example.com/"}),
        ))
        .await;

    // Not an IPC error: the request was replayed, and what came back was a
    // failure.
    let outcome = result(&response);
    assert_eq!(outcome["status"], Value::Null);
    assert_eq!(outcome["error"]["kind"], "upstream_connect");
    assert!(outcome["exchange_id"].is_string(), "{response}");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_of_something_that_is_not_a_request_is_refused() {
    let (server, replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    for parameters in [
        json!({"method": "GET", "url": "ftp://example.com/"}),
        json!({"method": "GET", "url": "/relative"}),
        json!({"method": "GET", "url": ""}),
        json!({"method": "GET POST", "url": "http://example.com/"}),
        json!({"method": "GET", "url": "https://ada:secret@example.com/"}),
        json!({
            "method": "GET",
            "url": "http://example.com/",
            "headers": [{"name": "Bad Name", "value": {"text": "x"}}],
        }),
        json!({
            "method": "GET",
            "url": "http://example.com/",
            "headers": [{"name": "X-Thing", "value": {"text": "a\r\nX-Evil: yes"}}],
        }),
        json!({
            "method": "GET",
            "url": "http://example.com/",
            "headers": [{"name": "Proxy-Authorization", "value": {"text": "Basic VEVTVA=="}}],
        }),
    ] {
        let response = client
            .request(&request_with("1", "replay_request", parameters.clone()))
            .await;

        assert_eq!(
            error(&response)["code"],
            "invalid_request",
            "{parameters} gave {response}"
        );
    }

    // Nothing was sent for any of them.
    assert!(replayer.requests().is_empty());

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_with_no_url_is_refused_as_a_parameter_problem() {
    let (server, _replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({"method": "GET"}),
        ))
        .await;

    assert_eq!(error(&response)["code"], "invalid_request");

    let unknown = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({"method": "GET", "url": "http://example.com/", "follow": true}),
        ))
        .await;

    assert_eq!(error(&unknown)["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_is_not_sending_requests_says_so() {
    // The server the other tests use has no replayer, which is what a core
    // started without a proxy to send through looks like.
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "replay_request",
            json!({"method": "GET", "url": "http://example.com/"}),
        ))
        .await;

    assert_eq!(error(&response)["code"], "internal");

    server.stop().await;
}

// MARK: - The three of them together

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_request_can_be_copied_imported_and_sent_again() {
    let (server, replayer) = TestServer::replaying().await;

    let mut stored = exchange("example.com", "/v1/orders", 200);
    stored.request.method = "POST".to_owned();
    stored.request.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer TEST_TOKEN".into()),
    );
    stored
        .request
        .headers
        .push("Cookie", HeaderValue::Text("session=TEST_SESSION".into()));
    stored
        .request
        .headers
        .push("Content-Type", HeaderValue::Text("application/json".into()));
    stored.request.body = BodyCapture::complete(br#"{"item":42}"#.to_vec());
    let id = server.store(&stored);

    let mut client = server.connect().await;

    // Copy as cURL.
    let copied = client
        .request(&request_with(
            "1",
            "curl_from_traffic",
            json!({"exchange_id": id.as_text()}),
        ))
        .await;
    let command = result(&copied)["curl"]
        .as_str()
        .expect("a command")
        .to_owned();

    // Import it again.
    let imported = client.request(&parse_curl("2", &command)).await;
    let parsed = result(&imported)["request"].clone();
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["url"], "http://example.com/v1/orders");

    // Send it.
    let replayed = client
        .request(&request_with("3", "replay_request", parsed))
        .await;
    assert_eq!(result(&replayed)["status"], 200);

    // Every credential survived all three hops, exactly as it was.
    let sent = replayer.only_request();
    assert_eq!(
        sent.headers().text("authorization"),
        Some("Bearer TEST_TOKEN")
    );
    assert_eq!(sent.headers().text("cookie"), Some("session=TEST_SESSION"));
    assert_eq!(sent.body(), br#"{"item":42}"#);

    // And the exchange it came from is untouched.
    let original = server
        .storage
        .get(id)
        .expect("the store answers")
        .expect("the original is still stored");
    assert_eq!(original, stored);
    assert_ne!(
        result(&replayed)["exchange_id"].as_str().expect("an id"),
        id.as_text()
    );

    server.stop().await;
}

// MARK: - Nothing leaks, and nothing old breaks

#[tokio::test(flavor = "multi_thread")]
async fn no_refusal_of_any_of_these_operations_repeats_a_credential() {
    let (server, _replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let responses = vec![
        client
            .request(&parse_curl(
                "1",
                "curl https://example.com/ -H 'Authorization: Bearer TEST_TOKEN' --nope",
            ))
            .await,
        client
            .request(&request_with(
                "2",
                "replay_request",
                json!({
                    "method": "GET",
                    "url": "https://ada:TEST_PASSWORD@example.com/",
                    "headers": [{"name": "Cookie", "value": {"text": "session=TEST_SESSION"}}],
                }),
            ))
            .await,
        client
            .request(&request_with(
                "3",
                "curl_from_traffic",
                json!({"exchange_id": "TEST_TOKEN"}),
            ))
            .await,
    ];

    for response in responses {
        let rendered = response.to_string();
        for secret in ["TEST_TOKEN", "TEST_PASSWORD", "TEST_SESSION"] {
            // The parse_curl answer is allowed to name the option; nothing
            // anywhere is allowed to name a value.
            assert!(
                !rendered.contains(secret),
                "{secret} came back in {rendered}"
            );
        }
    }

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operations_that_were_here_before_still_work() {
    let (server, _replayer) = TestServer::replaying().await;
    let id = server.store(&exchange("example.com", "/old", 200));
    let mut client = server.connect().await;

    let listed = client.request(&request("1", "list_traffic")).await;
    assert_eq!(listed["type"], "list_traffic");
    assert_eq!(result(&listed)["total"], 1);

    let got = client
        .request(&request_with(
            "2",
            "get_traffic",
            json!({"exchange_id": id.as_text()}),
        ))
        .await;
    assert_eq!(got["type"], "get_traffic");

    let rules = client.request(&request("3", "list_rules")).await;
    assert_eq!(rules["type"], "list_rules");

    let subscribed = client.request(&request("4", "subscribe")).await;
    assert_eq!(subscribed["type"], "subscribe");

    let cleared = client.request(&request("5", "clear_traffic")).await;
    assert_eq!(result(&cleared)["removed"], 1);

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_protocol_is_still_version_one_and_still_refuses_another() {
    let (server, _replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let response = client
        .request(&json!({
            "version": 2,
            "request_id": "1",
            "type": "parse_curl",
            "curl": "curl https://example.com/",
        }))
        .await;

    assert_eq!(response["version"], 1);
    assert_eq!(error(&response)["code"], "unsupported_version");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_operation_this_build_does_not_have_is_still_unknown() {
    let (server, _replayer) = TestServer::replaying().await;
    let mut client = server.connect().await;

    let response = client.request(&request("1", "execute_curl")).await;

    assert_eq!(error(&response)["code"], "unknown_operation");

    server.stop().await;
}
