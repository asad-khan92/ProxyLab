//! The bridge a transparently captured flow enters the proxy through.
//!
//! macOS's Network Extension hands ProxyLab's transparent proxy provider each
//! TCP flow an application opens, before a byte of it has left the machine.
//! The provider does not proxy anything itself: it opens a loopback
//! connection to this listener, says where the flow was going, and then
//! copies bytes. Everything a flow is made of — TLS interception, HTTP/1.1 and
//! HTTP/2, WebSockets, rules, breakpoints, throttling, capture — happens here,
//! in the same code a request through the explicit proxy goes through:
//!
//! ```text
//!   application ──▶ Network Extension ──▶ 127.0.0.1:9091 ──▶ this module
//!                                                              │
//!                          first byte 0x16 ◀───────────────────┤
//!                                 │                            │ anything else
//!                                 ▼                            ▼
//!                  intercept::run_named               plaintext: service::proxy
//!                  (SNI names the host)               (Host names the host)
//! ```
//!
//! # The bridge speaks `CONNECT`
//!
//! A flow is announced with `CONNECT address:port HTTP/1.1` and answered
//! `200 Connection Established`, exactly as a tunnel through the explicit
//! proxy is. There is no framing of ProxyLab's own to specify, parse, or get
//! wrong: the target is read by [`ConnectTarget`], and the upstream socket is
//! opened before the `200` so that an unreachable destination is refused to
//! the application rather than accepted and then dropped.
//!
//! It is still a separate listener, and a separate port, because two things
//! about a flow differ from a `CONNECT` a client wrote, and neither may
//! change what the explicit proxy does:
//!
//! - **The host is not in the target.** The application connected to an
//!   address. The name it means is the one in its TLS hello, so that is the
//!   name the certificate is minted for and the origin is verified as — see
//!   [`crate::intercept::run_named`]. An explicit `CONNECT` to an address is
//!   answered as that address, as it always was.
//! - **The flow need not be TLS.** An application talking to port 80 sends a
//!   plain HTTP/1.1 request in origin form. So the first byte decides: a TLS
//!   handshake record goes to interception, and anything else is read as
//!   HTTP, turned into the absolute-form request a proxy expects, and handed
//!   to [`crate::http1::service::proxy`] — the explicit proxy's own path.
//!
//! # Loops
//!
//! The proxy's own connections to origin servers are TCP flows like any
//! other, and a provider that handed them back here would have the proxy
//! connecting to itself without end. Two checks, both here and both
//! deterministic, stop that:
//!
//! - The provider names the process a flow came from in
//!   [`SOURCE_PID_HEADER`]. A flow from this process is refused with
//!   `508 Loop Detected` before anything is connected to, and the provider
//!   carries it directly instead.
//! - A destination that is one of this proxy's own listeners is refused, so
//!   nothing can ask the bridge to connect to the bridge.
//!
//! The provider also never offers a loopback flow at all; see
//! `ARCHITECTURE.md`.
//!
//! # Only this Mac
//!
//! The listener is bound to loopback and to nothing else, and the port is
//! fixed so the provider knows where to find it. It is no wider an opening
//! than the explicit listener beside it on `127.0.0.1:9090`.

mod bridge;
mod own;
mod plaintext;
mod prefixed;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use proxy_tls::Interceptor;
use rules::RuleEngine;
use tokio::net::TcpStream;

use crate::capture::Capture;
use crate::controls::Controls;
use crate::shutdown::Shutdown;
use crate::upstream::UpstreamManager;

pub use own::Own;

/// Where the bridge listens.
///
/// Loopback only, and fixed: the provider is told this port when it starts
/// and has no other way to find the core. The macOS app mirrors it in
/// `CoreLocations.swift`.
pub const DEFAULT_TRANSPARENT_ADDR: &str = "127.0.0.1:9091";

/// The header a flow's `CONNECT` names its source process in.
///
/// A decimal process identifier, and nothing else. It is how the proxy
/// recognises its own outbound connections coming back to it — see the
/// module documentation — and it is never recorded or logged.
pub const SOURCE_PID_HEADER: &str = "proxylab-source-pid";

/// Serves one connection from the transparent proxy provider.
///
/// One connection is one flow: a `CONNECT` naming where the application was
/// going, and then the application's own bytes. Anything that is not a
/// `CONNECT` is refused, because nothing else is a flow.
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    stream: TcpStream,
    client: SocketAddr,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    own: Arc<Own>,
    shutdown: Shutdown,
) {
    let service = service_fn(move |request| {
        let parts = bridge::Parts {
            interceptor: Arc::clone(&interceptor),
            upstream: Arc::clone(&upstream),
            capture: capture.clone(),
            rules: rules.clone(),
            controls: controls.clone(),
            own: Arc::clone(&own),
            shutdown: shutdown.clone(),
        };

        async move { Ok::<_, Infallible>(bridge::answer(client, request, parts).await) }
    });

    let served = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;

    if let Err(error) = served {
        eprintln!("{client} transparent bridge connection error: {error}");
    }
}
