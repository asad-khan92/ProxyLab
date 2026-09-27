//! Breakpoints and throttling over the socket, as a client speaks them.
//!
//! The protocol by hand, over a real Unix socket, against a real server. What
//! is under test is the wire: the operations a client may send, the shapes
//! they come back in, the two events that are not answers to anything, and
//! the refusals — a body longer than the boundary will pass, a status that is
//! not one, a header a message could not carry.
//!
//! The breakpoints themselves are a fake the test drives: matching a request
//! and holding a connection still are the proxy engine's, and are proved
//! there against a real proxy. Here, what matters is that a pause reaches a
//! client, that a decision reaches the core, and that a client going away is
//! what lets go.

mod support;

use serde_json::{json, Value};
use support::{Client, TestServer};

use proxy_ipc::{
    BreakpointControl, BreakpointDirection, BreakpointId, PauseDecision, PauseId, PausedExchange,
    PausedRequest, PausedResponse, MAX_BREAKPOINT_BODY_BYTES,
};
use proxy_storage::{BodyCapture, HeaderValue, Headers};

/// A request with one header and a short body, as a breakpoint would show it.
fn paused_request(pause: PauseId) -> PausedExchange {
    let mut headers = Headers::new();
    headers.push("Accept", HeaderValue::Text("application/json".into()));

    PausedExchange {
        pause_id: pause,
        breakpoint_id: BreakpointId::next(),
        direction: BreakpointDirection::Request,
        method: "POST".to_owned(),
        url: "https://api.example.com/orders".to_owned(),
        request: Some(PausedRequest {
            method: "POST".to_owned(),
            url: "https://api.example.com/orders".to_owned(),
            headers,
            body: BodyCapture::complete(b"{\"id\":1}".to_vec()),
        }),
        response: None,
        websocket: None,
    }
}

/// A response, as a breakpoint would show it.
fn paused_response(pause: PauseId) -> PausedExchange {
    PausedExchange {
        pause_id: pause,
        breakpoint_id: BreakpointId::next(),
        direction: BreakpointDirection::Response,
        method: "GET".to_owned(),
        url: "https://api.example.com/orders".to_owned(),
        request: None,
        response: Some(PausedResponse {
            status: 200,
            headers: Headers::new(),
            body: BodyCapture::complete(b"upstream".to_vec()),
        }),
        websocket: None,
    }
}

/// A request line naming `operation`, with `parameters` beside it.
fn request(id: &str, operation: &str, parameters: Value) -> Value {
    let mut line = json!({"version": 1, "request_id": id, "type": operation});
    if let Some(fields) = parameters.as_object() {
        for (name, value) in fields {
            line[name] = value.clone();
        }
    }

    line
}

/// A client that is watching for pauses.
async fn watching(server: &TestServer) -> Client {
    let mut client = server.connect().await;
    let response = client
        .request(&request("w", "watch_breakpoints", json!({})))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["watching"], true);

    client
}

/// The next message that is a paused exchange, skipping any release.
async fn next_pause(client: &mut Client) -> Value {
    loop {
        let message = client.next_message().await;
        if message["type"] == "breakpoint_paused" {
            return message;
        }
        assert_eq!(message["type"], "breakpoint_released", "{message}");
    }
}

// MARK: - Configuring

#[tokio::test]
async fn a_breakpoint_is_created_listed_changed_and_removed() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let created = client
        .request(&request(
            "1",
            "create_breakpoint",
            json!({"direction": "request", "matcher": {"host": {"exact": "api.example.com"}}}),
        ))
        .await;
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(created["result"]["breakpoint"]["direction"], "request");
    assert_eq!(created["result"]["breakpoint"]["enabled"], true);
    let id = created["result"]["breakpoint"]["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let listed = client
        .request(&request("2", "list_breakpoints", json!({})))
        .await;
    assert_eq!(listed["result"]["breakpoints"][0]["id"], id.as_str());

    let updated = client
        .request(&request(
            "3",
            "update_breakpoint",
            json!({"breakpoint_id": id, "enabled": false}),
        ))
        .await;
    assert_eq!(updated["result"]["breakpoint"]["enabled"], false);

    let deleted = client
        .request(&request(
            "4",
            "delete_breakpoint",
            json!({"breakpoint_id": id}),
        ))
        .await;
    assert_eq!(deleted["result"]["deleted"], true);
    assert!(breakpoints.list().is_empty());

    server.stop().await;
}

#[tokio::test]
async fn a_breakpoint_that_is_not_there_is_not_found_rather_than_a_failure() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = server.connect().await;
    let absent = BreakpointId::next().to_string();

    let updated = client
        .request(&request(
            "1",
            "update_breakpoint",
            json!({"breakpoint_id": absent, "enabled": false}),
        ))
        .await;
    assert_eq!(updated["ok"], false);
    assert_eq!(updated["error"]["code"], "not_found");

    let deleted = client
        .request(&request(
            "2",
            "delete_breakpoint",
            json!({"breakpoint_id": absent}),
        ))
        .await;
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["result"]["deleted"], false);

    server.stop().await;
}

#[tokio::test]
async fn a_matcher_that_could_match_nothing_is_refused() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request(
            "1",
            "create_breakpoint",
            json!({"direction": "request", "matcher": {"path": {"exact": "no-slash"}}}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_breakpoint");

    server.stop().await;
}

#[tokio::test]
async fn an_id_that_is_not_an_id_is_refused_by_the_kind_of_id_it_is_not() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let breakpoint = client
        .request(&request(
            "1",
            "delete_breakpoint",
            json!({"breakpoint_id": "not-a-uuid"}),
        ))
        .await;
    assert_eq!(breakpoint["error"]["code"], "invalid_breakpoint_id");

    let pause = client
        .request(&request(
            "2",
            "cancel_breakpoint",
            json!({"pause_id": "not-a-uuid"}),
        ))
        .await;
    assert_eq!(pause["error"]["code"], "invalid_pause_id");

    server.stop().await;
}

// MARK: - Watching, and being told

#[tokio::test]
async fn watching_is_what_makes_the_core_count_a_client() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    assert_eq!(breakpoints.watchers(), 0);

    let client = watching(&server).await;
    assert_eq!(breakpoints.watchers(), 1);

    client.disconnect().await;
    // The server notices the client has gone and lets go of its watch.
    wait_for(|| breakpoints.watchers() == 0).await;

    server.stop().await;
}

#[tokio::test]
async fn a_connection_may_only_watch_once() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;

    let again = client
        .request(&request("2", "watch_breakpoints", json!({})))
        .await;

    assert_eq!(again["ok"], false, "{again}");
    assert_eq!(again["error"]["code"], "already_subscribed");

    server.stop().await;
}

#[tokio::test]
async fn a_paused_request_reaches_a_watching_client_whole() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();

    breakpoints.pause(paused_request(pause));
    let event = next_pause(&mut client).await;

    // An event, not an answer: no `ok`, and no request id.
    assert!(event.get("ok").is_none(), "{event}");
    assert!(event.get("request_id").is_none(), "{event}");
    assert_eq!(event["version"], 1);
    assert_eq!(event["paused"]["pause_id"], pause.to_string());
    assert_eq!(event["paused"]["direction"], "request");
    assert_eq!(event["paused"]["request"]["method"], "POST");
    assert_eq!(event["paused"]["request"]["body"]["data"], "eyJpZCI6MX0=");
    assert!(event["paused"].get("response").is_none(), "{event}");

    server.stop().await;
}

#[tokio::test]
async fn a_client_that_is_not_watching_is_told_about_nothing() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    breakpoints.pause(paused_request(PauseId::next()));

    // The next thing this client hears is the answer to what it asked, and
    // not a pause it never signed up for.
    let listed = client
        .request(&request("1", "list_breakpoints", json!({})))
        .await;
    assert_eq!(listed["type"], "list_breakpoints");

    server.stop().await;
}

// MARK: - Deciding

#[tokio::test]
async fn continuing_with_an_edit_carries_it_to_the_core() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();
    breakpoints.pause(paused_request(pause));
    next_pause(&mut client).await;

    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({
                "pause_id": pause.to_string(),
                "request": {
                    "method": "PUT",
                    "url": "https://api.example.com/v2/orders",
                    "headers": [{"name": "X-Edited", "value": {"text": "yes"}}],
                    "body": "ZWRpdGVk",
                },
            }),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["resolved"], true);

    let decisions = breakpoints.decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].0, pause);
    let PauseDecision::Continue {
        request, response, ..
    } = &decisions[0].1
    else {
        panic!("expected a continue: {:?}", decisions[0].1);
    };
    let edit = request.as_ref().expect("a request edit");
    assert_eq!(edit.method.as_deref(), Some("PUT"));
    assert_eq!(
        edit.url.as_deref(),
        Some("https://api.example.com/v2/orders")
    );
    assert_eq!(edit.body.as_deref(), Some(&b"edited"[..]));
    assert_eq!(
        edit.headers
            .as_ref()
            .and_then(|headers| headers.text("x-edited")),
        Some("yes")
    );
    assert!(response.is_none());

    server.stop().await;
}

#[tokio::test]
async fn continuing_with_nothing_is_a_decision_that_changes_nothing() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();
    breakpoints.pause(paused_request(pause));
    next_pause(&mut client).await;

    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({"pause_id": pause.to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(breakpoints.decisions()[0].1, PauseDecision::unchanged());

    server.stop().await;
}

#[tokio::test]
async fn cancelling_carries_a_cancel_to_the_core() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();
    breakpoints.pause(paused_response(pause));
    next_pause(&mut client).await;

    let response = client
        .request(&request(
            "1",
            "cancel_breakpoint",
            json!({"pause_id": pause.to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(breakpoints.decisions()[0].1, PauseDecision::Cancel);

    server.stop().await;
}

#[tokio::test]
async fn everyone_watching_is_told_a_pause_ended() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut deciding = watching(&server).await;
    let mut looking = watching(&server).await;
    let pause = PauseId::next();

    breakpoints.pause(paused_request(pause));
    next_pause(&mut deciding).await;
    next_pause(&mut looking).await;

    deciding
        .request(&request(
            "1",
            "cancel_breakpoint",
            json!({"pause_id": pause.to_string()}),
        ))
        .await;

    // The client that did not decide still has the pause on screen, so it is
    // told to stop showing it.
    let released = looking.next_message().await;
    assert_eq!(released["type"], "breakpoint_released");
    assert_eq!(released["pause_id"], pause.to_string());
    assert_eq!(released["reason"], "cancelled");

    server.stop().await;
}

#[tokio::test]
async fn a_pause_that_is_already_over_is_a_result_and_not_an_error() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();

    let response = client
        .request(&request(
            "1",
            "cancel_breakpoint",
            json!({"pause_id": pause.to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["resolved"], false);

    server.stop().await;
}

#[tokio::test]
async fn the_last_client_going_away_is_what_lets_go_of_what_was_held() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut first = watching(&server).await;
    let second = watching(&server).await;
    breakpoints.pause(paused_request(PauseId::next()));
    next_pause(&mut first).await;
    assert_eq!(breakpoints.pending(), 1);

    // One of two going away releases nothing: somebody is still watching.
    second.disconnect().await;
    wait_for(|| breakpoints.watchers() == 1).await;
    assert_eq!(breakpoints.pending(), 1);

    // The last one going away releases everything.
    first.disconnect().await;
    wait_for(|| breakpoints.watchers() == 0).await;
    assert_eq!(breakpoints.pending(), 0);

    server.stop().await;
}

// MARK: - What an edit may carry

#[tokio::test]
async fn an_edited_body_longer_than_the_boundary_will_pass_is_refused() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();

    // Base64 of one byte more than the limit, which is still well inside the
    // 64 KiB line the server will read: the refusal is about the body, not
    // about the line.
    let oversized = base64_of(&vec![b'x'; MAX_BREAKPOINT_BODY_BYTES + 1]);
    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({"pause_id": pause.to_string(), "request": {"body": oversized}}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("32768"),
        "{response}"
    );
    assert!(breakpoints.decisions().is_empty());

    server.stop().await;
}

#[tokio::test]
async fn a_body_of_exactly_the_limit_is_accepted() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = watching(&server).await;
    let pause = PauseId::next();
    breakpoints.pause(paused_request(pause));
    next_pause(&mut client).await;

    let body = base64_of(&vec![b'x'; MAX_BREAKPOINT_BODY_BYTES]);
    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({"pause_id": pause.to_string(), "request": {"body": body}}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["resolved"], true);

    server.stop().await;
}

#[tokio::test]
async fn an_edited_status_that_is_not_a_status_is_refused() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    for status in [99, 600, 0] {
        let response = client
            .request(&request(
                "1",
                "continue_breakpoint",
                json!({
                    "pause_id": PauseId::next().to_string(),
                    "response": {"status": status},
                }),
            ))
            .await;

        assert_eq!(response["ok"], false, "{status}: {response}");
        assert_eq!(response["error"]["code"], "invalid_parameters");
    }
    assert!(breakpoints.decisions().is_empty());

    server.stop().await;
}

#[tokio::test]
async fn a_header_that_could_split_a_message_in_two_is_refused() {
    let (server, breakpoints, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({
                "pause_id": PauseId::next().to_string(),
                "request": {
                    "headers": [
                        {"name": "X-Smuggled", "value": {"text": "a\r\nGET / HTTP/1.1"}}
                    ]
                },
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");
    assert!(breakpoints.decisions().is_empty());

    server.stop().await;
}

#[tokio::test]
async fn an_edit_for_the_half_a_pause_is_not_for_is_refused() {
    let (server, _, _) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request(
            "1",
            "continue_breakpoint",
            json!({
                "pause_id": PauseId::next().to_string(),
                "request": {"method": "PUT"},
                "response": {"status": 200},
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

// MARK: - Throttling

#[tokio::test]
async fn throttling_is_read_back_as_it_was_set() {
    let (server, _, throttle) = TestServer::breaking().await;
    let mut client = server.connect().await;

    let before = client
        .request(&request("1", "throttle_status", json!({})))
        .await;
    assert_eq!(before["result"]["enabled"], false);

    let applied = client
        .request(&request(
            "2",
            "set_throttle",
            json!({
                "enabled": true,
                "latency_ms": 250,
                "download_bytes_per_second": 131_072,
                "upload_bytes_per_second": 32_768,
            }),
        ))
        .await;
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["result"]["latency_ms"], 250);
    assert_eq!(throttle.applied().download_bytes_per_second, 131_072);

    let after = client
        .request(&request("3", "throttle_status", json!({})))
        .await;
    assert_eq!(after["result"]["upload_bytes_per_second"], 32_768);

    server.stop().await;
}

#[tokio::test]
async fn throttling_nothing_would_finish_at_is_refused() {
    let (server, _, throttle) = TestServer::breaking().await;
    let mut client = server.connect().await;

    for parameters in [
        json!({"enabled": true, "latency_ms": 60_001}),
        json!({"enabled": true, "download_bytes_per_second": 1}),
    ] {
        let response = client
            .request(&request("1", "set_throttle", parameters.clone()))
            .await;

        assert_eq!(response["ok"], false, "{parameters}: {response}");
        assert_eq!(response["error"]["code"], "invalid_parameters");
    }

    assert!(!throttle.applied().enabled);

    server.stop().await;
}

// MARK: - When the server is not serving them

#[tokio::test]
async fn a_server_that_is_not_managing_breakpoints_says_so_rather_than_pretending() {
    // The ordinary test server, started without the two boundaries.
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for operation in ["list_breakpoints", "watch_breakpoints", "throttle_status"] {
        let response = client.request(&request("1", operation, json!({}))).await;

        assert_eq!(response["ok"], false, "{operation}: {response}");
        assert_eq!(response["error"]["code"], "internal", "{operation}");
    }

    server.stop().await;
}

// MARK: - Helpers

/// The base64 the wire carries a byte string as.
fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Waits for `condition`, or fails the test.
///
/// A client going away is noticed by the server's own task rather than by
/// anything this test can await, so this is the one place a poll is the
/// honest thing to do — bounded, so a server that never notices fails
/// instead of hanging.
async fn wait_for(condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("the server never noticed");
}
