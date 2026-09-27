//! Which HTTP protocol a TLS session agreed to carry.
//!
//! HTTPS does not say which version of HTTP is inside it. ALPN does: the
//! client lists the protocols it speaks in its hello, the server picks one,
//! and both sides know what the first byte after the handshake will be.
//!
//! ```text
//! client ──ALPN: h2, http/1.1──▶ server
//! client ◀────ALPN: h2────────── server
//! ```
//!
//! ProxyLab is on both ends of that, once as the server the client is
//! talking to and once as the client talking to the origin, and the two
//! answers are independent — a client may speak HTTP/2 to ProxyLab while
//! ProxyLab speaks HTTP/1.1 to the origin, or the other way round.
//!
//! This module is the whole of the vocabulary: the protocols that can be
//! named, the bytes they are named with on the wire, and how to read the
//! answer back off a finished session. What is *done* with the answer is the
//! proxy engine's business, and nothing here knows that HTTP has messages.

use crate::handshake::{ClientSession, UpstreamSession};

/// An HTTP protocol a TLS session can agree to carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// HTTP/1.1, which is also what a session that negotiated nothing
    /// carries.
    Http11,
    /// HTTP/2, as `h2` — HTTP/2 over TLS. There is no ALPN name for HTTP/2
    /// without TLS, and ProxyLab does not offer one.
    Http2,
}

impl Protocol {
    /// The protocol's ALPN identifier, as it goes on the wire.
    pub(crate) fn alpn(self) -> &'static [u8] {
        match self {
            Self::Http11 => b"http/1.1",
            Self::Http2 => b"h2",
        }
    }

    /// The protocol an ALPN answer names.
    ///
    /// A session that negotiated nothing — either end left ALPN out — is
    /// HTTP/1.1: that is what HTTPS carried before ALPN existed, and it is
    /// what a peer that says nothing is expecting. HTTP/2 over TLS may only
    /// be used when it was agreed, so it is never the assumption.
    pub(crate) fn selected(alpn: Option<&[u8]>) -> Self {
        match alpn {
            Some(name) if name == Self::Http2.alpn() => Self::Http2,
            _ => Self::Http11,
        }
    }
}

/// Every protocol ProxyLab offers, in the order it prefers them.
///
/// HTTP/2 first: where both ends can speak it, it is the one to use, and a
/// peer that cannot falls through to HTTP/1.1 in the same handshake. The
/// order is the server's preference — rustls picks the first entry the other
/// end also offered — so this list is what decides, not what a client happens
/// to send first.
pub const DEFAULT_PROTOCOLS: &[Protocol] = &[Protocol::Http2, Protocol::Http11];

/// The protocol the client session agreed to.
///
/// This is ProxyLab answering a client's handshake, so the answer says which
/// HTTP the client is about to speak *to the proxy*.
pub fn client_protocol<S>(session: &ClientSession<S>) -> Protocol {
    Protocol::selected(session.get_ref().1.alpn_protocol())
}

/// The protocol the origin's session agreed to.
///
/// This is ProxyLab as a client, so the answer says which HTTP the *origin*
/// is about to speak. An origin that offers no ALPN, or picks HTTP/1.1, is
/// spoken to in HTTP/1.1: nothing here upgrades a server that did not agree
/// to be upgraded.
pub fn upstream_protocol<S>(session: &UpstreamSession<S>) -> Protocol {
    Protocol::selected(session.get_ref().1.alpn_protocol())
}

/// The ALPN list a TLS configuration offering `protocols` carries.
pub(crate) fn advertised(protocols: &[Protocol]) -> Vec<Vec<u8>> {
    protocols
        .iter()
        .map(|protocol| protocol.alpn().to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_protocol_has_the_name_it_is_known_by_on_the_wire() {
        assert_eq!(Protocol::Http11.alpn(), b"http/1.1");
        assert_eq!(Protocol::Http2.alpn(), b"h2");
    }

    #[test]
    fn the_negotiated_name_reads_back_as_the_protocol_it_names() {
        assert_eq!(Protocol::selected(Some(b"h2")), Protocol::Http2);
        assert_eq!(Protocol::selected(Some(b"http/1.1")), Protocol::Http11);
    }

    #[test]
    fn a_session_that_negotiated_nothing_is_http_1_1() {
        assert_eq!(Protocol::selected(None), Protocol::Http11);
    }

    #[test]
    fn a_name_this_build_does_not_know_is_not_taken_for_http_2() {
        // `h2c` is HTTP/2 without TLS and is not something that can be
        // negotiated here; `h3` is a later protocol entirely. Neither is
        // offered, so neither can be selected — and if one somehow were, it
        // must not be mistaken for the HTTP/2 this proxy speaks.
        for name in [&b"h2c"[..], b"h3", b"spdy/3.1", b""] {
            assert_eq!(Protocol::selected(Some(name)), Protocol::Http11);
        }
    }

    #[test]
    fn http_2_is_offered_ahead_of_http_1_1() {
        assert_eq!(DEFAULT_PROTOCOLS, [Protocol::Http2, Protocol::Http11]);
    }

    #[test]
    fn the_advertised_list_is_the_protocols_in_the_order_given() {
        assert_eq!(
            advertised(DEFAULT_PROTOCOLS),
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(advertised(&[Protocol::Http11]), [b"http/1.1".to_vec()]);
    }
}
