//! How ProxyLab verifies the origin server it connects to.
//!
//! Interception means the client is checking ProxyLab's certificate instead
//! of the origin's, so the origin's certificate has to be checked by
//! ProxyLab, properly, or nobody checks it at all. That is what this module
//! is for and it is the reason [`UpstreamTrust`] has the shape it has: it
//! chooses *which* certificates are trusted and offers no way to trust
//! everything. There is no variant for skipping verification and no custom
//! verifier anywhere in this crate, so a configuration that accepts an
//! unverified origin certificate cannot be built from this API.
//!
//! Verification itself — signature chain, validity dates, and the host name
//! against the certificate's subject alternative names — is rustls's own, and
//! is on by default in the configurations built here.

use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;

use crate::alpn::{self, Protocol};
use crate::config::provider;
use crate::error::TlsError;

/// Which certificates ProxyLab will accept from an origin server.
#[derive(Debug, Clone)]
pub enum UpstreamTrust {
    /// The public web PKI: the trust anchors a browser would use.
    ///
    /// This is what a running proxy uses. An origin certificate is accepted
    /// on exactly the terms it would have been accepted on had the client
    /// connected to the origin itself.
    PublicRoots,
    /// Only these certificates, and nothing else.
    ///
    /// For reaching a server whose certificate the public roots know nothing
    /// about — a local test server signed by a private CA. Verification is no
    /// weaker: the same chain, date, and host name checks are made, against
    /// these anchors instead of the public ones.
    Certificates(Vec<CertificateDer<'static>>),
}

impl UpstreamTrust {
    /// The trust anchors this choice amounts to.
    fn roots(&self) -> Result<RootCertStore, TlsError> {
        let mut roots = RootCertStore::empty();

        match self {
            Self::PublicRoots => {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            Self::Certificates(certificates) => {
                for certificate in certificates {
                    roots
                        .add(certificate.clone())
                        .map_err(TlsError::TrustAnchor)?;
                }
            }
        }

        Ok(roots)
    }
}

/// Builds the TLS client configuration ProxyLab connects to origins with.
///
/// The origin's certificate is verified against `trust`. No client
/// certificate is presented: ProxyLab is a proxy, not the client's
/// credentials.
///
/// `protocols` is what is offered over ALPN. The origin chooses from it, and
/// an origin that chooses HTTP/1.1 — or says nothing at all — is spoken to in
/// HTTP/1.1: offering HTTP/2 is not requiring it.
pub(crate) fn client_config(
    trust: &UpstreamTrust,
    protocols: &[Protocol],
) -> Result<ClientConfig, TlsError> {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(TlsError::Configuration)?
        .with_root_certificates(trust.roots()?)
        .with_no_client_auth();

    config.alpn_protocols = alpn::advertised(protocols);

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::authority::CertificateAuthority;

    fn config(trust: &UpstreamTrust) -> ClientConfig {
        client_config(trust, alpn::DEFAULT_PROTOCOLS).expect("build a client configuration")
    }

    #[test]
    fn the_public_roots_are_a_full_trust_store() {
        let roots = UpstreamTrust::PublicRoots
            .roots()
            .expect("the public roots load");

        // A web PKI trust store is dozens of anchors. The exact number
        // changes with the `webpki-roots` release; that it is a real store
        // rather than an empty one does not.
        assert!(roots.len() > 20, "only {} anchors", roots.len());
    }

    #[test]
    fn a_named_certificate_is_the_only_anchor() {
        let ca = CertificateAuthority::generate().expect("generate a CA");

        let roots = UpstreamTrust::Certificates(vec![ca.certificate_der().clone()])
            .roots()
            .expect("the CA is a usable anchor");

        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn trusting_nothing_in_particular_trusts_nothing() {
        let roots = UpstreamTrust::Certificates(Vec::new())
            .roots()
            .expect("an empty list is a valid, useless trust store");

        assert!(roots.is_empty());
    }

    #[test]
    fn something_that_is_not_a_certificate_is_not_an_anchor() {
        let error = UpstreamTrust::Certificates(vec![CertificateDer::from(vec![0, 1, 2, 3])])
            .roots()
            .expect_err("junk is not a trust anchor");

        assert!(matches!(error, TlsError::TrustAnchor(_)), "{error}");
    }

    #[test]
    fn the_client_configuration_offers_http_2_and_http_1_1() {
        let config = config(&UpstreamTrust::PublicRoots);

        assert_eq!(
            config.alpn_protocols,
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn the_client_configuration_offers_exactly_what_it_was_built_for() {
        let config = client_config(&UpstreamTrust::PublicRoots, &[Protocol::Http11])
            .expect("build a client configuration");

        assert_eq!(config.alpn_protocols, [b"http/1.1"]);
    }

    #[test]
    fn the_client_configuration_sends_sni() {
        let config = config(&UpstreamTrust::PublicRoots);

        assert!(
            config.enable_sni,
            "the origin needs the host name to serve the right certificate"
        );
    }

    #[test]
    fn a_configuration_can_only_be_built_from_a_trust_store() {
        // Verification is proved by handshake, in `tests/tls_sessions.rs`:
        // an origin whose certificate does not chain to the trust store is
        // refused. What is asserted here is the shape of the API — the only
        // way to reach a `ClientConfig` from this crate is through
        // `UpstreamTrust`, which has no variant that means "trust anything",
        // so verification is not something a caller can turn off.
        for trust in [
            UpstreamTrust::PublicRoots,
            UpstreamTrust::Certificates(Vec::new()),
        ] {
            config(&trust);
        }
    }
}
