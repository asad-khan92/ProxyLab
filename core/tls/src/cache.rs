//! Keeping the certificate for a host instead of minting it again.
//!
//! Without this, every connection to the same host would generate a key pair,
//! sign a certificate, and build a TLS configuration — work a client sees as
//! handshake latency, repeated for each of the several connections a page
//! load opens. With it, the first connection to a host pays for it and the
//! rest do not.
//!
//! In memory only, and deliberately: the cache lives as long as the process
//! and holds private keys, so writing it down would mean writing keys down.
//! Nothing is evicted, because the number of hosts one debugging session
//! visits is small and an identity is a few hundred bytes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::TlsError;
use crate::host::Host;
use crate::identity::Identity;

/// The identities minted so far, by host.
#[derive(Debug, Default)]
pub(crate) struct IdentityCache {
    entries: Mutex<HashMap<String, Arc<Identity>>>,
}

impl IdentityCache {
    /// The identity for `host`, minting it with `issue` if it is not held.
    ///
    /// `issue` runs outside the lock, so a slow signing operation for one
    /// host never delays another. Two connections to the same host arriving
    /// at once may therefore both mint a certificate; the first one to be
    /// stored is the one both of them — and everything after — is given, so
    /// what is returned is always the cached identity.
    pub(crate) fn get_or_issue<I>(&self, host: &Host, issue: I) -> Result<Arc<Identity>, TlsError>
    where
        I: FnOnce() -> Result<Identity, TlsError>,
    {
        if let Some(identity) = self.get(host) {
            return Ok(identity);
        }

        Ok(self.store(host, issue()?))
    }

    /// The identity held for `host`, if there is one.
    fn get(&self, host: &Host) -> Option<Arc<Identity>> {
        self.entries().get(host.as_str()).map(Arc::clone)
    }

    /// Stores `identity` for `host` unless one is already held, and returns
    /// whichever is now the held one.
    ///
    /// The already-held identity wins, so a host that was minted twice at
    /// once still has exactly one certificate from then on.
    fn store(&self, host: &Host, identity: Identity) -> Arc<Identity> {
        Arc::clone(
            self.entries()
                .entry(host.as_str().to_owned())
                .or_insert_with(|| Arc::new(identity)),
        )
    }

    /// How many hosts are held.
    pub(crate) fn len(&self) -> usize {
        self.entries().len()
    }

    /// The entries, whether or not a panic left the lock poisoned.
    ///
    /// A poisoned lock here means a thread panicked while holding it. The map
    /// it protects is a cache of immutable values, so there is no torn state
    /// to recover from and no reason to bring the proxy down over it.
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Identity>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::authority::CertificateAuthority;

    fn host(text: &str) -> Host {
        Host::parse(text).expect("usable host")
    }

    /// A cache, a CA to mint from, and a count of how often minting ran.
    struct Fixture {
        cache: IdentityCache,
        ca: CertificateAuthority,
        issued: AtomicUsize,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                cache: IdentityCache::default(),
                ca: CertificateAuthority::generate().expect("generate a CA"),
                issued: AtomicUsize::new(0),
            }
        }

        fn identity(&self, host: &Host) -> Arc<Identity> {
            self.cache
                .get_or_issue(host, || {
                    self.issued.fetch_add(1, Ordering::Relaxed);
                    Identity::issue(host, &self.ca, crate::alpn::DEFAULT_PROTOCOLS)
                })
                .expect("issue an identity")
        }

        fn issued(&self) -> usize {
            self.issued.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn a_new_cache_is_empty() {
        assert_eq!(IdentityCache::default().len(), 0);
    }

    #[test]
    fn the_first_request_for_a_host_mints_a_certificate() {
        let fixture = Fixture::new();

        let identity = fixture.identity(&host("example.com"));

        assert_eq!(identity.host().as_str(), "example.com");
        assert_eq!(fixture.issued(), 1);
        assert_eq!(fixture.cache.len(), 1);
    }

    #[test]
    fn a_second_request_for_a_host_reuses_the_certificate() {
        let fixture = Fixture::new();
        let host = host("example.com");

        let first = fixture.identity(&host);
        let second = fixture.identity(&host);

        assert_eq!(fixture.issued(), 1, "the certificate was minted twice");
        assert_eq!(first.certificate_der(), second.certificate_der());
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn a_host_spelled_in_another_case_is_the_same_host() {
        let fixture = Fixture::new();

        let first = fixture.identity(&host("example.com"));
        let second = fixture.identity(&host("EXAMPLE.COM"));

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(fixture.cache.len(), 1);
    }

    #[test]
    fn different_hosts_get_different_certificates() {
        let fixture = Fixture::new();

        let first = fixture.identity(&host("example.com"));
        let second = fixture.identity(&host("other.example.com"));

        assert_ne!(first.certificate_der(), second.certificate_der());
        assert_eq!(fixture.issued(), 2);
        assert_eq!(fixture.cache.len(), 2);
    }

    #[test]
    fn an_address_and_a_name_are_different_hosts() {
        let fixture = Fixture::new();

        let name = fixture.identity(&host("localhost"));
        let address = fixture.identity(&host("127.0.0.1"));

        assert_ne!(name.certificate_der(), address.certificate_der());
        assert_eq!(fixture.cache.len(), 2);
    }

    #[test]
    fn a_failure_to_mint_is_not_cached() {
        let cache = IdentityCache::default();
        let host = host("example.com");

        let error = cache
            .get_or_issue(&host, || Err(TlsError::NoHomeDirectory))
            .expect_err("minting failed");

        assert!(matches!(error, TlsError::NoHomeDirectory), "{error}");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn two_certificates_minted_at_once_do_not_both_get_used() {
        let fixture = Fixture::new();
        let host = host("example.com");
        let stored = fixture.identity(&host);

        // What two connections racing on the same host produce: a second
        // identity, minted before the first was stored, offered to the cache
        // afterwards. The stored one wins, so the two connections cannot end
        // up being served different certificates.
        let raced = fixture.cache.store(
            &host,
            Identity::issue(&host, &fixture.ca, crate::alpn::DEFAULT_PROTOCOLS)
                .expect("issue an identity"),
        );

        assert!(Arc::ptr_eq(&stored, &raced));
        assert_eq!(fixture.cache.len(), 1);
    }
}
