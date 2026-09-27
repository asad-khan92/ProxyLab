//! What ProxyLab presents when it answers a TLS handshake as a server.
//!
//! An [`Identity`] is one host's certificate, the key that goes with it, and
//! the TLS server configuration built from the pair. They are kept together
//! because they are only ever useful together, and because it means the key
//! is parsed into a configuration once per host rather than once per
//! connection.
//!
//! Nothing here reaches the key. The configuration holds it, the certificate
//! is public, and neither this type's accessors nor its [`fmt::Debug`] give
//! any way to read the private material back out.

use std::fmt;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls_pki_types::CertificateDer;

use crate::alpn::{self, Protocol};
use crate::authority::CertificateAuthority;
use crate::config::provider;
use crate::error::TlsError;
use crate::host::Host;
use crate::leaf::Leaf;

/// The certificate and TLS configuration ProxyLab serves for one host.
pub struct Identity {
    host: Host,
    leaf: Leaf,
    server_config: Arc<ServerConfig>,
}

impl Identity {
    /// Mints an identity for `host` from `ca`, offering `protocols`.
    ///
    /// `protocols` is what the certificate's TLS sessions advertise over
    /// ALPN, in preference order. It belongs to the identity because it
    /// belongs to the server configuration built from it, which is built once
    /// per host and then reused.
    pub(crate) fn issue(
        host: &Host,
        ca: &CertificateAuthority,
        protocols: &[Protocol],
    ) -> Result<Self, TlsError> {
        let leaf = Leaf::issue(host, ca)?;
        let (chain, key) = leaf.identity();

        Ok(Self {
            host: host.clone(),
            leaf,
            server_config: Arc::new(server_config(chain, key, protocols)?),
        })
    }

    /// The host this identity is for.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The certificate the client is sent.
    ///
    /// Public material, and the thing to compare when asking whether two
    /// connections were served the same certificate.
    pub fn certificate_der(&self) -> &CertificateDer<'static> {
        self.leaf.certificate()
    }

    /// The TLS server configuration built from this identity.
    pub(crate) fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.server_config)
    }
}

/// Shows which host it is for, and nothing that is secret.
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("host", &self.host.as_str())
            .field("leaf", &self.leaf)
            .finish()
    }
}

/// Builds the TLS server configuration ProxyLab answers a client with.
///
/// No client certificates are asked for: the client is a program on this
/// machine being debugged, not something this proxy authenticates.
///
/// The protocol versions are rustls's own defaults, which are TLS 1.3 and
/// TLS 1.2. Nothing older is offered, and nothing older exists in the library
/// to offer.
///
/// `protocols` is the ALPN list, in the order this server prefers them:
/// rustls picks the first entry the client also offered, so the order here is
/// what decides which HTTP the session carries.
fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: rustls_pki_types::PrivateKeyDer<'static>,
    protocols: &[Protocol],
) -> Result<ServerConfig, TlsError> {
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(TlsError::Configuration)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(TlsError::Configuration)?;

    config.alpn_protocols = alpn::advertised(protocols);

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority() -> CertificateAuthority {
        CertificateAuthority::generate().expect("generate a CA")
    }

    fn identity(host: &str, ca: &CertificateAuthority) -> Identity {
        offering(host, ca, alpn::DEFAULT_PROTOCOLS)
    }

    fn offering(host: &str, ca: &CertificateAuthority, protocols: &[Protocol]) -> Identity {
        Identity::issue(&Host::parse(host).expect("usable host"), ca, protocols)
            .expect("issue an identity")
    }

    #[test]
    fn an_identity_knows_its_host() {
        let ca = authority();
        let identity = identity("example.com", &ca);

        assert_eq!(identity.host().as_str(), "example.com");
    }

    #[test]
    fn an_identity_carries_a_certificate() {
        let ca = authority();
        let identity = identity("example.com", &ca);

        assert!(!identity.certificate_der().is_empty());
    }

    #[test]
    fn the_server_configuration_offers_http_2_and_http_1_1() {
        let ca = authority();
        let identity = identity("example.com", &ca);

        assert_eq!(
            identity.server_config().alpn_protocols,
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn an_identity_offers_exactly_the_protocols_it_was_issued_for() {
        let ca = authority();
        let identity = offering("example.com", &ca, &[Protocol::Http11]);

        assert_eq!(identity.server_config().alpn_protocols, [b"http/1.1"]);
    }

    #[test]
    fn the_configuration_is_shared_rather_than_rebuilt() {
        let ca = authority();
        let identity = identity("example.com", &ca);

        assert!(Arc::ptr_eq(
            &identity.server_config(),
            &identity.server_config()
        ));
    }

    #[test]
    fn the_debug_form_names_the_host_and_no_secret() {
        let ca = authority();
        let identity = identity("example.com", &ca);

        let described = format!("{identity:?}");

        assert!(described.contains("example.com"), "{described}");
        assert!(described.contains("elided"), "{described}");
    }
}
