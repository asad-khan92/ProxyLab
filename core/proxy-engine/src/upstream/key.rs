//! Where an upstream request is going, and what makes two connections to it
//! interchangeable.
//!
//! A pooled connection is only ever handed to a request that would have
//! opened exactly the same connection for itself. [`UpstreamKey`] is the
//! statement of "exactly the same", and it is deliberately narrow:
//!
//! ```text
//! http   ──▶ (Http,  host, port)
//! https  ──▶ (Https, host, port, the name whose certificate was verified)
//! ```
//!
//! * **Host and port** — the socket the connection is to. Compared
//!   case-insensitively, because host names are, and without the brackets an
//!   IPv6 literal wears in a URL, because a socket address does not have
//!   them.
//! * **Scheme** — plaintext and TLS are never the same connection, even to
//!   the same socket.
//! * **The verified name** — for TLS, the host that was sent in SNI and whose
//!   certificate was checked. Two requests that verified different names have
//!   different keys even when they resolve to the same address, so a session
//!   established for one host is never reused for another.
//!
//! # What is not in the key, and why
//!
//! The TLS *configuration* — the trust anchors, the ALPN offer, the protocol
//! versions — is not a field, because it cannot vary within a pool: an
//! [`UpstreamManager`](super::UpstreamManager) holds one
//! [`Interceptor`](proxy_tls::Interceptor) for its whole life and that is the
//! only thing it opens upstream sessions with. A second configuration means a
//! second interceptor, which means a second manager and a second pool. The
//! key would have to grow a field the day that stops being true.
//!
//! The protocol is not a field either. HTTP/1.1 and HTTP/2 connections are
//! not mixed by keying them apart but by being kept in different places — see
//! [`super::pool`] and [`super::manager`] — because what can be done with one
//! is not what can be done with the other.

use std::fmt;

use hyper::http::uri::{Authority, Scheme};
use proxy_tls::Host;

use crate::http1::upstream::UpstreamError;
use crate::origin;

/// Which transport reaches an origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// A plain socket.
    Plaintext,
    /// A TLS session ProxyLab opens and verifies.
    Tls,
}

/// What makes two upstream connections interchangeable.
///
/// See the module documentation for what each part is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UpstreamKey {
    transport: Transport,
    /// Lowercased, and without the brackets an IPv6 literal wears in a URL.
    host: String,
    port: u16,
    /// The name whose certificate was verified, for a TLS connection.
    verified: Option<Host>,
}

impl UpstreamKey {
    /// Which transport connections under this key use.
    pub fn transport(&self) -> Transport {
        self.transport
    }
}

/// The key, in a form a log line can carry.
///
/// Host and port only: a key names a server, and never a request, a path, or
/// anything a request carried.
impl fmt::Display for UpstreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = match self.transport {
            Transport::Plaintext => "http",
            Transport::Tls => "https",
        };

        if self.host.contains(':') {
            write!(f, "{scheme}://[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{scheme}://{}:{}", self.host, self.port)
        }
    }
}

/// Everything opening a connection to an origin needs.
///
/// Built once per request, from wherever the request is actually going: the
/// target a plaintext request named, the `CONNECT` an intercepted session was
/// opened to, or the destination a Map Remote rule decided on. It is the rule
/// that decides, not the URL the client asked for — see
/// [`crate::http1::ruled`] — so a mapped request keys on where it ends up.
#[derive(Clone, Debug)]
pub struct UpstreamDestination {
    key: UpstreamKey,
    /// What the origin is called, for the requests that do not say.
    authority: Authority,
    /// The URL scheme, for building the absolute target HTTP/2 wants.
    scheme: Scheme,
}

impl UpstreamDestination {
    /// A plaintext destination at `host` and `port`.
    ///
    /// `host` is a connect host: no brackets around an IPv6 literal.
    pub fn plaintext(host: &str, port: u16) -> Result<Self, UpstreamError> {
        Ok(Self {
            key: UpstreamKey {
                transport: Transport::Plaintext,
                host: host.to_ascii_lowercase(),
                port,
                verified: None,
            },
            authority: origin::authority(host, port).ok_or(UpstreamError::UnusableHost)?,
            scheme: Scheme::HTTP,
        })
    }

    /// A TLS destination at `host` and `port`.
    ///
    /// `host` is also the name sent in SNI and verified against the origin's
    /// certificate, which is why it is a [`Host`] rather than a string: a
    /// destination that cannot be verified cannot be connected to.
    pub fn tls(host: &Host, port: u16) -> Result<Self, UpstreamError> {
        Ok(Self {
            key: UpstreamKey {
                transport: Transport::Tls,
                host: host.as_str().to_ascii_lowercase(),
                port,
                verified: Some(host.clone()),
            },
            authority: origin::authority(host.as_str(), port).ok_or(UpstreamError::UnusableHost)?,
            scheme: Scheme::HTTPS,
        })
    }

    /// The pool key connections to this destination are kept under.
    pub fn key(&self) -> &UpstreamKey {
        &self.key
    }

    /// The host to open the socket to, without IPv6 brackets.
    pub fn host(&self) -> &str {
        &self.key.host
    }

    /// The port to open the socket to.
    pub fn port(&self) -> u16 {
        self.key.port
    }

    /// The name to verify, for a TLS destination.
    pub fn verified(&self) -> Option<&Host> {
        self.key.verified.as_ref()
    }

    /// What the origin is called, for the requests that do not say.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// The URL scheme this destination is reached on.
    pub fn scheme(&self) -> &Scheme {
        &self.scheme
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(text: &str) -> Host {
        Host::parse(text).expect("usable host")
    }

    fn plaintext(host: &str, port: u16) -> UpstreamDestination {
        UpstreamDestination::plaintext(host, port).expect("a usable destination")
    }

    fn tls(name: &str, port: u16) -> UpstreamDestination {
        UpstreamDestination::tls(&host(name), port).expect("a usable destination")
    }

    #[test]
    fn the_same_plaintext_origin_is_the_same_key() {
        assert_eq!(
            plaintext("example.com", 80).key(),
            plaintext("example.com", 80).key()
        );
    }

    #[test]
    fn a_host_name_is_matched_case_insensitively() {
        assert_eq!(
            plaintext("Example.COM", 80).key(),
            plaintext("example.com", 80).key()
        );
    }

    #[test]
    fn a_different_host_is_a_different_key() {
        assert_ne!(
            plaintext("example.com", 80).key(),
            plaintext("other.com", 80).key()
        );
    }

    #[test]
    fn a_different_port_is_a_different_key() {
        assert_ne!(
            plaintext("example.com", 80).key(),
            plaintext("example.com", 8080).key()
        );
    }

    #[test]
    fn plaintext_and_tls_never_share_a_connection() {
        assert_ne!(
            plaintext("example.com", 443).key(),
            tls("example.com", 443).key()
        );
    }

    #[test]
    fn a_tls_session_verified_for_one_name_is_not_reused_for_another() {
        assert_ne!(
            tls("example.com", 443).key(),
            tls("other.example.com", 443).key()
        );
    }

    #[test]
    fn a_key_names_a_server_and_nothing_a_request_carried() {
        assert_eq!(
            plaintext("example.com", 8080).key().to_string(),
            "http://example.com:8080"
        );
        assert_eq!(
            tls("example.com", 443).key().to_string(),
            "https://example.com:443"
        );
    }

    #[test]
    fn an_ipv6_literal_is_bracketed_only_where_a_url_needs_it() {
        let destination = plaintext("::1", 8080);

        assert_eq!(destination.host(), "::1");
        assert_eq!(destination.authority().to_string(), "[::1]:8080");
        assert_eq!(destination.key().to_string(), "http://[::1]:8080");
    }

    #[test]
    fn a_destination_knows_which_transport_reaches_it() {
        assert_eq!(
            plaintext("example.com", 80).key().transport(),
            Transport::Plaintext
        );
        assert_eq!(tls("example.com", 443).key().transport(), Transport::Tls);
    }

    #[test]
    fn a_host_that_cannot_be_named_in_a_url_is_not_a_destination() {
        assert!(UpstreamDestination::plaintext("not a host", 80).is_err());
    }
}
