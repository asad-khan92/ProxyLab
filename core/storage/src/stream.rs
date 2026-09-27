//! Messages that live on a long-lived exchange: WebSocket frames and SSE events.
//!
//! ```text
//! TrafficExchange  (the handshake, or the SSE response head)
//!        │
//!        └── stream_messages   one row per captured message/event
//! ```
//!
//! A WebSocket and an event stream both break the "one request, one response"
//! shape of an exchange. The handshake — or the SSE response head — is still
//! one [`TrafficExchange`](crate::TrafficExchange). What follows is a sequence
//! of messages that belong to it, stored beside it and deleted with it.
//!
//! # Bounded, always
//!
//! A socket that stays open for hours can carry an unbounded number of
//! messages. Capture stops at [`DEFAULT_MAX_STREAM_MESSAGES`] messages per
//! exchange and at the ordinary body limit per payload; forwarding does not.
//! A capture that hit either limit says so, the same way a truncated body
//! does.
//!
//! # What a summary carries
//!
//! [`StreamMessageSummary`] is what the live event stream and a list page
//! carry: direction, kind, timestamps, sizes, and the SSE fields that are
//! already short. The payload stays in storage until a client asks for that
//! one message by id, which is the same split the traffic list already has.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::body::BodyCapture;
use crate::events::{StreamError, DEFAULT_EVENT_CAPACITY};
use crate::timing::Timestamp;
use crate::traffic::ExchangeId;

/// The most messages captured for one exchange.
///
/// Ten thousand is a long debugging session on one socket, and a bounded
/// number of rows rather than a history that grows with the socket. Past
/// this, messages are forwarded and not stored.
pub const DEFAULT_MAX_STREAM_MESSAGES: u64 = 10_000;

/// Identifies one captured stream message.
///
/// A UUIDv7, for the same reasons [`ExchangeId`] is one: unique across runs,
/// ordered by the moment it was minted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamMessageId(Uuid);

impl StreamMessageId {
    /// A new id, for a message being captured now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }

    /// The id in the form it is written down as.
    pub fn as_text(self) -> String {
        self.to_string()
    }
}

impl FromStr for StreamMessageId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for StreamMessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// Which way a message travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamDirection {
    /// Client → server.
    ClientToServer,
    /// Server → client.
    ServerToClient,
}

impl StreamDirection {
    /// The direction as it is named on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientToServer => "client_to_server",
            Self::ServerToClient => "server_to_client",
        }
    }
}

impl fmt::Display for StreamDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of stream message this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// A WebSocket text message (opcode 1), assembled from its frames.
    WebsocketText,
    /// A WebSocket binary message (opcode 2), assembled from its frames.
    WebsocketBinary,
    /// One Server-Sent Event.
    Sse,
}

impl StreamKind {
    /// The kind as it is named on the wire and in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebsocketText => "websocket_text",
            Self::WebsocketBinary => "websocket_binary",
            Self::Sse => "sse",
        }
    }
}

impl fmt::Display for StreamKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One captured WebSocket message or SSE event.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMessage {
    /// Which message this is.
    pub id: StreamMessageId,
    /// The exchange it belongs to.
    pub exchange_id: ExchangeId,
    /// Order within that exchange, starting at 0.
    pub sequence: u64,
    /// When it was captured.
    pub captured_at: Timestamp,
    /// Which way it travelled.
    pub direction: StreamDirection,
    /// What kind of message it is.
    pub kind: StreamKind,
    /// SSE `event:` field, when this is an SSE event that named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    /// SSE `id:` field, when this is an SSE event that named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// SSE `retry:` field, milliseconds, when present and parseable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
    /// As much of the payload as was captured, and how long it was.
    pub payload: BodyCapture,
}

impl StreamMessage {
    /// The row a list or an event carries, with no payload.
    pub fn summary(&self) -> StreamMessageSummary {
        StreamMessageSummary {
            id: self.id,
            exchange_id: self.exchange_id,
            sequence: self.sequence,
            captured_at: self.captured_at,
            direction: self.direction,
            kind: self.kind,
            event_name: self.event_name.clone(),
            event_id: self.event_id.clone(),
            retry_ms: self.retry_ms,
            payload_size: self.payload.size(),
            payload_captured_len: self.payload.captured_len() as u64,
            payload_truncated: self.payload.is_truncated(),
        }
    }
}

impl fmt::Debug for StreamMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamMessage")
            .field("id", &self.id)
            .field("exchange_id", &self.exchange_id)
            .field("sequence", &self.sequence)
            .field("direction", &self.direction)
            .field("kind", &self.kind)
            .field("payload_size", &self.payload.size())
            .field("payload_truncated", &self.payload.is_truncated())
            .finish()
    }
}

/// A stream message without its payload, for a list or a live event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMessageSummary {
    /// Which message this is.
    pub id: StreamMessageId,
    /// The exchange it belongs to.
    pub exchange_id: ExchangeId,
    /// Order within that exchange, starting at 0.
    pub sequence: u64,
    /// When it was captured.
    pub captured_at: Timestamp,
    /// Which way it travelled.
    pub direction: StreamDirection,
    /// What kind of message it is.
    pub kind: StreamKind,
    /// SSE `event:` field, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    /// SSE `id:` field, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// SSE `retry:` field, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_ms: Option<u64>,
    /// How long the payload was.
    pub payload_size: u64,
    /// How many bytes of it were captured.
    pub payload_captured_len: u64,
    /// Whether the capture was cut short.
    pub payload_truncated: bool,
}

/// A page of stream messages, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMessagePage {
    /// The messages on this page, in sequence order.
    pub messages: Vec<StreamMessageSummary>,
    /// The sequence to pass as `after` for the next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<u64>,
}

/// What a subscriber receives: one captured stream message's summary.
pub type StreamEvent = Arc<StreamMessageSummary>;

/// Publishes captured stream messages.
///
/// The same shape as the traffic [`EventPublisher`](crate::EventPublisher):
/// bounded, never blocking, lossy for a subscriber that falls behind.
#[derive(Clone, Debug)]
pub struct StreamPublisher {
    sender: broadcast::Sender<StreamEvent>,
    capacity: usize,
}

impl StreamPublisher {
    /// A publisher holding [`DEFAULT_EVENT_CAPACITY`] events per subscriber.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }

    /// A publisher holding `capacity` events per subscriber.
    ///
    /// # Panics
    ///
    /// If `capacity` is zero.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "a stream event channel must be able to hold an event"
        );

        let (sender, _) = broadcast::channel(capacity);

        Self { sender, capacity }
    }

    /// Starts receiving messages published from now on.
    pub fn subscribe(&self) -> StreamSubscriber {
        StreamSubscriber {
            receiver: self.sender.subscribe(),
        }
    }

    /// Publishes a summary, and says how many subscribers it went to.
    pub fn publish(&self, summary: StreamMessageSummary) -> usize {
        self.sender.send(Arc::new(summary)).unwrap_or(0)
    }

    /// How many subscribers are listening.
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// How many events are held for a subscriber that is not keeping up.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for StreamPublisher {
    fn default() -> Self {
        Self::new()
    }
}

/// Receives captured stream-message summaries.
#[derive(Debug)]
pub struct StreamSubscriber {
    receiver: broadcast::Receiver<StreamEvent>,
}

impl StreamSubscriber {
    /// Waits for the next summary.
    pub async fn recv(&mut self) -> Result<StreamEvent, StreamError> {
        self.receiver.recv().await.map_err(StreamError::from)
    }

    /// The next summary, if one is already waiting.
    pub fn try_recv(&mut self) -> Option<Result<StreamEvent, StreamError>> {
        match self.receiver.try_recv() {
            Ok(event) => Some(Ok(event)),
            Err(broadcast::error::TryRecvError::Empty) => None,
            Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                Some(Err(StreamError::Lagged(missed)))
            }
            Err(broadcast::error::TryRecvError::Closed) => Some(Err(StreamError::Closed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> StreamMessage {
        StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: ExchangeId::next(),
            sequence: 3,
            captured_at: Timestamp::now(),
            direction: StreamDirection::ClientToServer,
            kind: StreamKind::WebsocketText,
            event_name: None,
            event_id: None,
            retry_ms: None,
            payload: BodyCapture::complete(br#"{"ok":true}"#.to_vec()),
        }
    }

    #[test]
    fn a_summary_carries_sizes_and_not_the_payload() {
        let message = message();
        let summary = message.summary();

        assert_eq!(summary.id, message.id);
        assert_eq!(summary.exchange_id, message.exchange_id);
        assert_eq!(summary.sequence, 3);
        assert_eq!(summary.direction, StreamDirection::ClientToServer);
        assert_eq!(summary.kind, StreamKind::WebsocketText);
        assert_eq!(summary.payload_size, 11);
        assert!(!summary.payload_truncated);

        let json = serde_json::to_value(&summary).expect("serializes");
        assert!(json.get("payload").is_none(), "{json}");
    }

    #[test]
    fn a_message_round_trips_through_json_with_its_payload() {
        let original = message();
        let json = serde_json::to_string(&original).expect("serializes");
        let back: StreamMessage = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, original);
        assert_eq!(back.payload.data(), br#"{"ok":true}"#);
    }

    #[test]
    fn debug_does_not_render_the_payload() {
        let rendered = format!("{:?}", message());

        assert!(!rendered.contains("ok"), "{rendered}");
        assert!(rendered.contains("payload_size"), "{rendered}");
    }

    #[test]
    fn an_sse_event_keeps_its_fields() {
        let mut event = message();
        event.kind = StreamKind::Sse;
        event.direction = StreamDirection::ServerToClient;
        event.event_name = Some("update".to_owned());
        event.event_id = Some("42".to_owned());
        event.retry_ms = Some(3_000);

        let summary = event.summary();
        assert_eq!(summary.event_name.as_deref(), Some("update"));
        assert_eq!(summary.event_id.as_deref(), Some("42"));
        assert_eq!(summary.retry_ms, Some(3_000));
    }
}
