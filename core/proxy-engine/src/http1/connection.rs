//! Speaking HTTP/1.1 on an accepted client connection.
//!
//! This is the seam between the listener, which deals in sockets, and the
//! proxy service, which deals in requests and responses.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use proxy_tls::Interceptor;
use rules::RuleEngine;
use tokio::net::TcpStream;

use super::service;
use crate::capture::Capture;
use crate::controls::Controls;
use crate::shutdown::Shutdown;
use crate::upstream::UpstreamManager;

/// Serves one client connection until its HTTP exchange is finished.
///
/// The proxy answers one request per connection and then closes it, which
/// each response says for itself with `Connection: close` (see
/// [`super::headers::set_close`]). That is the connection to the *client*;
/// the connection to the origin is pooled and reused — see
/// [`crate::upstream`] — and the two are independent.
///
/// Keep-alive is therefore left at hyper's default rather than switched off:
/// with it off, hyper stamps `Connection: close` onto every response it
/// writes, including the `200 Connection Established` that opens a tunnel,
/// where that header contradicts the connection being handed over. The
/// connection still closes after one exchange either way.
///
/// Upgrades are enabled, because that is how a `CONNECT` takes the connection
/// over: once the response is written, hyper hands the socket to whoever asked
/// for the upgrade instead of closing it. For ProxyLab that is the TLS
/// interception in [`super::intercept`].
///
/// A client that sends something that is not an HTTP request gets hyper's
/// `400 Bad Request` and the connection ends; nothing here panics on bad
/// input.
///
/// `capture` is what the exchange is recorded into. It is shared with every
/// other connection; what belongs to this one is created per request.
///
/// `rules` is shared the same way, and is `None` for a proxy running without
/// a rule engine — which allows everything. Each request takes its own
/// snapshot of it, so a rule change never reaches a request half-applied.
///
/// `upstream` is the connections the proxy is holding open to origin servers.
/// It is shared with every other client connection, which is the point: a
/// connection this request opens to an origin is there for the next request
/// to any client to use.
///
/// `shutdown` is passed on rather than used here: this connection is over
/// after one exchange, but the intercepted connection a `CONNECT` on it opens
/// may not be, and that one has to be told when the proxy is stopping.
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    stream: TcpStream,
    client: SocketAddr,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
) {
    let service = service_fn(move |request| {
        let interceptor = Arc::clone(&interceptor);
        let upstream = Arc::clone(&upstream);
        let capture = capture.clone();
        let rules = rules.clone();
        let controls = controls.clone();
        let shutdown = shutdown.clone();

        async move {
            Ok::<_, Infallible>(
                service::proxy(
                    client,
                    request,
                    interceptor,
                    upstream,
                    &capture,
                    rules,
                    controls,
                    shutdown,
                )
                .await,
            )
        }
    });

    let served = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;

    if let Err(error) = served {
        eprintln!("{client} connection error: {error}");
    }
}
