//! The settings both sides of an intercepted connection share.
//!
//! There are two TLS configurations in this crate — the server session with
//! the client and the client session with the origin — and two things about
//! them have to agree. They are here rather than in either module, so they
//! cannot drift apart.
//!
//! Which application protocols are offered is one of them, and it lives in
//! [`crate::alpn`] because it is a vocabulary rather than a setting. Both
//! sides offer the same list, so a client that can speak HTTP/2 to ProxyLab
//! is offered it, and so is an origin ProxyLab connects to — the two
//! handshakes still answer independently.

use std::sync::{Arc, OnceLock};

use rustls::crypto::{ring, CryptoProvider};

/// The cryptography behind both configurations.
///
/// `ring`, chosen once for the whole crate: the certificate generator uses it
/// too, so there is one implementation of the primitives rather than two.
/// Built once and shared, because a provider is a table of algorithms with
/// nothing per-connection in it.
pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();

    Arc::clone(PROVIDER.get_or_init(|| Arc::new(ring::default_provider())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_is_built_once_and_shared() {
        assert!(Arc::ptr_eq(&provider(), &provider()));
    }

    #[test]
    fn the_provider_offers_cipher_suites_and_key_exchanges() {
        let provider = provider();

        assert!(!provider.cipher_suites.is_empty());
        assert!(!provider.kx_groups.is_empty());
    }

    #[test]
    fn no_protocol_older_than_tls_1_2_can_be_negotiated() {
        // rustls implements TLS 1.2 and TLS 1.3 and nothing else, so SSLv3,
        // TLS 1.0 and TLS 1.1 are not merely disabled here — there is no code
        // in the library that could speak them. This is the assertion that
        // notices if that ever stops being true.
        let versions: Vec<_> = rustls::DEFAULT_VERSIONS
            .iter()
            .map(|version| version.version)
            .collect();

        assert_eq!(
            versions,
            [
                rustls::ProtocolVersion::TLSv1_3,
                rustls::ProtocolVersion::TLSv1_2
            ]
        );
    }
}
