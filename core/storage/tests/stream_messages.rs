//! Stream messages hang off an exchange and are deleted with it.

mod support;

use support::{http_exchange, TestStore};

use proxy_storage::{
    BodyCapture, StreamDirection, StreamKind, StreamMessage, StreamMessageId, Timestamp,
};

fn message(
    store: &TestStore,
    sequence: u64,
    direction: StreamDirection,
    kind: StreamKind,
    payload: BodyCapture,
) -> StreamMessage {
    let exchange = http_exchange("example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores the parent");

    StreamMessage {
        id: StreamMessageId::next(),
        exchange_id: exchange.id,
        sequence,
        captured_at: Timestamp::now(),
        direction,
        kind,
        event_name: None,
        event_id: None,
        retry_ms: None,
        payload,
    }
}

fn attached(
    exchange: &proxy_storage::TrafficExchange,
    sequence: u64,
    payload: &[u8],
) -> StreamMessage {
    StreamMessage {
        id: StreamMessageId::next(),
        exchange_id: exchange.id,
        sequence,
        captured_at: Timestamp::now(),
        direction: StreamDirection::ClientToServer,
        kind: StreamKind::WebsocketText,
        event_name: None,
        event_id: None,
        retry_ms: None,
        payload: BodyCapture::complete(payload.to_vec()),
    }
}

#[test]
fn a_text_message_comes_back_with_its_payload() {
    let store = TestStore::open();
    let message = message(
        &store,
        0,
        StreamDirection::ClientToServer,
        StreamKind::WebsocketText,
        BodyCapture::complete(br#"{"ok":true}"#.to_vec()),
    );

    store
        .storage
        .insert_stream_message(&message)
        .expect("stores");

    let stored = store
        .storage
        .get_stream_message(message.id)
        .expect("reads")
        .expect("the message is stored");

    assert_eq!(stored, message);
    assert_eq!(stored.payload.data(), br#"{"ok":true}"#);
    assert!(serde_json::from_slice::<serde_json::Value>(stored.payload.data()).is_ok());
}

#[test]
fn a_binary_payload_survives_unchanged() {
    let store = TestStore::open();
    let bytes = vec![0x00, 0x81, 0xff, 0x7f, b'x'];
    let message = message(
        &store,
        0,
        StreamDirection::ServerToClient,
        StreamKind::WebsocketBinary,
        BodyCapture::complete(bytes.clone()),
    );

    store
        .storage
        .insert_stream_message(&message)
        .expect("stores");

    let stored = store
        .storage
        .get_stream_message(message.id)
        .expect("reads")
        .expect("the message is stored");

    assert_eq!(stored.kind, StreamKind::WebsocketBinary);
    assert_eq!(stored.payload.data(), bytes);
}

#[test]
fn messages_keep_their_direction_and_order() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores the parent");

    let first = attached(&exchange, 0, b"up");
    let second = {
        let mut message = attached(&exchange, 1, b"down");
        message.direction = StreamDirection::ServerToClient;
        message
    };

    store.storage.insert_stream_message(&first).expect("stores");
    store
        .storage
        .insert_stream_message(&second)
        .expect("stores");

    let page = store
        .storage
        .list_stream_messages(exchange.id, None, None)
        .expect("lists");

    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.messages[0].sequence, 0);
    assert_eq!(page.messages[0].direction, StreamDirection::ClientToServer);
    assert_eq!(page.messages[1].sequence, 1);
    assert_eq!(page.messages[1].direction, StreamDirection::ServerToClient);
    assert!(page.next_after.is_none());
}

#[test]
fn an_sse_event_keeps_its_fields() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/events", 200);
    store.storage.insert(&exchange).expect("stores the parent");

    let event = StreamMessage {
        id: StreamMessageId::next(),
        exchange_id: exchange.id,
        sequence: 0,
        captured_at: Timestamp::now(),
        direction: StreamDirection::ServerToClient,
        kind: StreamKind::Sse,
        event_name: Some("update".to_owned()),
        event_id: Some("42".to_owned()),
        retry_ms: Some(3_000),
        payload: BodyCapture::complete(b"hello".to_vec()),
    };

    store.storage.insert_stream_message(&event).expect("stores");

    let stored = store
        .storage
        .get_stream_message(event.id)
        .expect("reads")
        .expect("the event is stored");

    assert_eq!(stored.event_name.as_deref(), Some("update"));
    assert_eq!(stored.event_id.as_deref(), Some("42"));
    assert_eq!(stored.retry_ms, Some(3_000));
    assert_eq!(stored.payload.data(), b"hello");
}

#[test]
fn a_list_does_not_carry_payloads() {
    let store = TestStore::open();
    let message = message(
        &store,
        0,
        StreamDirection::ClientToServer,
        StreamKind::WebsocketText,
        BodyCapture::complete(b"secret".to_vec()),
    );
    store
        .storage
        .insert_stream_message(&message)
        .expect("stores");

    let page = store
        .storage
        .list_stream_messages(message.exchange_id, None, None)
        .expect("lists");

    assert_eq!(page.messages[0].payload_size, 6);
    let json = serde_json::to_value(&page.messages[0]).expect("serializes");
    assert!(json.get("payload").is_none(), "{json}");
}

#[test]
fn deleting_the_parent_removes_its_messages() {
    let store = TestStore::with_inline_limit(8);
    let exchange = http_exchange("example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores the parent");

    let large = StreamMessage {
        id: StreamMessageId::next(),
        exchange_id: exchange.id,
        sequence: 0,
        captured_at: Timestamp::now(),
        direction: StreamDirection::ClientToServer,
        kind: StreamKind::WebsocketBinary,
        event_name: None,
        event_id: None,
        retry_ms: None,
        payload: BodyCapture::complete(vec![b'x'; 64]),
    };
    store.storage.insert_stream_message(&large).expect("stores");
    assert!(
        store
            .body_files()
            .iter()
            .any(|name| name.ends_with(".stream")),
        "{:?}",
        store.body_files()
    );

    store.storage.delete(exchange.id).expect("deletes");

    assert_eq!(
        store.storage.get_stream_message(large.id).expect("reads"),
        None
    );
    assert!(
        store
            .body_files()
            .iter()
            .all(|name| !name.ends_with(".stream")),
        "{:?}",
        store.body_files()
    );
}

#[test]
fn a_message_cannot_be_stored_without_its_parent() {
    let store = TestStore::open();
    let message = StreamMessage {
        id: StreamMessageId::next(),
        exchange_id: proxy_storage::ExchangeId::next(),
        sequence: 0,
        captured_at: Timestamp::now(),
        direction: StreamDirection::ClientToServer,
        kind: StreamKind::WebsocketText,
        event_name: None,
        event_id: None,
        retry_ms: None,
        payload: BodyCapture::complete(b"orphan".to_vec()),
    };

    assert!(store.storage.insert_stream_message(&message).is_err());
}

#[test]
fn upserting_the_parent_leaves_its_messages() {
    let store = TestStore::open();
    let mut exchange = http_exchange("example.com", "/events", 200);
    store.storage.insert(&exchange).expect("stores");

    let event = attached(&exchange, 0, b"ping");
    store.storage.insert_stream_message(&event).expect("stores");

    exchange.response.as_mut().expect("a response").status = 200;
    store.storage.upsert(&exchange).expect("updates");

    let page = store
        .storage
        .list_stream_messages(exchange.id, None, None)
        .expect("lists");
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].id, event.id);
}
