//! WebSocket: the handshake is HTTP, and everything after it is not.
//!
//! ```text
//! GET /socket  Upgrade: websocket ──▶ rules ──▶ origin
//!                                                 │
//!            101 Switching Protocols ◀────────────┘
//!                     │
//!                     └──▶ the connection stops being HTTP
//!                          and becomes two streams of bytes
//! ```
//!
//! Two quite different things happen on one connection, and the split is what
//! this module is for. Up to the `101`, a WebSocket upgrade is an ordinary
//! HTTP request: it has a URL, it is matched against the rules, it is captured,
//! and it is carried to the origin like any other. That request and the
//! origin's answer are the exchange ProxyLab stores — see below. After the
//! `101`, there is no request and no response, only a long-lived pair of byte
//! streams, and [`tunnel`] joins them.
//!
//! # Why this is not the ordinary path
//!
//! Two assumptions the rest of the proxy is built on stop being true here.
//!
//! The first is that a connection to an origin carries a request, gets a
//! response, and is then free for the next request. A `101` ends that: the
//! socket is not an HTTP connection any more, and returning it to the pool
//! would hand the next request a socket with somebody's WebSocket on it. So
//! the handshake goes out on a connection opened for it alone — see
//! [`UpstreamManager::open_upgradable`] — and that connection is never pooled,
//! never registered, and never shared.
//!
//! The second is that hop-by-hop headers never cross the proxy. `Connection`
//! and `Upgrade` are hop-by-hop, and stripping them is right for every other
//! request and fatal for this one: an origin that is not asked to upgrade does
//! not upgrade. They are put back deliberately, in the proxy's own words
//! rather than the client's, so what reaches the origin asks for exactly one
//! thing and carries nothing else the client happened to list.
//!
//! # What is captured
//!
//! The handshake is one [`TrafficExchange`](proxy_storage::TrafficExchange),
//! stored as soon as the `101` is known so the inspector can open the socket
//! while it is still carrying frames. Each WebSocket *message* after that is
//! stored beside it: direction, text vs binary, timestamp, size, and payload,
//! bounded per message and per connection. Bytes that are not frames are
//! forwarded unchanged and are not captured as messages.
//!
//! The stored exchange still says what it is without anything being added to
//! the model: a `101` whose response headers name `Upgrade: websocket` is a
//! WebSocket handshake, and nothing else in HTTP looks like that.
//!
//! # TLS
//!
//! `wss://` is this same path inside an intercepted TLS session, and nothing
//! about it is weaker. The client checks ProxyLab's certificate, ProxyLab
//! verifies the origin's against the same trust anchors with the same host
//! name check and the same SNI, and the only difference from any other
//! upstream session is that this one offers `http/1.1` alone over ALPN —
//! because the handshake has no HTTP/2 form this proxy carries.

mod detect;
pub(crate) mod frame;
mod tunnel;

use std::error::Error as StdError;
use std::net::SocketAddr;

use hyper::body::{Body, Bytes, Incoming};
use hyper::{Request, Response, StatusCode, Version};

pub use detect::is_upgrade;

/// Encodes a text WebSocket message, masked for client-to-server.
pub(crate) fn encode_text(payload: &[u8], mask: bool) -> Vec<u8> {
    frame::encode_message(frame::Opcode::Text, payload, mask)
}

use crate::capture::Recorder;
use crate::controls::{Controls, Exchange};
use crate::http1::headers;
use crate::http1::reply::{self, ProxyBody};
use crate::http1::service;
use crate::http1::upstream::UpstreamError;
use crate::shutdown::Shutdown;
use crate::upstream::{self, UpstreamDestination, UpstreamManager};

/// Carries a WebSocket handshake to the origin, and joins the two connections
/// if it succeeds.
///
/// `request` is the request already rewritten for the origin — origin form,
/// `Host` set, hop-by-hop headers stripped — exactly as an ordinary forwarded
/// request would be. The upgrade headers this protocol needs are put back
/// here, because putting them back is the one thing that makes this request
/// different from any other, and it should be visible in one place.
///
/// The response that comes back is what goes to the client: a `101` if the
/// origin agreed to upgrade, and whatever the origin said instead if it did
/// not. An origin that refuses the upgrade has simply answered an HTTP
/// request, and that answer is forwarded like any other.
#[allow(clippy::too_many_arguments)]
pub async fn forward<B>(
    client: SocketAddr,
    url: &str,
    destination: &UpstreamDestination,
    upstream: &UpstreamManager,
    recorder: &Recorder,
    exchange: &Exchange,
    request: Request<B>,
    controls: &Controls,
    shutdown: Shutdown,
) -> Result<Response<ProxyBody>, UpstreamError>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    let (mut parts, body) = request.into_parts();
    headers::set_websocket_upgrade(&mut parts.headers);
    let mut request = Request::from_parts(parts, body);

    // Taken before the request is sent, and taken from the request rather
    // than from the response: this is the *client's* half of the connection,
    // which hyper hands over once the `101` has been written back to it.
    let client_upgrade = hyper::upgrade::on(&mut request);

    let request = request
        .map(|body| upstream::boxed(controls.throttle().upload(recorder.capture_request(body))));

    recorder.upstream_connect_started();
    let mut sender = upstream.open_upgradable(destination).await?;
    let mut response = sender
        .send_request(request)
        .await
        .map_err(UpstreamError::Send)?;
    recorder.upstream_responded(&response);

    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        // The origin answered the handshake with an ordinary response — a
        // `404`, a `403`, a redirect. There is no upgrade, so there is no
        // tunnel: this is a request that got an answer, and it is forwarded
        // as one. The connection it came in on is this exchange's alone and
        // closes with it, because it was never a pooled connection.
        println!(
            "{client} WebSocket {url} refused by the origin -> {}",
            response.status()
        );

        // A refused handshake is not transformed: see [`crate::modify`].
        return Ok(service::into_client_response(
            response,
            recorder,
            exchange,
            controls,
            &shutdown,
            &[],
        )
        .await);
    }

    let origin_upgrade = hyper::upgrade::on(&mut response);

    // The handshake is traffic of its own, stored before any message is,
    // because messages hang off it. Announcing here is what lets the
    // inspector open the socket while it is still carrying frames.
    recorder.announce();

    // Spawned rather than awaited: the client's half is handed over only once
    // the `101` below has been written, and that cannot happen until this
    // function has returned it.
    // The ceilings are read now rather than inside the tunnel: a WebSocket
    // may be open for hours, and one that changed speed half way through
    // would be a link nothing is simulating.
    let settings = controls.throttle().settings();

    tokio::spawn(tunnel::join(
        client,
        url.to_owned(),
        client_upgrade,
        origin_upgrade,
        recorder.stream_tap(),
        exchange.clone(),
        std::sync::Arc::clone(controls.breakpoints()),
        settings,
        shutdown,
    ));

    Ok(switching(response))
}

/// The `101` to send back to the client.
///
/// The origin's own headers, passed through as they arrived. That is not
/// laziness: `Sec-WebSocket-Accept` is computed from the key the client chose,
/// and the client checks it, so a proxy that edited this header section would
/// be a proxy that broke the handshake it was carrying. The negotiated
/// subprotocol and extensions are the origin's answers too, and are the
/// client's to read.
///
/// `Connection` and `Upgrade` are set rather than trusted: they are what tell
/// hyper that this response hands the socket over, and a `101` that reached
/// here without them is one nothing could act on.
fn switching(response: Response<Incoming>) -> Response<ProxyBody> {
    let (mut parts, _) = response.into_parts();

    parts.status = StatusCode::SWITCHING_PROTOCOLS;
    parts.version = Version::HTTP_11;
    headers::set_websocket_upgrade(&mut parts.headers);

    // A `101` has no body. The origin's body is dropped here rather than
    // forwarded, because what follows a `101` on that socket is not a body —
    // it is the WebSocket, and the tunnel is what carries it.
    Response::from_parts(parts, reply::empty())
}
