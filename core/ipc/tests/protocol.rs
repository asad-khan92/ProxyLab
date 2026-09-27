//! What the server says to a client that gets the protocol wrong.
//!
//! Every test here sends real bytes over a real socket, because the point is
//! what survives the wire rather than what the parser does in isolation.

mod support;

use support::{request, request_with, Client, TestServer};

use serde_json::json;

/// The error code of a response, which must be a failed one.
fn error_code(response: &serde_json::Value) -> String {
    assert_eq!(response["ok"], false, "expected a failure: {response}");

    response["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("an error has a code: {response}"))
        .to_owned()
}

#[tokio::test]
async fn a_valid_request_is_answered_with_a_valid_response() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "clear_traffic")).await;

    assert_eq!(response["version"], 1);
    assert_eq!(response["request_id"], "r1");
    assert_eq!(response["ok"], true);
    assert_eq!(response["type"], "clear_traffic");
    assert_eq!(response["result"]["removed"], 0);
    assert!(response.get("error").is_none(), "{response}");

    server.stop().await;
}

#[tokio::test]
async fn a_response_carries_the_request_id_it_is_answering() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Several requests in flight at once: the ids are what tells the answers
    // apart, and they must come back on the right ones.
    for id in ["first", "second", "third"] {
        client.send(&request(id, "clear_traffic")).await;
    }

    for id in ["first", "second", "third"] {
        assert_eq!(client.next_message().await["request_id"], id);
    }

    server.stop().await;
}

#[tokio::test]
async fn a_line_that_is_not_json_is_refused_without_closing_the_connection() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request_raw("this is not json").await;

    assert_eq!(error_code(&response), "invalid_json");
    assert_eq!(response["request_id"], serde_json::Value::Null);

    // Still usable: one bad line is not the end of a conversation.
    let after = client.request(&request("r1", "clear_traffic")).await;
    assert_eq!(after["ok"], true);

    server.stop().await;
}

#[tokio::test]
async fn a_request_naming_another_protocol_version_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&json!({"version": 2, "request_id": "r1", "type": "clear_traffic"}))
        .await;

    assert_eq!(error_code(&response), "unsupported_version");
    // Still correlated, so a client knows which of its requests was refused.
    assert_eq!(response["request_id"], "r1");

    server.stop().await;
}

#[tokio::test]
async fn a_request_with_no_request_id_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&json!({"version": 1, "type": "clear_traffic"}))
        .await;

    assert_eq!(error_code(&response), "missing_request_id");
    assert_eq!(response["request_id"], serde_json::Value::Null);

    server.stop().await;
}

#[tokio::test]
async fn an_operation_this_server_does_not_have_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "drop_everything")).await;

    assert_eq!(error_code(&response), "unknown_operation");
    assert!(
        !response["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("drop_everything"),
        "the operation was repeated back: {response}"
    );

    server.stop().await;
}

#[tokio::test]
async fn an_exchange_id_that_is_not_an_id_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for operation in ["get_traffic", "delete_traffic"] {
        let response = client
            .request(&request_with(
                "r1",
                operation,
                json!({"exchange_id": "../../etc/passwd"}),
            ))
            .await;

        assert_eq!(error_code(&response), "invalid_exchange_id", "{operation}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_request_longer_than_the_server_will_read_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // A well-formed request that is simply enormous.
    let huge = "x".repeat(proxy_ipc::MAX_REQUEST_BYTES + 1);
    let response = client
        .request(&request_with("r1", "list_traffic", json!({"host": huge})))
        .await;

    assert_eq!(error_code(&response), "message_too_large");

    // The connection survives it, and the oversized line was skipped rather
    // than half-read: the next request is answered normally.
    let after = client.request(&request("r2", "clear_traffic")).await;
    assert_eq!(after["request_id"], "r2");
    assert_eq!(after["ok"], true);

    server.stop().await;
}

#[tokio::test]
async fn a_request_at_the_size_limit_is_still_answered() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Padded to just under the limit with a filter value that is allowed to
    // be long-ish, so the line is large but legitimate in shape.
    let request = request_with("r1", "list_traffic", json!({"host": "x".repeat(255)}));
    let line = serde_json::to_string(&request).expect("serializes");
    assert!(line.len() < proxy_ipc::MAX_REQUEST_BYTES);

    let response = client.request_raw(&line).await;

    assert_eq!(response["ok"], true, "{response}");

    server.stop().await;
}

#[tokio::test]
async fn an_invalid_page_size_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for limit in [json!(0), json!(1001), json!(-1), json!("all")] {
        let response = client
            .request(&request_with("r1", "list_traffic", json!({"limit": limit})))
            .await;

        assert_eq!(error_code(&response), "invalid_parameters", "{limit}");
    }

    server.stop().await;
}

#[tokio::test]
async fn an_invalid_filter_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let invalid = [
        json!({"status": {"class": 9}}),
        json!({"status": 404}),
        json!({"host": ""}),
        json!({"started_at_or_after": {"seconds": 2, "nanos": 0},
               "started_before": {"seconds": 1, "nanos": 0}}),
        json!({"unknown_filter": "value"}),
    ];

    for parameters in invalid {
        let response = client
            .request(&request_with("r1", "list_traffic", parameters.clone()))
            .await;

        assert_eq!(error_code(&response), "invalid_parameters", "{parameters}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_blank_line_is_not_a_request() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client.send_raw("").await;
    client.send_raw("   ").await;

    // Nothing was asked, so nothing is answered: the next message is the
    // response to the request that follows.
    let response = client.request(&request("r1", "clear_traffic")).await;
    assert_eq!(response["request_id"], "r1");

    server.stop().await;
}

#[tokio::test]
async fn several_bad_requests_in_a_row_do_not_wear_the_server_out() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for _ in 0..64 {
        client.send_raw("{").await;
    }
    for _ in 0..64 {
        assert_eq!(error_code(&client.next_message().await), "invalid_json");
    }

    let after = client.request(&request("r1", "clear_traffic")).await;
    assert_eq!(after["ok"], true);

    server.stop().await;
}

#[tokio::test]
async fn a_client_sending_nonsense_does_not_disturb_another_client() {
    let server = TestServer::start().await;
    let mut noisy = server.connect().await;
    let mut quiet = server.connect().await;

    for _ in 0..16 {
        noisy.send_raw("]]]not json[[[").await;
    }
    let response = quiet.request(&request("r1", "list_traffic")).await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["total"], 0);

    server.stop().await;
}

#[tokio::test]
async fn two_requests_on_one_line_boundary_are_both_answered() {
    let server = TestServer::start().await;
    let mut client = Client::connect(server.socket()).await;

    // Written as one chunk with the newline in the middle: framing is the
    // newline, not the write.
    let first = serde_json::to_string(&request("r1", "clear_traffic")).expect("serializes");
    let second = serde_json::to_string(&request("r2", "clear_traffic")).expect("serializes");
    client.send_raw(&format!("{first}\n{second}")).await;

    assert_eq!(client.next_message().await["request_id"], "r1");
    assert_eq!(client.next_message().await["request_id"], "r2");

    server.stop().await;
}
