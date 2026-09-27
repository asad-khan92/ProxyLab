//! Saved requests over the socket, as a client speaks them.
//!
//! The protocol by hand, over a real Unix socket, against a real server with
//! a real database behind it. What is under test is the wire: the six
//! operations, the shapes they come back in, and the refusals — a name that
//! is not one, a URL this proxy could not send to, an id that is not an id.
//!
//! Sending a saved request is not among them, and that is the point: replay
//! is `replay_request`, tested in `developer.rs` and in the engine. A client
//! reads a saved request and asks for it to be sent, and there is no second
//! path.

mod support;

use serde_json::{json, Value};
use support::{request, request_with, secretive_exchange, Client, TestServer};

/// A `create_saved_request` for a request with headers and a JSON body.
fn create(id: &str, name: &str) -> Value {
    request_with(
        id,
        "create_saved_request",
        json!({
            "name": name,
            "method": "post",
            "url": "https://example.com/users",
            "headers": [
                {"name": "Content-Type", "value": {"text": "application/json"}},
                {"name": "Authorization", "value": {"text": "Bearer sk-secret"}},
            ],
            // {"name":"ada"}
            "body": "eyJuYW1lIjoiYWRhIn0=",
        }),
    )
}

/// Saves one request and gives back its id.
async fn saved(client: &mut Client, name: &str) -> String {
    let response = client.request(&create("save", name)).await;

    assert_eq!(response["ok"], true, "{response}");

    response["result"]["saved_request"]["id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

#[tokio::test]
async fn a_saved_request_comes_back_with_an_id_and_timestamps() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&create("1", "Create a user")).await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["type"], "create_saved_request");
    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["name"], "Create a user");
    assert_eq!(saved["url"], "https://example.com/users");
    assert!(saved["id"].as_str().expect("an id").contains('-'));
    assert!(saved["created_at_ms"].as_u64().expect("a moment") > 0);
    assert_eq!(saved["created_at_ms"], saved["updated_at_ms"]);

    server.stop().await;
}

#[tokio::test]
async fn the_url_is_normalized_the_way_a_replay_would_normalize_it() {
    // Saved through the same check a replay goes through, so what is stored
    // is what will be sent: the scheme and host lowercased and the path
    // filled in. The method is *not* touched — an HTTP method is
    // case-sensitive, and a proxy that quietly uppercased one would be
    // sending a different request from the one that was written down.
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({"name": "Root", "method": "get", "url": "HTTPS://Example.COM"}),
        ))
        .await;

    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["method"], "get");
    assert_eq!(saved["url"], "https://example.com/");

    server.stop().await;
}

#[tokio::test]
async fn an_edited_url_is_normalized_the_same_way_a_new_one_is() {
    // What is stored is what will be sent, whichever way the request got
    // there: an edit goes through the same `RequestSpec` check a save does.
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({"saved_request_id": id, "url": "HTTPS://Example.COM"}),
        ))
        .await;

    assert_eq!(
        response["result"]["saved_request"]["url"],
        "https://example.com/"
    );

    let read = client
        .request(&request_with(
            "2",
            "get_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;
    assert_eq!(
        read["result"]["saved_request"]["url"],
        "https://example.com/"
    );

    server.stop().await;
}

#[tokio::test]
async fn an_edit_that_names_no_url_leaves_the_stored_one_alone() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({"saved_request_id": id, "name": "Renamed"}),
        ))
        .await;

    assert_eq!(response["result"]["saved_request"]["method"], "post");
    assert_eq!(
        response["result"]["saved_request"]["url"],
        "https://example.com/users"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_saved_request_is_read_back_whole() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "get_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;

    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["headers"][1]["name"], "Authorization");
    assert_eq!(saved["headers"][1]["value"]["text"], "Bearer sk-secret");
    assert_eq!(saved["body"]["data"], "eyJuYW1lIjoiYWRhIn0=");
    assert_eq!(saved["body"]["size"], 14);
    assert_eq!(saved["body"]["truncated"], false);

    server.stop().await;
}

#[tokio::test]
async fn the_list_is_in_the_order_they_were_saved() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    saved(&mut client, "First").await;
    saved(&mut client, "Second").await;
    saved(&mut client, "Third").await;

    let response = client.request(&request("1", "list_saved_requests")).await;

    let names: Vec<&str> = response["result"]["saved_requests"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|saved| saved["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["First", "Second", "Third"]);

    server.stop().await;
}

#[tokio::test]
async fn a_list_with_nothing_in_it_is_a_list() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("1", "list_saved_requests")).await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["saved_requests"], json!([]));

    server.stop().await;
}

#[tokio::test]
async fn renaming_changes_the_name_and_leaves_the_request_alone() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({"saved_request_id": id, "name": "Create Ada"}),
        ))
        .await;

    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["name"], "Create Ada");
    assert_eq!(saved["url"], "https://example.com/users");
    assert_eq!(saved["body"]["data"], "eyJuYW1lIjoiYWRhIn0=");
    assert_ne!(saved["created_at_ms"], Value::Null);

    server.stop().await;
}

#[tokio::test]
async fn editing_replaces_the_fields_an_update_names() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({
                "saved_request_id": id,
                "method": "PUT",
                "url": "https://example.com/users/1",
                "headers": [],
                "body": "e30=",
            }),
        ))
        .await;

    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["method"], "PUT");
    assert_eq!(saved["url"], "https://example.com/users/1");
    assert_eq!(saved["headers"], json!([]));
    assert_eq!(saved["body"]["data"], "e30=");
    assert_eq!(saved["body"]["size"], 2);

    server.stop().await;
}

#[tokio::test]
async fn deleting_says_whether_there_was_one_to_delete() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let deleted = client
        .request(&request_with(
            "1",
            "delete_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;
    assert_eq!(deleted["result"]["deleted"], true);
    assert_eq!(deleted["result"]["saved_request_id"], id);

    let again = client
        .request(&request_with(
            "2",
            "delete_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["result"]["deleted"], false);

    server.stop().await;
}

// MARK: - Saving one out of captured traffic

#[tokio::test]
async fn a_captured_request_is_saved_as_the_core_stored_it() {
    // The credential included: the point of saving a request is sending it
    // again, and one that dropped the `Authorization` would reproduce
    // nothing.
    let server = TestServer::start().await;
    let id = server.store(&secretive_exchange("api.example.com"));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "save_request_from_traffic",
            json!({"exchange_id": id.to_string(), "name": "The order call"}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["type"], "save_request_from_traffic");
    let saved = &response["result"]["saved_request"];
    assert_eq!(saved["name"], "The order call");
    assert_eq!(saved["method"], "GET");
    assert_eq!(saved["url"], "http://api.example.com/orders");
    assert_eq!(saved["headers"][0]["name"], "Authorization");
    assert_eq!(saved["headers"][0]["value"]["text"], "Bearer sk-secret");
    assert_eq!(saved["body"]["data"], "cGFzc3dvcmQ9aHVudGVyMg==");

    server.stop().await;
}

#[tokio::test]
async fn a_request_saved_from_a_truncated_capture_says_it_sends_less() {
    let server = TestServer::start().await;
    let mut exchange = support::exchange("example.com", "/upload", 201);
    exchange.request.method = "POST".to_owned();
    exchange.request.body =
        proxy_storage::BodyCapture::truncated(b"the first bit".to_vec(), 5_000_000);
    let id = server.store(&exchange);
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "save_request_from_traffic",
            json!({"exchange_id": id.to_string(), "name": "A big upload"}),
        ))
        .await;

    let body = &response["result"]["saved_request"]["body"];
    assert_eq!(body["truncated"], true);
    assert_eq!(body["size"], 5_000_000);

    server.stop().await;
}

#[tokio::test]
async fn saving_from_an_exchange_that_is_not_there_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "save_request_from_traffic",
            json!({
                "exchange_id": proxy_storage::ExchangeId::next().to_string(),
                "name": "Nothing",
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "not_found");

    server.stop().await;
}

// MARK: - Refusals

#[tokio::test]
async fn a_request_with_no_name_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({"name": "  ", "method": "GET", "url": "https://example.com/"}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_saved_request");

    server.stop().await;
}

#[tokio::test]
async fn a_name_longer_than_the_limit_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({"name": "n".repeat(201), "method": "GET", "url": "https://example.com/"}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_saved_request");

    server.stop().await;
}

#[tokio::test]
async fn a_url_this_proxy_could_not_send_to_is_refused_when_it_is_saved() {
    // Refused here rather than at the moment it would have mattered: the
    // same `RequestSpec` check a replay makes, made when the request is
    // written down.
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for url in [
        "ftp://example.com/x",
        "/relative",
        "https://ada:secret@example.com/",
    ] {
        let response = client
            .request(&request_with(
                "1",
                "create_saved_request",
                json!({"name": "No", "method": "GET", "url": url}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{url} was accepted: {response}");
        assert_eq!(response["error"]["code"], "invalid_saved_request", "{url}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_header_that_could_split_a_request_in_two_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({
                "name": "Smuggled",
                "method": "GET",
                "url": "https://example.com/",
                "headers": [{"name": "X-Thing", "value": {"text": "a\r\nGET /admin"}}],
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_saved_request");

    server.stop().await;
}

#[tokio::test]
async fn an_edit_that_would_make_it_unsendable_is_refused_and_stores_nothing() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let refused = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({"saved_request_id": id, "url": "ftp://example.com/x"}),
        ))
        .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_saved_request");

    let read = client
        .request(&request_with(
            "2",
            "get_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;
    assert_eq!(
        read["result"]["saved_request"]["url"],
        "https://example.com/users"
    );

    server.stop().await;
}

#[tokio::test]
async fn an_id_that_is_not_an_id_is_refused_by_name() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for operation in ["get_saved_request", "delete_saved_request"] {
        let response = client
            .request(&request_with(
                "1",
                operation,
                json!({"saved_request_id": "not-an-id"}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(
            response["error"]["code"], "invalid_saved_request_id",
            "{operation}"
        );
    }

    server.stop().await;
}

#[tokio::test]
async fn a_request_that_is_not_there_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let absent = proxy_storage::SavedRequestId::next().to_string();

    let read = client
        .request(&request_with(
            "1",
            "get_saved_request",
            json!({"saved_request_id": absent}),
        ))
        .await;
    assert_eq!(read["error"]["code"], "not_found", "{read}");

    let updated = client
        .request(&request_with(
            "2",
            "update_saved_request",
            json!({"saved_request_id": absent, "name": "x"}),
        ))
        .await;
    assert_eq!(updated["error"]["code"], "not_found", "{updated}");

    server.stop().await;
}

#[tokio::test]
async fn an_update_that_changes_nothing_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let id = saved(&mut client, "Create a user").await;

    let response = client
        .request(&request_with(
            "1",
            "update_saved_request",
            json!({"saved_request_id": id}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn a_parameter_the_server_does_not_know_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({
                "name": "x",
                "method": "GET",
                "url": "https://example.com/",
                "folder": "misspelled",
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn nothing_a_client_sent_comes_back_in_a_refusal() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "create_saved_request",
            json!({
                "name": "MY-SECRET-LABEL",
                "method": "GET",
                "url": "ftp://sk-secret.example.com/",
            }),
        ))
        .await;

    let message = serde_json::to_string(&response).expect("serializes");
    assert!(!message.contains("MY-SECRET-LABEL"), "{message}");
    assert!(!message.contains("sk-secret"), "{message}");

    server.stop().await;
}

// MARK: - Saved requests are not traffic

#[tokio::test]
async fn clearing_the_traffic_leaves_the_saved_requests_where_they_are() {
    let server = TestServer::start().await;
    server.store(&support::exchange("example.com", "/x", 200));
    let mut client = server.connect().await;
    saved(&mut client, "Create a user").await;

    let cleared = client.request(&request("1", "clear_traffic")).await;
    assert_eq!(cleared["result"]["removed"], 1);

    let list = client.request(&request("2", "list_saved_requests")).await;
    assert_eq!(
        list["result"]["saved_requests"]
            .as_array()
            .expect("a list")
            .len(),
        1
    );

    server.stop().await;
}

#[tokio::test]
async fn saving_a_request_publishes_no_traffic_event() {
    // A saved request is not an exchange the proxy carried, and a subscriber
    // that was told it was would show a row for a request nobody sent.
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let subscribed = client.request(&request("1", "subscribe")).await;
    assert_eq!(subscribed["result"]["subscribed"], true);

    saved(&mut client, "Create a user").await;

    // The next message is the answer to this list, not an event: an event
    // would arrive first and this assertion would read it instead.
    let list = client.request(&request("2", "list_saved_requests")).await;
    assert_eq!(list["type"], "list_saved_requests", "{list}");

    server.stop().await;
}
