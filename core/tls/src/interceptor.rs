//! The one thing the proxy engine holds.
//!
//! An [`Interceptor`] is the CA, the certificate cache, and the upstream TLS
//! configuration behind a small surface: given a host it hands back the
//! identity to answer that host's clients with, and given a socket it hands
//! back a verified session to that host's real server. The engine never sees
//! a certificate parameter, a trust store, or a private key.
//!
//! It is built once, when the proxy starts, and shared between every
//! connection. Nothing in it is per-connection, and the only mutable part is
//! the cache, which locks internally — so `&self` is all any of this needs.
//!
//! Which HTTP protocols its sessions offer over ALPN is one of its settings,
//! the same list on both sides. A proxy uses [`alpn::DEFAULT_PROTOCOLS`],
//! which offers HTTP/2 and HTTP/1.1; [`Interceptor::offering`] exists for
//! somewhere that has to speak one and only one of them — a server standing
//! in for an origin that has never heard of HTTP/2, for instance. What each
//! individual session actually agreed to is read back with
//! [`crate::client_protocol`] and [`crate::upstream_protocol`].
//!
//! One session is not negotiated at all: a WebSocket upgrade is an HTTP/1.1
//! mechanism, so [`Interceptor::connect_upstream_http11`] offers `http/1.1`
//! and nothing else. It is the same trust anchors, the same verification, and
//! the same SNI as every other upstream session — only the protocol list
//! differs, because there is no version of the handshake to carry over HTTP/2.

use std::path::Path;
use std::sync::Arc;

use rustls::ClientConfig;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::alpn::{self, Protocol};
use crate::authority::CertificateAuthority;
use crate::cache::IdentityCache;
use crate::error::TlsError;
use crate::handshake::{self, ClientSession, UpstreamSession};
use crate::host::Host;
use crate::identity::Identity;
use crate::upstream::{self, UpstreamTrust};

/// Everything interception needs, ready to serve any host.
#[derive(Debug)]
pub struct Interceptor {
    authority: CertificateAuthority,
    identities: IdentityCache,
    protocols: Vec<Protocol>,
    upstream: Arc<ClientConfig>,
    /// The same configuration with `h2` left out, for the one thing that
    /// cannot be carried over HTTP/2. Built alongside the other because a
    /// `ClientConfig` is built once and shared, and building one per upgrade
    /// would rebuild the trust store each time.
    upstream_http11: Arc<ClientConfig>,
}

impl Interceptor {
    /// Builds an interceptor from `authority`, verifying origins per `trust`.
    ///
    /// Its sessions offer HTTP/2 and HTTP/1.1 over ALPN, in that order of
    /// preference. This is what a proxy wants: each side of an intercepted
    /// connection settles on the best protocol that end can speak, and the
    /// two need not settle on the same one.
    pub fn new(authority: CertificateAuthority, trust: &UpstreamTrust) -> Result<Self, TlsError> {
        Self::offering(authority, trust, alpn::DEFAULT_PROTOCOLS)
    }

    /// The same, offering exactly `protocols` over ALPN.
    ///
    /// The list is the preference order, and it is used on both sides: a
    /// client is offered it, and so is an origin. An empty list offers no
    /// ALPN at all, which is a session whose peer is left to assume HTTP/1.1.
    pub fn offering(
        authority: CertificateAuthority,
        trust: &UpstreamTrust,
        protocols: &[Protocol],
    ) -> Result<Self, TlsError> {
        Ok(Self {
            authority,
            identities: IdentityCache::default(),
            protocols: protocols.to_vec(),
            upstream: Arc::new(upstream::client_config(trust, protocols)?),
            upstream_http11: Arc::new(upstream::client_config(trust, &[Protocol::Http11])?),
        })
    }

    /// Builds an interceptor around the CA stored in `directory`, generating
    /// and storing one if there is none, and verifying origins against the
    /// public web PKI.
    ///
    /// This is what a launching proxy calls.
    pub fn with_ca_in(directory: &Path) -> Result<Self, TlsError> {
        Self::new(
            CertificateAuthority::load_or_create(directory)?,
            &UpstreamTrust::PublicRoots,
        )
    }

    /// The certificate authority, whose certificate is public material.
    pub fn authority(&self) -> &CertificateAuthority {
        &self.authority
    }

    /// The identity to answer `host`'s clients with.
    ///
    /// Minted on first use and reused afterwards, so repeated connections to
    /// one host are served one certificate.
    pub fn identity(&self, host: &Host) -> Result<Arc<Identity>, TlsError> {
        self.identities.get_or_issue(host, || {
            Identity::issue(host, &self.authority, &self.protocols)
        })
    }

    /// Answers a client's TLS handshake as whichever host its hello names.
    ///
    /// The host is not known before the handshake starts: the connection was
    /// made to an address, and the name the client means is the one it sends
    /// in SNI. That name is read out of the hello, the identity for it is
    /// minted or reused exactly as [`accept_client`](Self::accept_client)
    /// would, and the host it settled on is returned with the session, since
    /// it is also the name the origin's certificate has to be verified
    /// against.
    ///
    /// A hello with no usable name is answered as `fallback` — the address
    /// the client connected to, and therefore what it will be checking.
    pub async fn accept_client_named<S>(
        &self,
        fallback: &Host,
        stream: S,
    ) -> Result<(Host, ClientSession<S>), TlsError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        handshake::accept_named(stream, fallback, |host| self.identity(host)).await
    }

    /// Answers a client's TLS handshake as `host`.
    pub async fn accept_client<S>(
        &self,
        host: &Host,
        stream: S,
    ) -> Result<ClientSession<S>, TlsError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let identity = self.identity(host)?;

        handshake::accept(&identity, stream).await
    }

    /// Opens a verified TLS session to `host`'s real server over `stream`.
    pub async fn connect_upstream<S>(
        &self,
        host: &Host,
        stream: S,
    ) -> Result<UpstreamSession<S>, TlsError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        handshake::connect(Arc::clone(&self.upstream), host, stream).await
    }

    /// Opens a verified TLS session to `host`'s real server that will carry
    /// HTTP/1.1.
    ///
    /// The same verification as [`Self::connect_upstream`] — the same trust
    /// anchors, the same host name check, the same SNI — offering `http/1.1`
    /// alone over ALPN. For a WebSocket upgrade, which is defined over
    /// HTTP/1.1 and has no HTTP/2 form this proxy carries: a session that
    /// agreed `h2` would be one the handshake could not be sent inside.
    pub async fn connect_upstream_http11<S>(
        &self,
        host: &Host,
        stream: S,
    ) -> Result<UpstreamSession<S>, TlsError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        handshake::connect(Arc::clone(&self.upstream_http11), host, stream).await
    }

    /// How many hosts have certificates minted for them.
    pub fn cached_hosts(&self) -> usize {
        self.identities.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::temp_dir;

    fn host(text: &str) -> Host {
        Host::parse(text).expect("usable host")
    }

    fn interceptor() -> Interceptor {
        Interceptor::new(
            CertificateAuthority::generate().expect("generate a CA"),
            &UpstreamTrust::PublicRoots,
        )
        .expect("build an interceptor")
    }

    #[test]
    fn an_interceptor_starts_with_no_certificates() {
        assert_eq!(interceptor().cached_hosts(), 0);
    }

    #[test]
    fn an_identity_is_minted_for_a_host_and_then_reused() {
        let interceptor = interceptor();
        let host = host("example.com");

        let first = interceptor.identity(&host).expect("mint an identity");
        let second = interceptor.identity(&host).expect("reuse the identity");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(interceptor.cached_hosts(), 1);
    }

    #[test]
    fn every_host_gets_its_own_identity() {
        let interceptor = interceptor();

        let first = interceptor
            .identity(&host("example.com"))
            .expect("mint an identity");
        let second = interceptor
            .identity(&host("other.example.com"))
            .expect("mint an identity");

        assert_ne!(first.certificate_der(), second.certificate_der());
        assert_eq!(interceptor.cached_hosts(), 2);
    }

    #[test]
    fn the_ca_is_the_one_the_interceptor_was_built_with() {
        let authority = CertificateAuthority::generate().expect("generate a CA");
        let certificate = authority.certificate_der().clone();

        let interceptor =
            Interceptor::new(authority, &UpstreamTrust::PublicRoots).expect("build an interceptor");

        assert_eq!(interceptor.authority().certificate_der(), &certificate);
    }

    #[test]
    fn an_interceptor_can_be_built_around_a_stored_ca() {
        let directory = temp_dir("interceptor-ca");

        let first = Interceptor::with_ca_in(directory.path()).expect("create a CA");
        let second = Interceptor::with_ca_in(directory.path()).expect("load the CA");

        assert_eq!(
            first.authority().certificate_der(),
            second.authority().certificate_der(),
            "the second launch generated a different CA"
        );
    }

    #[test]
    fn the_debug_form_carries_no_secret() {
        let interceptor = interceptor();
        interceptor
            .identity(&host("example.com"))
            .expect("mint an identity");

        let described = format!("{interceptor:?}");

        assert!(described.contains("elided"), "{described}");
    }
}
