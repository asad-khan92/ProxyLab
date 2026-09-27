//! Captured traffic: the model, body capture, timing, the event stream, and
//! persistence.
//!
//! This crate owns ProxyLab's canonical record of an HTTP exchange. A
//! [`TrafficExchange`] is what the proxy engine produces for every request it
//! carries, whatever route the request took: a plaintext request forwarded to
//! an origin and an HTTPS request read out of an intercepted TLS session
//! produce the same structure, so nothing downstream has to know which it
//! was.
//!
//! ```text
//! request + response
//!         ↓
//! TrafficExchange
//!         ├──────────→ EventPublisher ──→ live subscribers
//!         │
//!         └──────────→ ExchangeSink ────→ Storage ──→ SQLite
//! ```
//!
//! A finished exchange goes two ways, and the two are not alternatives. The
//! event stream is live and lossy: it carries an exchange to whoever is
//! watching right now, and drops it for a subscriber that has fallen behind.
//! [`Storage`] is the history: it keeps the exchange, so a UI can ask for
//! traffic that happened before it was started. See [`db`] for that half.
//!
//! The model is deliberately its own: no hyper, rustls, or tokio type appears
//! in it, so it can be serialized, sent across the IPC boundary, persisted, or
//! read by a UI without any of them coming along.
//!
//! # Sensitive data
//!
//! A captured exchange contains whatever the request and response contained —
//! credentials, cookies, tokens, bodies. That is the point: a debugging proxy
//! that hides them is not a debugging proxy. What is *not* acceptable is that
//! data reaching a log, so no type here renders a header value or a body
//! through [`Debug`]. Every `Debug` in this crate is metadata only: names,
//! sizes, counts, statuses. See [`TrafficExchange::summary`] for the one-line
//! form that is safe to write down.
//!
//! Storage holds those values too, because a debugging proxy that cannot show
//! you the `Authorization` header you are debugging is useless. What it does
//! not do is write them anywhere else: a storage failure is reported by
//! exchange id, and by nothing the exchange was carrying.
//!
//! # What this crate is not
//!
//! An IPC boundary. Nothing here talks to a UI; a future UI reads stored
//! traffic through the IPC boundary in `core/ipc/`, which is a later phase —
//! see `ROADMAP.md`.

pub mod base64_bytes;
pub mod body;
pub mod db;
pub mod error;
pub mod events;
pub mod headers;
pub mod saved;
pub mod stream;
pub mod timing;
pub mod traffic;

pub use body::{BodyCapture, BodyCollector, CaptureConfig, DEFAULT_MAX_CAPTURE_BODY_SIZE};
pub use db::{
    regex_is_valid, Cursor, ExchangeSink, ExchangeSummary, InvalidSearch, Needle, StatusFilter,
    Storage, StorageConfig, StorageError, StorageHandle, StorageWorker, StreamMessageSink,
    TrafficFilter, TrafficOrder, TrafficPage, TrafficQuery, DEFAULT_INLINE_BODY_MAX_BYTES,
    DEFAULT_PAGE_LIMIT, DEFAULT_STORAGE_QUEUE_CAPACITY, MAX_PAGE_LIMIT, SCHEMA_VERSION,
};
pub use error::{TrafficError, TrafficErrorKind};
pub use events::{
    EventPublisher, EventSubscriber, StreamError, TrafficEvent, DEFAULT_EVENT_CAPACITY,
};
pub use headers::{Header, HeaderValue, Headers};
pub use saved::{
    NewSavedRequest, SavedRequest, SavedRequestError, SavedRequestId, SavedRequestUpdate,
    MAX_SAVED_REQUEST_NAME_LENGTH,
};
pub use stream::{
    StreamDirection, StreamEvent, StreamKind, StreamMessage, StreamMessageId, StreamMessagePage,
    StreamMessageSummary, StreamPublisher, StreamSubscriber, DEFAULT_MAX_STREAM_MESSAGES,
};
pub use timing::{Timestamp, Timing, TimingRecorder};
pub use traffic::{
    CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId, HttpVersion,
    Scheme, TrafficExchange,
};
