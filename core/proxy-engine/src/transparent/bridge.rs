//! One flow through the bridge: the `CONNECT` that announces it, and what is
//! done with the bytes that follow.
//!
//! The order is the explicit proxy's. The target is checked, the loop checks
//! are made, and the upstream socket is opened — all before the `200`, so a
//! destination that cannot be reached is refused to the application rather
//! than accepted and then dropped. Only then does the flow's first byte say
//! whether it is TLS.

use std::error::Error as StdError;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use proxy_storage::{Destination, TrafficError, TrafficErrorKind};
use proxy_tls::Interceptor;
use rules::RuleEngine;
use tokio::io::AsyncReadExt;
use tokio::net::{lookup_host, TcpStream};

use super::own::Own;
use super::plaintext;
use super::prefixed::Prefixed;
use super::SOURCE_PID_HEADER;
use crate::capture::{Capture, Recorder};
use crate::controls::Controls;
use crate::http1::connect::{self, ConnectError};
use crate::http1::connect_target::ConnectTarget;
use crate::http1::redact;
use crate::http1::reply::{self, ProxyBody};
use crate::intercept;
use crate::shutdown::Shutdown;
use crate::upstream::UpstreamManager;

/// The first byte of every TLS handshake: a record of content type
/// `handshake` (RFC 8446 §5.1), which is how a `ClientHello` arrives.
const TLS_HANDSHAKE: u8 = 0x16;

/// What a request that is not a flow is told.
pub const NOT_A_FLOW: &str = "the ProxyLab transparent bridge carries CONNECT and nothing else";

/// What a flow from this proxy's own process is told.
pub const OWN_FLOW: &str = "this connection came from ProxyLab itself";

/// What a flow to one of this proxy's own listeners is told.
pub const OWN_DESTINATION: &str = "the destination is this ProxyLab proxy";

/// The proxy's own parts, which every flow shares.
pub struct Parts {
    pub interceptor: Arc<Interceptor>,
    pub upstream: Arc<UpstreamManager>,
    pub capture: Capture,
    pub rules: Option<Arc<RuleEngine>>,
    pub controls: Controls,
    pub own: Arc<Own>,
    pub shutdown: Shutdown,
}

/// Answers one request made on the bridge.
///
/// Never fails: a flow that cannot be carried is answered with the status
/// saying why, which the provider turns into a connection the application
/// sees fail.
pub async fn answer(
    client: SocketAddr,
    request: Request<Incoming>,
    parts: Parts,
) -> Response<ProxyBody> {
    if request.method() != Method::CONNECT {
        proxy_diagnostics::warn(
            "bridge",
            "accept",
            "connection",
            &format!(
                "{client} transparent bridge refused {}: not a CONNECT",
                request.method()
            ),
        );

        return reply::error(StatusCode::METHOD_NOT_ALLOWED, NOT_A_FLOW);
    }

    // First, and before anything is recorded or connected to. A flow from
    // this process is one of the proxy's own upstream connections coming back
    // to it: recording it would record every exchange twice, and carrying it
    // would open another upstream connection, which would come back too.
    if from_this_process(&request, &parts.own) {
        println!(
            "{client} transparent bridge passed back {}: {OWN_FLOW}",
            redact::loggable(request.uri())
        );

        return reply::error(StatusCode::LOOP_DETECTED, OWN_FLOW);
    }

    let target = redact::loggable(request.uri());
    println!("{client} TRANSPARENT {target}");

    // A flow that becomes a session or a plain HTTP conversation publishes
    // nothing of its own — the requests inside it are the traffic. This
    // recording is for the one that does not get that far.
    let recorder = parts.capture.connecting(client, target.clone(), &request);

    match establish(client, request, parts, recorder.clone()).await {
        Ok(established) => {
            println!("{client} TRANSPARENT established {established}");
            connect::established()
        }
        Err(error) => {
            recorder.fail(error.traffic_error());
            proxy_diagnostics::error(
                "bridge",
                "connect",
                "connection",
                &format!(
                    "{client} TRANSPARENT {target} -> {} ({error})",
                    error.status()
                ),
            );

            reply::error(error.status(), error.reason())
        }
    }
}

/// Whether the provider says this flow came from this process.
fn from_this_process<B>(request: &Request<B>, own: &Own) -> bool {
    request
        .headers()
        .get(SOURCE_PID_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|source| own.is_source(source))
}

/// Everything a flow has to succeed at before the provider is answered.
async fn establish(
    client: SocketAddr,
    request: Request<Incoming>,
    parts: Parts,
    recorder: Recorder,
) -> Result<ConnectTarget, BridgeError> {
    let target = ConnectTarget::parse(request.uri())
        .map_err(|error| BridgeError::Connect(ConnectError::Target(error)))?;
    recorder.destination(Destination::new(target.host(), target.port()));

    // Resolved here, rather than by `connect`, so every address it could have
    // connected to is checked first. The provider sends an address, which
    // resolves to itself; a name is resolved once, and the socket goes to
    // what that resolution said.
    let addresses: Vec<SocketAddr> = lookup_host((target.host(), target.port()))
        .await
        .map_err(|error| BridgeError::Connect(ConnectError::Upstream(error)))?
        .collect();
    if addresses
        .iter()
        .any(|address| parts.own.is_destination(address))
    {
        return Err(BridgeError::Itself);
    }

    let socket = TcpStream::connect(&addresses[..])
        .await
        .map_err(|error| BridgeError::Connect(ConnectError::Upstream(error)))?;

    // The upgrade only resolves after the `200` has been written, so the flow
    // is carried in a task of its own — detached for the reason an
    // intercepted `CONNECT` is, and told when the proxy stops through the
    // same handle.
    let upgrade = hyper::upgrade::on(request);
    let carried = target.clone();
    tokio::spawn(async move {
        match upgrade.await {
            Ok(upgraded) => carry(client, carried, upgraded, socket, parts, recorder).await,
            Err(error) => {
                recorder.fail(TrafficError::new(
                    TrafficErrorKind::ClientDisconnected,
                    "the client did not open the connection it asked for",
                ));
                proxy_diagnostics::warn(
                    "bridge",
                    "connect",
                    "connection",
                    &format!("{client} TRANSPARENT {carried} was never opened: {error}"),
                );
            }
        }
    });

    Ok(target)
}

/// Carries a flow once the provider has been told it is established.
async fn carry(
    client: SocketAddr,
    target: ConnectTarget,
    upgraded: hyper::upgrade::Upgraded,
    socket: TcpStream,
    parts: Parts,
    recorder: Recorder,
) {
    let Parts {
        interceptor,
        upstream,
        capture,
        rules,
        controls,
        own: _,
        shutdown,
    } = parts;
    let mut stream = TokioIo::new(upgraded);

    // TLS and HTTP both wait for the client to speak first, so the first byte
    // is what the application sent and it says which of the two this is.
    let mut first = [0u8; 1];
    let read = tokio::select! {
        read = stream.read(&mut first) => read,
        () = shutdown.signalled() => Ok(0),
    };
    if !matches!(read, Ok(1)) {
        // Opened and closed with nothing said: there was no request, and so
        // nothing to show.
        recorder.discard();
        return;
    }
    let stream = Prefixed::new(first.to_vec(), stream);

    if first[0] == TLS_HANDSHAKE {
        let outcome = intercept::run_named(
            client,
            &target,
            &interceptor,
            &upstream,
            capture,
            rules,
            controls,
            stream,
            socket,
            shutdown,
        )
        .await;

        connect::report(client, &target, &recorder, outcome);
    } else {
        // The service opens its own connection, by the name the request
        // gives, through the same pool as every forwarded request. The socket
        // opened to prove the address reachable is not needed for that.
        drop(socket);
        recorder.discard();

        plaintext::serve(
            stream,
            client,
            target,
            interceptor,
            upstream,
            capture,
            rules,
            controls,
            shutdown,
        )
        .await;
    }
}

/// Why a flow was not carried.
#[derive(Debug)]
pub enum BridgeError {
    /// Refused for a reason a `CONNECT` through the explicit proxy would have
    /// been refused for.
    Connect(ConnectError),
    /// The destination is one of this proxy's own listeners.
    Itself,
}

impl BridgeError {
    /// Status to answer the provider with.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Connect(error) => error.status(),
            Self::Itself => StatusCode::LOOP_DETECTED,
        }
    }

    /// Explanation safe to send back.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Connect(error) => error.reason(),
            Self::Itself => OWN_DESTINATION,
        }
    }

    /// The failure as a captured exchange records it.
    pub fn traffic_error(&self) -> TrafficError {
        match self {
            Self::Connect(error) => error.traffic_error(),
            Self::Itself => TrafficError::new(TrafficErrorKind::RequestRejected, OWN_DESTINATION),
        }
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => error.fmt(f),
            Self::Itself => f.write_str(OWN_DESTINATION),
        }
    }
}

impl StdError for BridgeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Connect(error) => Some(error),
            Self::Itself => None,
        }
    }
}
