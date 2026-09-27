//! Stream messages through the IPC boundary: list, get, events, cleanup.

mod support;

use support::{exchange, request, request_with, TestServer};

use proxy_storage::{
    BodyCapture, StreamDirection, StreamKind, StreamMessage, StreamMessageId, Timestamp,
};
use serde_json::{json, Value};

fn result(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "expected success: {response}");
    &response["result"]
}

fn stream_message(
    exchange_id: proxy_storage::ExchangeId,
    sequence: u64,
    direction: StreamDirection,
    kind: StreamKind,
    payload: &[u8],
) -> StreamMessage {
    StreamMessage {
        id: StreamMessageId::next(),
        exchange_id,
        sequence,
        captured_at: Timestamp::now(),
        direction,
        kind,
        event_name: None,
        event_id: None,
        retry_ms: None,
        payload: BodyCapture::complete(payload.to_vec()),
    }
}

#[tokio::test]
async fn listing_stream_messages_is_oldest_first_without_payloads() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let parent = server.store(&exchange("example.com", "/socket", 101));

    let first = stream_message(
        parent,
        0,
        StreamDirection::ClientToServer,
        StreamKind::WebsocketText,
        br#"{"ok":true}"#,
    );
    let second = stream_message(
        parent,
        1,
        StreamDirection::ServerToClient,
        StreamKind::WebsocketBinary,
        &[0x00, 0xff],
    );
    server.store_stream(&first);
    server.store_stream(&second);

    let listed = client
        .request(&request_with(
            "r1",
            "list_stream_messages",
            json!({"exchange_id": parent.to_string()}),
        ))
        .await;

    assert_eq!(listed["type"], "list_stream_messages", "{listed}");
    let messages = result(&listed)["messages"].as_array().expect("a page");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], first.id.to_string());
    assert_eq!(messages[0]["direction"], "client_to_server");
    assert_eq!(messages[0]["kind"], "websocket_text");
    assert_eq!(messages[0]["sequence"], 0);
    assert_eq!(messages[1]["id"], second.id.to_string());
    assert_eq!(messages[1]["direction"], "server_to_client");
    assert_eq!(messages[1]["kind"], "websocket_binary");
    assert!(messages[0].get("payload").is_none(), "{listed}");
    assert_eq!(result(&listed)["next_after"], Value::Null);

    server.stop().await;
}

#[tokio::test]
async fn getting_a_stream_message_returns_its_payload() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let parent = server.store(&exchange("example.com", "/socket", 101));
    let message = stream_message(
        parent,
        0,
        StreamDirection::ClientToServer,
        StreamKind::WebsocketText,
        br#"{"ok":true}"#,
    );
    server.store_stream(&message);

    let got = client
        .request(&request_with(
            "r1",
            "get_stream_message",
            json!({"stream_message_id": message.id.to_string()}),
        ))
        .await;

    assert_eq!(got["type"], "get_stream_message", "{got}");
    let payload = &result(&got)["message"]["payload"];
    assert_eq!(payload["truncated"], false);
    assert_eq!(payload["size"], 11);
    assert_eq!(payload["data"], "eyJvayI6dHJ1ZX0=");

    server.stop().await;
}

#[tokio::test]
async fn a_missing_stream_message_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "get_stream_message",
            json!({"stream_message_id": StreamMessageId::next().to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "not_found");

    server.stop().await;
}

#[tokio::test]
async fn deleting_an_exchange_removes_its_stream_messages() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let parent = server.store(&exchange("example.com", "/socket", 101));
    let message = stream_message(
        parent,
        0,
        StreamDirection::ServerToClient,
        StreamKind::Sse,
        b"hello",
    );
    server.store_stream(&message);

    let deleted = client
        .request(&request_with(
            "r1",
            "delete_traffic",
            json!({"exchange_id": parent.to_string()}),
        ))
        .await;
    assert_eq!(result(&deleted)["deleted"], true);

    let listed = client
        .request(&request_with(
            "r2",
            "list_stream_messages",
            json!({"exchange_id": parent.to_string()}),
        ))
        .await;
    assert_eq!(result(&listed)["messages"], json!([]));

    let got = client
        .request(&request_with(
            "r3",
            "get_stream_message",
            json!({"stream_message_id": message.id.to_string()}),
        ))
        .await;
    assert_eq!(got["ok"], false, "{got}");

    server.stop().await;
}

#[tokio::test]
async fn a_subscriber_is_told_about_new_stream_messages() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let subscribed = client.request(&request("r1", "subscribe")).await;
    assert_eq!(result(&subscribed)["subscribed"], true);

    let parent = server.store(&exchange("example.com", "/socket", 101));
    let message = stream_message(
        parent,
        0,
        StreamDirection::ClientToServer,
        StreamKind::WebsocketText,
        b"secret",
    );
    assert_eq!(server.streams.publish(message.summary()), 1);

    let event = client.next_message().await;
    assert!(event.get("ok").is_none(), "{event}");
    assert_eq!(event["type"], "stream_message_added");
    assert_eq!(event["message"]["id"], message.id.to_string());
    assert_eq!(event["message"]["exchange_id"], parent.to_string());
    assert!(event["message"].get("payload").is_none(), "{event}");
    let line = event.to_string();
    assert!(!line.contains("secret"), "{line}");

    server.stop().await;
}

#[tokio::test]
async fn an_sse_event_keeps_its_fields_on_the_wire() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let parent = server.store(&exchange("example.com", "/events", 200));
    let mut event = stream_message(
        parent,
        0,
        StreamDirection::ServerToClient,
        StreamKind::Sse,
        b"hello",
    );
    event.event_name = Some("update".to_owned());
    event.event_id = Some("42".to_owned());
    event.retry_ms = Some(3_000);
    server.store_stream(&event);

    let listed = client
        .request(&request_with(
            "r1",
            "list_stream_messages",
            json!({"exchange_id": parent.to_string()}),
        ))
        .await;
    let row = &result(&listed)["messages"][0];
    assert_eq!(row["kind"], "sse");
    assert_eq!(row["event_name"], "update");
    assert_eq!(row["event_id"], "42");
    assert_eq!(row["retry_ms"], 3_000);

    server.stop().await;
}
