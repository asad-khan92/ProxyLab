//! The connections ProxyLab holds open to origin servers.
//!
//! Everything above this module produces one shape of request and knows
//! nothing about how it travels. This module owns the other half: which
//! connection carries it, whether that connection already existed, and
//! whether it is worth keeping afterwards.
//!
//! ```text
//!   canonical request ──▶ UpstreamManager ──▶ origin
//!                               │
//!                     ┌─────────┴─────────┐
//!                     ▼                   ▼
//!              HTTP/1.1 pool       HTTP/2 connections
//! ```
//!
//! Four modules, one job each:
//!
//! | Module | Job |
//! | --- | --- |
//! | [`key`] | What makes two connections interchangeable |
//! | [`pool`] | The idle HTTP/1.1 connections, and what may be kept |
//! | [`body`] | The bodies that cross the pool, and when a connection is free |
//! | [`manager`] | Choosing a connection, and the one safe retry |
//!
//! # Why an HTTP/1.1 connection is not shared
//!
//! HTTP/1.1 carries one exchange at a time. Before this module existed, an
//! intercepted connection opened one upstream connection and every request
//! inside it took a turn on that one — which is correct, and which meant four
//! concurrent HTTP/2 streams from a client reached an HTTP/1.1 origin one
//! after another. Now each of them takes a connection of its own out of the
//! pool, and gives it back when its response body has been read to the end.
//!
//! # Why an HTTP/2 connection is shared
//!
//! Because sharing it is what it is for. An HTTP/2 connection carries as many
//! concurrent streams as the origin allows, so a second connection to the
//! same origin would buy nothing and cost a handshake. One is registered per
//! origin and every request takes a clone of its sender.
//!
//! The two are never mixed: a pool of one-at-a-time connections and a
//! registry of multiplexed ones are different things kept in different
//! places, and which a connection is was settled by ALPN when it was opened.

pub mod body;
pub mod key;
pub mod manager;
pub mod pool;

pub use body::{boxed, PooledBody, UpstreamBody};
pub use key::{Transport, UpstreamDestination, UpstreamKey};
pub use manager::UpstreamManager;
pub use pool::{Http1Pool, PoolLimits};
