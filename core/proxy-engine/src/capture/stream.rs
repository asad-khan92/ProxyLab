//! Capturing one stream message at a time, bounded per exchange.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use proxy_storage::{
    BodyCapture, ExchangeId, StreamDirection, StreamKind, StreamMessage, StreamMessageId,
    StreamMessageSink, StreamPublisher, Timestamp, DEFAULT_MAX_STREAM_MESSAGES,
};

/// Hands captured stream messages to storage and to whoever is watching.
///
/// Cheap to clone: every clone shares the sequence counter, so two directions
/// of one WebSocket number the same sequence.
#[derive(Clone, Debug)]
pub struct StreamTap {
    exchange_id: ExchangeId,
    sequence: Arc<AtomicU64>,
    max_messages: u64,
    max_payload: usize,
    publisher: StreamPublisher,
    sink: Option<Arc<dyn StreamMessageSink>>,
}

impl StreamTap {
    /// A tap on `exchange_id`, keeping at most `max_payload` bytes per message.
    pub fn new(
        exchange_id: ExchangeId,
        max_payload: usize,
        publisher: StreamPublisher,
        sink: Option<Arc<dyn StreamMessageSink>>,
    ) -> Self {
        Self {
            exchange_id,
            sequence: Arc::new(AtomicU64::new(0)),
            max_messages: DEFAULT_MAX_STREAM_MESSAGES,
            max_payload,
            publisher,
            sink,
        }
    }

    /// The exchange these messages belong to.
    pub fn exchange_id(&self) -> ExchangeId {
        self.exchange_id
    }

    /// How much of one payload is kept.
    pub fn max_payload(&self) -> usize {
        self.max_payload
    }

    /// Records one message, if this exchange has not already hit the cap.
    pub fn capture(
        &self,
        direction: StreamDirection,
        kind: StreamKind,
        event_name: Option<String>,
        event_id: Option<String>,
        retry_ms: Option<u64>,
        payload: BodyCapture,
    ) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        if sequence >= self.max_messages {
            return;
        }

        let message = StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: self.exchange_id,
            sequence,
            captured_at: Timestamp::now(),
            direction,
            kind,
            event_name,
            event_id,
            retry_ms,
            payload,
        };

        self.publisher.publish(message.summary());
        if let Some(sink) = &self.sink {
            sink.accept_stream(message);
        }
    }

    /// Records a WebSocket payload, truncated at the capture limit.
    pub fn capture_websocket(&self, direction: StreamDirection, kind: StreamKind, payload: &[u8]) {
        self.capture(
            direction,
            kind,
            None,
            None,
            None,
            BodyCapture::limited(payload, self.max_payload),
        );
    }
}
