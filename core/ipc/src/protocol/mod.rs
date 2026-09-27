//! The wire protocol: what a client may ask for, and what it is told.
//!
//! ```text
//! client ──▶ {"version":1,"request_id":"r1","type":"list_traffic","limit":50}
//! server ──▶ {"version":1,"request_id":"r1","ok":true,"type":"list_traffic","result":{…}}
//! server ──▶ {"version":1,"type":"traffic_added","exchange":{…}}
//! server ──▶ {"version":1,"type":"breakpoint_paused","paused":{…}}
//! ```
//!
//! Two operations name a file rather than carrying its contents: `export_har`
//! and `import_har`. A HAR document is the captured traffic with its bodies
//! in it, which is megabytes where a request line is 64 KiB, so the client
//! sends the path a person picked in a panel and the core writes or reads the
//! file. It is the shape a Map Local rule already has.
//!
//! One JSON object per line, in both directions. A line is a complete
//! message, which is the whole of the framing: there is no length prefix, no
//! handshake, and no multiplexing layer, because a request/response protocol
//! over a local socket does not need one.
//!
//! # Telling a response from an event
//!
//! A response carries `ok` and the `request_id` it is answering. An event
//! carries neither: it was not asked for. So `ok` is the discriminator, and
//! `request_id` is what correlates a response with the request that caused
//! it — a client may have several in flight.
//!
//! # Versioning
//!
//! Every request names [`PROTOCOL_VERSION`] and every message the server
//! sends carries it. A request naming any other version is refused rather
//! than guessed at, because a client that disagrees with the server about the
//! schema will misread whatever it is sent.
//!
//! The message bodies are the traffic model's own serialized form —
//! [`ExchangeSummary`](proxy_storage::ExchangeSummary) and
//! [`TrafficExchange`](proxy_storage::TrafficExchange) as `proxy-storage`
//! spells them. That is deliberate: the model is the one ProxyLab already
//! has, and a second set of structures here would be two models to keep in
//! step. It also means a change to the model is a change to this protocol,
//! and has to be declared as one.
//!
//! # Sensitive data
//!
//! A stored exchange holds credentials, cookies, and bodies; that is what a
//! debugging proxy is for, and a client asking for an exchange is asking for
//! exactly that. What never carries any of it is an *error*: every message
//! in [`ErrorBody`] is a fixed phrase chosen here, so nothing a client sent
//! and nothing an exchange held can be reflected back or written to a log.

mod event;
mod request;
mod response;

pub use event::{Event, EventBody};
pub use request::{
    parse, BreakpointOperation, DeveloperOperation, HarExport, HarOperation, HarSelection,
    LanOperation, ListRequest, ListStreamRequest, Operation, ParseFailure, Request, RequestId,
    RuleOperation, SavedRequestOperation, ThrottleOperation, TrafficOperation, MAX_EXPORT_ENTRIES,
    MAX_EXPORT_SELECTION, MAX_REQUEST_ID_LENGTH,
};
pub use response::{
    BreakpointDeleted, BreakpointDetail, BreakpointList, Cleared, CurlGenerated, CurlParsed,
    CurlProblem, Deleted, ErrorBody, ErrorCode, HarExported, HarImported, Outcome, PauseResolved,
    Response, RuleDeleted, RuleDetail, RuleList, SavedRequestDeleted, SavedRequestDetail,
    SavedRequestList, StreamMessageDetail, Subscribed, TrafficDetail, TrafficList, Watching,
};

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The largest request the server will read, in bytes.
///
/// A request is metadata: an operation, an exchange id, a page size, a
/// handful of filters. The largest legitimate one is a list request with
/// every filter set, which is a few hundred bytes, so 64 KiB is room to
/// spare many times over while still bounding what one client can make the
/// server hold. A line longer than this is answered with
/// [`ErrorCode::MessageTooLarge`] and skipped; the connection carries on.
///
/// It is a limit on what is *read*. A response is not bounded by it, because
/// a response is not something a client can inflate: the largest is one
/// exchange, whose size is decided by the capture limit.
///
/// One request is no longer only metadata: a `continue_breakpoint` carries an
/// edited body, and a `replay_request` carries one to send. Both are bounded
/// well inside this — see
/// [`MAX_BREAKPOINT_BODY_BYTES`](crate::breakpoint::MAX_BREAKPOINT_BODY_BYTES)
/// — so this limit has not moved and does not need to.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
