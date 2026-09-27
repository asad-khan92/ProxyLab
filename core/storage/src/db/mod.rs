//! Keeping captured traffic, so that it outlives the process that saw it.
//!
//! ```text
//! TrafficExchange
//!       |
//!       +--> EventPublisher   live subscribers, lossy by design
//!       |
//!       +--> ExchangeSink ──> StorageWorker ──> Storage ──> SQLite
//!                                                             |
//!                                                    the traffic a UI
//!                                                    asks for later
//! ```
//!
//! [`Storage`] is the whole of the persistence: a SQLite file for the
//! metadata, a directory for the captured bodies too large to keep in it, and
//! the operations a traffic list needs — store one, read one back, page
//! through them newest first, count them, delete one, delete them all.
//!
//! [`StorageWorker`] is how a proxy uses it. Storage is synchronous, so the
//! worker owns a blocking thread and a bounded queue, and the proxy only ever
//! hands an exchange to an [`ExchangeSink`] and carries on.
//!
//! # Storage never breaks proxying
//!
//! This is the rule the whole layer is built around. A captured exchange is a
//! debugging record; the request that produced it has already been carried.
//! So there is no path from a storage failure to a client:
//!
//! - handing an exchange over cannot block and cannot fail;
//! - a full queue drops the exchange and counts it;
//! - a database that refuses the work is counted and logged, once, safely;
//! - a database that cannot even be opened means the proxy runs without
//!   persistence rather than not running.
//!
//! Nothing here turns into an HTTP status, and nothing here panics on a
//! failure it can report.
//!
//! # Sensitive data
//!
//! The database holds what the traffic held: credentials, cookies, tokens,
//! bodies. That is the point of a debugging proxy. What must not happen is
//! that data reaching a log, so no diagnostic in this module renders a URL, a
//! header, or a body — a storage failure is reported by exchange id and
//! nothing else. See the crate documentation for the same rule applied to the
//! model.

mod bodies;
mod config;
mod encoding;
mod error;
mod query;
mod rules;
mod saved;
mod schema;
mod search;
mod store;
mod stream;
mod worker;

pub use config::{StorageConfig, BODIES_DIRECTORY, DATABASE_FILE, DEFAULT_INLINE_BODY_MAX_BYTES};
pub use error::StorageError;
pub use query::{
    Cursor, ExchangeSummary, StatusFilter, TrafficFilter, TrafficOrder, TrafficPage, TrafficQuery,
    DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};
pub use schema::SCHEMA_VERSION;
pub use search::{regex_is_valid, InvalidSearch, Needle};
pub use store::Storage;
pub use worker::{
    ExchangeSink, StorageHandle, StorageWorker, StreamMessageSink, DEFAULT_STORAGE_QUEUE_CAPACITY,
};
