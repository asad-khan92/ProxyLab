//! HAR export and import: captured traffic written as a file, and a file read
//! back as captured traffic.
//!
//! ```text
//!                     ┌──────────────┐
//!  TrafficExchange ──▶│  convert     │──▶ Entry ──▶ HarWriter ──▶ a .har file
//!                     │              │
//!  TrafficExchange ◀──│              │◀── Entry ◀── import::read ◀──┘
//!                     └──────────────┘
//! ```
//!
//! HAR 1.2 is the format every other HTTP debugging tool reads and writes, so
//! this is how a ProxyLab session leaves the machine it was captured on and
//! how somebody else's session arrives. It is the same shape as
//! `proxy-curl`: one crate, one job, and no model of its own — what is
//! exported is the [`TrafficExchange`](proxy_storage::TrafficExchange) the
//! proxy captured, and what is imported is the same structure again.
//!
//! # What this crate is not
//!
//! A store. Nothing here reads or writes the traffic database, decides which
//! exchanges to export, or keeps what it imported: it is handed exchanges and
//! a path, or a path and limits, and it answers. The IPC boundary is what
//! pages the database into an export and what puts an import into it.
//!
//! It is also not a client. Importing a HAR file produces records and opens
//! no connection — see [`import`] for why that is worth stating.
//!
//! # Sensitive data
//!
//! A HAR file contains what the traffic contained: credentials, cookies,
//! tokens, bodies. That is what makes it useful and it is why exporting one
//! is something a person asks for, by name, with a save panel. What this
//! crate does not do is write any of it anywhere else — no logging, and no
//! error message that quotes the document. See [`error`].
//!
//! # Limits
//!
//! An export writes what was captured, which the proxy's own capture limit
//! has already bounded; a body that was cut short is written with its real
//! size and a note saying so. An import is bounded here instead, because the
//! file was written by something else: see [`ImportLimits`].

pub mod convert;
pub mod error;
pub mod export;
pub mod import;
pub mod model;
mod time;

pub use error::{HarError, PathProblem};
pub use export::{write_all, ExportSummary, HarWriter, CREATOR_NAME};
pub use import::{read, Import, ImportLimits, MAX_IMPORT_BYTES, MAX_IMPORT_ENTRIES};
pub use model::{Entry, Har, HAR_VERSION};
