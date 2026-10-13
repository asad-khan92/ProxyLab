//! The IPC boundary between the core and the macOS UI.
//!
//! ```text
//! SwiftUI macOS app
//!         │
//!         │  Unix domain socket, one JSON object per line
//!         ▼
//!   proxy-ipc server
//!         │
//!         ├──▶ proxy-storage   the stored traffic: list, get, delete, clear
//!         ├──▶ rule engine     rules: list, get, create, update, delete
//!         ├──▶ proxy-curl      a command read as a request, a request written as one
//!         ├──▶ proxy-har       traffic written out as a HAR file, and one read back in
//!         ├──▶ RequestReplayer a request sent again, through the proxy itself
//!         ├──▶ LanControl      whether the proxy is also on the local network
//!         ├──▶ BreakpointControl  exchanges held still while they are edited
//!         ├──▶ ThrottleControl    how slow the proxy's own link pretends to be
//!         ├──▶ event stream    every exchange the proxy finishes
//!         └──▶ pause stream    every exchange a breakpoint is holding
//! ```
//!
//! This crate is the only way anything outside the core reaches ProxyLab's
//! traffic. It owns the socket, the protocol, and the operations a client may
//! perform; it owns no queries, no schema, and no proxy state.
//!
//! It owns no networking either. A replay has to go through the rule engine,
//! ProxyLab's TLS, the capture, and the storage, all of which live in
//! `proxy-engine` — which depends on this crate. So [`RequestReplayer`] is
//! declared here and implemented there, and this crate never opens a
//! connection to anything. See [`replay`]. [`LanControl`] is declared the
//! same way and for the same reason: binding a second proxy listener is the
//! engine's work, and this crate only carries the request for it. See
//! [`lan`]. [`BreakpointControl`] and [`ThrottleControl`] are declared the
//! same way again: holding a request still and pacing a body are things done
//! to a connection, and the connections are the engine's. See [`breakpoint`]
//! and [`throttle`].
//!
//! # The boundary goes one way
//!
//! Nothing here knows what a UI is. There is no Swift, no AppKit, and no
//! assumption that a client is a macOS application at all — a client is
//! whatever connects to the socket and speaks the protocol, which makes the
//! server testable with a few lines of socket code and usable from `nc`.
//!
//! # It is a security boundary
//!
//! Everything ProxyLab has ever captured is reachable through this socket:
//! credentials, cookies, tokens, and bodies. So it is a Unix socket rather
//! than a TCP port — unreachable from another machine by construction — in a
//! directory created `0700`, with the socket itself set to `0600`.
//!
//! That boundary is the socket's, and it does not stretch over a network.
//! Putting the proxy on the LAN — [`lan`] — widens what can reach the
//! *proxy*, and nothing else: the IPC socket stays exactly where it is, so
//! the captured traffic, the rules, and the controls remain reachable only
//! by this user on this machine.
//!
//! There is no authentication beyond that, deliberately. The socket's
//! permissions already say what the boundary is: this user, on this machine.
//! A password or a token on top would be a secret that has to live somewhere
//! the same user can read, which is the same boundary again with more moving
//! parts. If ProxyLab ever serves a client that is not the local user, that
//! is the point at which authentication becomes a real question rather than
//! a ritual.
//!
//! # Nothing sensitive is logged
//!
//! The server logs connection-level failures and storage-level failures, and
//! neither can carry captured traffic: every protocol error message is a
//! fixed phrase chosen in this crate, a storage error names a file and an
//! operating-system error, and no request, header, or body is ever written
//! out. A client's own text is never echoed either, not even an unknown
//! operation's name.
//!
//! # Unix only
//!
//! There is no portable fallback and no TCP path. ProxyLab is a macOS
//! debugging proxy, and a Unix socket is what gives it a local-user boundary
//! for free.

pub mod breakpoint;
pub mod diagnostics;
pub mod error;
pub mod lan;
pub mod protocol;
pub mod replay;
pub mod service;
pub mod socket;
pub mod throttle;

mod connection;
mod server;

pub use breakpoint::{
    Breakpoint, BreakpointControl, BreakpointDirection, BreakpointId, BreakpointRefusal,
    BreakpointUpdate, NewBreakpoint, PauseDecision, PauseId, PauseNotice, PauseStream,
    PausedExchange, PausedRequest, PausedResponse, PausedWebSocketMessage, ReleaseReason,
    RequestEdit, ResponseEdit, WebSocketEdit, MAX_BREAKPOINT_BODY_BYTES,
};
pub use diagnostics::{
    ComponentStatus, DiagnosticEvent, DiagnosticsControl, DiagnosticsFacts, DiagnosticsReport,
    EndpointStatus, OPEN,
};
pub use error::IpcError;
pub use lan::{LanAddress, LanControl, LanFuture, LanRefusal, LanStatus};
pub use protocol::{
    ErrorBody, ErrorCode, Event, EventBody, HarExported, HarImported, Outcome, Request, Response,
    MAX_REQUEST_BYTES, PROTOCOL_VERSION,
};
pub use replay::{ReplayFuture, ReplayOutcome, RequestReplayer};
pub use server::IpcServer;
pub use service::{load_rules, TrafficService};
pub use socket::{IpcConfig, SOCKET_FILE};
pub use throttle::{
    ThrottleControl, ThrottleRefusal, ThrottleSettings, MAX_THROTTLE_LATENCY_MS,
    MIN_THROTTLE_BYTES_PER_SECOND,
};
