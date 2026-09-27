//! ProxyLab's HTTP message models.
//!
//! HTTP/1.1 framing on the wire belongs to the proxy engine, which speaks it
//! through hyper on both sides of the proxy. What lives here is the model of
//! a request that has not been sent yet: the method, the URL, the headers,
//! and the body, checked once and then carried around as something that is
//! known to be sendable.
//!
//! ```text
//! cURL command ──▶ proxy-curl ──▶ RequestSpec ──▶ proxy-engine ──▶ origin
//!                                      ▲
//!                                      │
//!                                  IPC client
//! ```
//!
//! [`RequestSpec`] is the one representation of "a request somebody wants
//! sent" in the core. The cURL parser produces one, the IPC boundary carries
//! one, and the replay entry point sends one, so there is one set of rules
//! about what a URL, a method, and a header may be rather than three.
//!
//! # It is a validation boundary
//!
//! A `RequestSpec` cannot be built out of parts that have not been checked:
//! its fields are private and [`RequestSpec::new`] is the only way in.
//! Everything downstream is therefore free of the questions a request
//! arriving from outside raises — whether the URL names a host, whether the
//! scheme is one this proxy speaks, whether a header value carries a newline
//! that would let a second request be smuggled inside the first.
//!
//! # Sensitive data
//!
//! A request to replay carries whatever the original carried: `Authorization`,
//! `Cookie`, an API key in a header of a service's own naming. It is kept
//! exactly, because a replay that quietly dropped the credential would be a
//! replay of a different request. What is *not* acceptable is any of it
//! reaching a log, so [`Debug`] here renders names, counts, and sizes and
//! never a header value or a body — the same rule `proxy-storage` follows for
//! a captured exchange.
//!
//! Credentials in a URL authority — `https://user:secret@example.com/` — are
//! refused rather than carried, because a URL is the one part of a request
//! that *is* written to logs and shown in a traffic list. Whoever is parsing
//! such a URL is expected to turn it into an `Authorization` header first,
//! which is what the cURL parser does.
//!
//! # What this crate is not
//!
//! An HTTP implementation. There is no parser, no framing, no connection
//! handling, and no HTTP/2 here; the engine has all of that through hyper.

pub mod request;

pub use request::{
    RequestParts, RequestSpec, RequestSpecError, MAX_HEADERS, MAX_HEADER_NAME_LENGTH,
    MAX_METHOD_LENGTH, MAX_URL_LENGTH,
};
