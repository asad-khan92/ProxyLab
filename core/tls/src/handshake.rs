//! The two handshakes an intercepted connection is made of.
//!
//! Interception is two TLS sessions back to back. This module performs both,
//! so the engine deals in streams and never in rustls configurations:
//!
//! - [`accept`] answers the client's handshake as a server, using the
//!   certificate minted for the host it asked for;
//! - [`accept_named`] does the same when the host is not known in advance,
//!   reading it from the name the client's own hello asks for;
//! - [`connect`] opens a separate session to the origin server as a client,
//!   verifying the origin's real certificate.
//!
//! They are separate sessions with separate keys. Nothing from one is carried
//! into the other.

use std::sync::Arc;

use rustls::server::Acceptor;
use rustls::ClientConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::{LazyConfigAcceptor, TlsAcceptor, TlsConnector};

use crate::error::TlsError;
use crate::host::Host;
use crate::identity::Identity;

/// A TLS session in which ProxyLab is the server: the client's session.
pub type ClientSession<S> = tokio_rustls::server::TlsStream<S>;

/// A TLS session in which ProxyLab is the client: the origin's session.
pub type UpstreamSession<S> = tokio_rustls::client::TlsStream<S>;

/// Answers the client's TLS handshake with `identity`'s certificate.
///
/// Returns once the handshake is complete, so a caller that gets a stream
/// back has a client that accepted the certificate. A client that refused it
/// — the usual reason being that the development CA is not installed — is a
/// failed handshake, reported rather than worked around.
pub(crate) async fn accept<S>(identity: &Identity, stream: S) -> Result<ClientSession<S>, TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    TlsAcceptor::from(identity.server_config())
        .accept(stream)
        .await
        .map_err(TlsError::ClientHandshake)
}

/// Answers the client's TLS handshake as the host its hello names.
///
/// For a connection whose destination is an address rather than a name —
/// which is what a flow captured below the application looks like — the
/// name the client means is the one it put in SNI, and that is also the name
/// it will check the certificate against. So the hello is read first, the
/// host is taken from it, `identity` is asked for that host's certificate,
/// and only then is the handshake answered.
///
/// A hello that names no host, or names something that is not one, is
/// answered as `fallback`: the address the client connected to, which is
/// what a client that sent no SNI will be verifying against.
///
/// Nothing is parsed by hand. Reading the hello and resuming the handshake
/// afterwards is rustls's own, so the bytes the client sent are the bytes the
/// handshake is completed over.
pub(crate) async fn accept_named<S, F>(
    stream: S,
    fallback: &Host,
    identity: F,
) -> Result<(Host, ClientSession<S>), TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&Host) -> Result<Arc<Identity>, TlsError>,
{
    let hello = LazyConfigAcceptor::new(Acceptor::default(), stream)
        .await
        .map_err(TlsError::ClientHandshake)?;

    let host = hello
        .client_hello()
        .server_name()
        .and_then(|name| Host::parse(name).ok())
        .unwrap_or_else(|| fallback.clone());
    let identity = identity(&host)?;

    let session = hello
        .into_stream(identity.server_config())
        .await
        .map_err(TlsError::ClientHandshake)?;

    Ok((host, session))
}

/// Opens a verified TLS session to the origin server over `stream`.
///
/// `host` does two jobs: it is sent as SNI, so the origin serves the right
/// certificate, and it is the name the certificate that comes back must be
/// valid for. Both are rustls's own work, and there is no way to reach this
/// function with verification disabled — see [`crate::UpstreamTrust`].
pub(crate) async fn connect<S>(
    config: Arc<ClientConfig>,
    host: &Host,
    stream: S,
) -> Result<UpstreamSession<S>, TlsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    TlsConnector::from(config)
        .connect(host.server_name(), stream)
        .await
        .map_err(TlsError::UpstreamHandshake)
}
