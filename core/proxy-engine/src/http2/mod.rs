//! HTTP/2 framing, on either side of the proxy.
//!
//! This module is to HTTP/2 what [`crate::http1`] is to HTTP/1.1, and
//! deliberately no more than that: it frames messages and it does not decide
//! anything about them. A request read off an HTTP/2 stream becomes the same
//! `Request` an HTTP/1.1 connection produces, is evaluated by the same rules,
//! is captured by the same recorder, and is sent upstream over whichever
//! protocol that hop settled on.
//!
//! ```text
//!   client                     ProxyLab                      origin
//!  ────────                   ──────────                    ────────
//!  HTTP/2  ──stream 1──▶ ┐                   ┌──▶ HTTP/2 or HTTP/1.1
//!  HTTP/2  ──stream 3──▶ ├─▶ one exchange ───┤
//!  HTTP/2  ──stream 5──▶ ┘   per stream      └──▶ …
//! ```
//!
//! [`serve`] is the client side: one HTTP/2 connection, any number of
//! concurrent streams on it, and one exchange per stream. [`upstream`] is the
//! other side: the HTTP/2 conversation with an origin that agreed to one.
//!
//! # One connection is not one request
//!
//! This is the difference that matters. An HTTP/1.1 connection through this
//! proxy carries one exchange; an HTTP/2 connection carries as many as the
//! client opens streams, at the same time, each with its own identity, its
//! own timing, its own capture, and its own answer. Nothing about a stream is
//! shared with its neighbours except the socket underneath them, and a stream
//! that fails is a stream that fails — not a connection that closes.
//!
//! # Nothing here implements HTTP/2
//!
//! No framing, no HPACK, no flow control, no window accounting. All of that
//! is hyper's, through the `h2` crate it is built on, which is why enabling
//! HTTP/2 was a feature flag rather than a protocol implementation. Flow
//! control is left switched on and at its defaults: it is what stops a fast
//! origin from filling this process's memory on behalf of a slow client.

pub mod upstream;

mod connection;

pub use connection::serve;
