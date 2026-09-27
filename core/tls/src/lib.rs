//! Root CA, dynamic leaf certificate generation, and TLS interception.
//!
//! Everything TLS in ProxyLab lives here. The proxy engine holds one
//! [`Interceptor`] and asks it for two things: the identity to answer a
//! client's handshake with, and a verified session to the real origin server.
//! It never handles a certificate parameter, a trust store, or a key.
//!
//! ```text
//! proxy-engine
//!       |
//!       v
//! Interceptor
//!       |
//!       +-- CertificateAuthority   the development CA, generated once and reused
//!       +-- Identity               a host's leaf certificate, cached per host
//!       +-- accept_client          the TLS session with the client
//!       +-- connect_upstream       the TLS session with the origin server
//! ```
//!
//! ## What is secret and what is not
//!
//! The CA certificate is public and there are accessors for it, because
//! handing it out is how anything comes to trust ProxyLab. Private keys are
//! not: no type here exposes one, the CA's key is written to a single
//! owner-readable file, leaf keys are never written down at all, and every
//! [`std::fmt::Debug`] implementation in the crate elides key material so a
//! stray log line cannot print one.
//!
//! ## Which HTTP is inside
//!
//! A TLS session says nothing about the HTTP inside it; ALPN does. Both
//! configurations here offer `h2` and `http/1.1`, so a client that speaks
//! HTTP/2 is answered in HTTP/2 and one that does not is answered in
//! HTTP/1.1 — and the session ProxyLab opens to the origin settles the same
//! question separately, because the two ends need not agree. [`Protocol`] is
//! what was settled on; [`client_protocol`] and [`upstream_protocol`] read it
//! back off a finished session. Nothing here forces HTTP/2 on a peer that did
//! not offer it, and a session that negotiated nothing is HTTP/1.1.
//!
//! ## Interception is not a downgrade
//!
//! The session with the client uses a certificate this CA signed, so a client
//! that has not been told to trust the CA refuses it — interception is
//! visible, never silent. The session with the origin verifies the origin's
//! real certificate against a trust store, on the same terms a browser would.
//! [`UpstreamTrust`] chooses which anchors those are and has no variant
//! meaning "trust anything"; there is no custom verifier in this crate, so
//! there is no way to reach an unverified origin session through this API.
//!
//! TLS 1.2 and TLS 1.3 are the protocol versions. SSLv3, TLS 1.0 and TLS 1.1
//! are not implemented by the TLS library this crate is built on, so they are
//! not something that can be switched on.

mod alpn;
mod authority;
mod cache;
mod config;
mod error;
mod handshake;
mod host;
mod identity;
mod interceptor;
mod leaf;
mod store;
mod upstream;

#[cfg(test)]
mod test_support;

pub use alpn::{client_protocol, upstream_protocol, Protocol, DEFAULT_PROTOCOLS};
pub use authority::CertificateAuthority;
pub use error::TlsError;
pub use handshake::{ClientSession, UpstreamSession};
pub use host::{Host, HostError};
pub use identity::Identity;
pub use interceptor::Interceptor;
pub use upstream::UpstreamTrust;
