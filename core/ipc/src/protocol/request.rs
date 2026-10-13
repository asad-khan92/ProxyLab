//! What a client asks for, and whether it is allowed to.
//!
//! Everything a client sends arrives here first, and nothing past here is
//! client-shaped: [`parse`] either produces a [`Request`] whose parameters
//! have already been checked, or an error saying which check failed. So the
//! server never holds a page size it has not bounded, a host filter of
//! unknown length, or an exchange id that is not an id.
//!
//! ```text
//! bytes ──▶ JSON ──▶ request id ──▶ version ──▶ operation ──▶ parameters
//!            │         │              │            │             │
//!         invalid   missing       unsupported   unknown       invalid
//!          json    request_id      version     operation    parameters
//! ```
//!
//! The order is what lets an error name the request it is about: the request
//! id is read before anything else can fail, so every failure after that
//! point is correlated. A failure before it is answered with a null id,
//! which is all the server honestly knows.
//!
//! # Nothing a client sent is repeated back
//!
//! Every message is a fixed phrase. An unknown operation is not quoted, an
//! oversized host filter is not echoed, and a malformed id is not reflected:
//! a client's own text going back out is how one client's junk ends up in
//! another reader's log.

use serde::Deserialize;
use serde_json::{Map, Value};

use proxy_http::{RequestParts, RequestSpec};
use proxy_storage::{
    Cursor, ExchangeId, NewSavedRequest, SavedRequestId, SavedRequestUpdate, StatusFilter,
    StreamMessageId, Timestamp, TrafficFilter, TrafficOrder, TrafficQuery, MAX_PAGE_LIMIT,
};
use rules::{NewRule, RuleId, RuleUpdate};

use crate::breakpoint::{
    BreakpointId, BreakpointRefusal, BreakpointUpdate, NewBreakpoint, PauseDecision, PauseId,
    RequestEdit, ResponseEdit, WebSocketEdit, MAX_BREAKPOINT_BODY_BYTES,
};
use crate::throttle::ThrottleSettings;

use super::response::{ErrorBody, ErrorCode};
use super::PROTOCOL_VERSION;

/// The longest address an `enable_lan` may name.
///
/// An IPv6 literal with a zone identifier is the longest address there is,
/// and it is well under this. Generous for that, and short enough that the
/// field is never a place to put something else.
const MAX_LAN_ADDRESS_LENGTH: usize = 64;

/// The longest request id the server will correlate with.
///
/// A request id is a client's own label for a request it is waiting on — a
/// counter, or a UUID. Long enough for any of those, short enough that it is
/// never a place to put something else.
pub const MAX_REQUEST_ID_LENGTH: usize = 128;

/// The longest host a filter may name.
///
/// A DNS name is at most 253 characters and the model stores what was
/// captured, so anything longer than a generous 255 is not a host.
const MAX_HOST_LENGTH: usize = 255;

/// The longest method a filter may name.
const MAX_METHOD_LENGTH: usize = 64;

/// The longest path a HAR export or import may name.
///
/// A file system path, and macOS's own limit is 1024 bytes. Generous for
/// that, and short enough that the field is never a place to put something
/// else. Whether the path is one the core will write to or read from is
/// decided in `proxy-har`, which is what opens it.
const MAX_PATH_LENGTH: usize = 4_096;

/// The most exchanges an export may name one by one.
///
/// A selection is what somebody has picked in a list, and a request carrying
/// more ids than this would be approaching the line limit anyway. An export
/// of everything a filter matches names no ids at all and is bounded by
/// [`MAX_EXPORT_ENTRIES`] instead.
pub const MAX_EXPORT_SELECTION: usize = 500;

/// The most exchanges one export will write.
///
/// A bound on how long an export runs and how large the file gets, for the
/// case where the client named a filter rather than a list of exchanges.
pub const MAX_EXPORT_ENTRIES: usize = 10_000;

/// The longest name a saved request may be given when one is saved out of
/// captured traffic.
///
/// The model's own limit, spelled here so the boundary refuses an oversized
/// one rather than the storage layer doing it a moment later.
const MAX_SAVED_REQUEST_NAME_LENGTH: usize = proxy_storage::MAX_SAVED_REQUEST_NAME_LENGTH;

/// The longest search a filter may carry.
///
/// A search is something a person typed into a box, and the longest useful
/// one is a URL. Generous for that, and short enough that it is never a place
/// to put something else.
const MAX_SEARCH_LENGTH: usize = 512;

/// A request that has been read and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The client's label for this request, echoed in the response.
    pub id: RequestId,
    /// What it is asking for.
    pub operation: Operation,
}

/// A client's label for one request.
///
/// Checked on the way in: present, not empty, not longer than
/// [`MAX_REQUEST_ID_LENGTH`], and made only of printable characters, so what
/// is echoed back into a response is something the server has looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestId(String);

impl RequestId {
    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a request is asking the server to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Something about the stored traffic.
    Traffic(TrafficOperation),
    /// Something about the configured rules.
    Rule(RuleOperation),
    /// Something to do with cURL, or with sending a request again.
    Developer(DeveloperOperation),
    /// Something about listening on the local network as well as loopback.
    Lan(LanOperation),
    /// Something about the breakpoints, or about an exchange one is holding.
    Breakpoint(BreakpointOperation),
    /// Something about how the proxy is shaping the traffic it carries.
    Throttle(ThrottleOperation),
    /// Something about the requests somebody has kept.
    Saved(SavedRequestOperation),
    /// Writing captured traffic out as a HAR file, or reading one in.
    Har(HarOperation),
    /// Send this connection every exchange from now on.
    ///
    /// Not a traffic operation: it changes what this one connection does
    /// rather than asking the history a question, so it is answered by the
    /// connection and never reaches the storage layer.
    Subscribe,
    /// Send this connection every paused exchange from now on.
    ///
    /// Not a breakpoint operation, for the same reason `Subscribe` is not a
    /// traffic one: it changes what this one connection does. It is also
    /// what makes the proxy pause at all — see
    /// [`PauseStream`](crate::breakpoint::PauseStream).
    WatchBreakpoints,
    /// Where the proxy process stands, and the recent problems it has kept.
    ///
    /// Not a traffic operation: it reads the listeners, the LAN control, and
    /// an in-memory buffer. It does not read a captured exchange.
    Diagnostics,
}

/// A request about the requests somebody has kept.
///
/// Separate from [`DeveloperOperation`] although both are about a request
/// somebody wants sent: those three turn a request from one shape into
/// another or send it and keep nothing, and these five are a small store with
/// a lifetime of its own. They reach different parts of the server — these
/// reach the database, and none of them opens a connection to anything.
///
/// Sending a saved request is not among them, and deliberately. A replay is
/// `replay_request`, which every other request already goes through: a client
/// reads the saved request, shows it, lets it be changed, and asks for the
/// request to be sent. A second way in would be a second replay path to keep
/// in step with the rules, the capture, and the storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedRequestOperation {
    /// Every saved request, in the order they were saved.
    List,
    /// One saved request.
    Get(SavedRequestId),
    /// Keep a request a client has described.
    Create(NewSavedRequest),
    /// Keep the request of one captured exchange, under this name.
    CreateFromTraffic(ExchangeId, String),
    /// Change one that exists.
    Update(SavedRequestId, SavedRequestUpdate),
    /// Remove one.
    Delete(SavedRequestId),
}

impl SavedRequestOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List => "list_saved_requests",
            Self::Get(_) => "get_saved_request",
            Self::Create(_) => "create_saved_request",
            Self::CreateFromTraffic(_, _) => "save_request_from_traffic",
            Self::Update(_, _) => "update_saved_request",
            Self::Delete(_) => "delete_saved_request",
        }
    }
}

/// A request to write captured traffic out as a HAR file, or to read one in.
///
/// The one pair of operations that names a path. A HAR file is megabytes of
/// traffic with its bodies in it and one IPC request is 64 KiB, so the
/// document cannot travel over the socket in either direction: the core
/// writes and reads the file, and the client sends the path a person picked
/// in a save or open panel. It is the shape a Map Local rule already has —
/// the client names a file and the core is what opens it — and it is checked
/// the same way, here for its shape and in `proxy-har` for what is at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarOperation {
    /// Write these exchanges, or the ones this filter matches, to this file.
    Export(HarExport),
    /// Read this file, and keep what it describes as captured traffic.
    Import(String),
}

impl HarOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Export(_) => "export_har",
            Self::Import(_) => "import_har",
        }
    }
}

/// A checked request to write a HAR file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarExport {
    /// Where to write it.
    pub path: String,
    /// Which exchanges go in it.
    pub selection: HarSelection,
}

/// Which exchanges an export is for.
///
/// One or the other, never both: an export names the exchanges somebody
/// picked, or it names how the list they are looking at is narrowed. A
/// request carrying ids *and* filters is refused rather than resolved, since
/// either reading of it would export something nobody asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarSelection {
    /// These exchanges, by id, in the order they are stored in.
    Exchanges(Vec<ExchangeId>),
    /// Every exchange this filter matches, newest first.
    Filtered(TrafficFilter),
}

/// A request about the breakpoints, or about an exchange one is holding.
///
/// Separate from [`RuleOperation`] because a breakpoint is not a rule: a rule
/// decides by itself and a breakpoint asks a person. They reach different
/// parts of the server for that reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BreakpointOperation {
    /// Every configured breakpoint.
    List,
    /// Configure a new one.
    Create(NewBreakpoint),
    /// Change one that exists.
    Update(BreakpointId, BreakpointUpdate),
    /// Remove one.
    Delete(BreakpointId),
    /// Let a paused exchange carry on, with whatever it carries changed.
    Continue(PauseId, PauseDecision),
    /// Stop a paused exchange here.
    Cancel(PauseId),
}

impl BreakpointOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List => "list_breakpoints",
            Self::Create(_) => "create_breakpoint",
            Self::Update(_, _) => "update_breakpoint",
            Self::Delete(_) => "delete_breakpoint",
            Self::Continue(_, _) => "continue_breakpoint",
            Self::Cancel(_) => "cancel_breakpoint",
        }
    }
}

/// A request about how the proxy is shaping the traffic it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThrottleOperation {
    /// How it is being shaped right now.
    Status,
    /// Shape it this way from the next exchange on.
    Set(ThrottleSettings),
}

impl ThrottleOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Status => "throttle_status",
            Self::Set(_) => "set_throttle",
        }
    }
}

/// A request about the rules the proxy applies.
///
/// Separate from [`TrafficOperation`] because they are separate things: a
/// traffic operation asks the history a question, and a rule operation
/// changes what the proxy will do next. They reach different parts of the
/// server for that reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleOperation {
    /// Every rule, in the order they are consulted in.
    List,
    /// One rule.
    Get(RuleId),
    /// Store a new rule.
    Create(NewRule),
    /// Change a rule that exists.
    Update(RuleId, RuleUpdate),
    /// Remove a rule.
    Delete(RuleId),
}

impl RuleOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List => "list_rules",
            Self::Get(_) => "get_rule",
            Self::Create(_) => "create_rule",
            Self::Update(_, _) => "update_rule",
            Self::Delete(_) => "delete_rule",
        }
    }
}

/// A request about cURL, or about sending a request again.
///
/// Separate from the other two for the same reason they are separate from
/// each other: a traffic operation asks the history a question, a rule
/// operation changes what the proxy will do next, and these two turn a
/// request from one shape into another or send one. They reach different
/// parts of the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeveloperOperation {
    /// Read a cURL command as the request it describes.
    ///
    /// Answered without touching the database, the rules, or the proxy: it
    /// is a parse, and the text is carried in the request itself.
    ParseCurl(String),
    /// Write a stored request out as the cURL command that would send it
    /// again.
    CurlFromTraffic(ExchangeId),
    /// Send a request through the proxy, as a new exchange.
    Replay(RequestSpec),
}

impl DeveloperOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::ParseCurl(_) => "parse_curl",
            Self::CurlFromTraffic(_) => "curl_from_traffic",
            Self::Replay(_) => "replay_request",
        }
    }
}

/// A request about where the proxy is listening.
///
/// Separate from the other three because it is about the proxy itself rather
/// than about traffic, rules, or a request: these three change, or ask about,
/// which networks can reach ProxyLab at all. They reach a different part of
/// the server — see [`crate::lan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LanOperation {
    /// Whether the proxy is on the local network, and which addresses it
    /// could use.
    Status,
    /// Start listening on this address of this Mac, as well as on loopback.
    Enable(String),
    /// Stop listening on the local network, leaving loopback alone.
    Disable,
}

impl LanOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Status => "lan_status",
            Self::Enable(_) => "enable_lan",
            Self::Disable => "disable_lan",
        }
    }
}

/// A question about the stored traffic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrafficOperation {
    /// A page of the traffic list.
    List(ListRequest),
    /// One exchange, whole.
    Get(ExchangeId),
    /// Remove one exchange.
    Delete(ExchangeId),
    /// Remove all of them.
    Clear,
    /// A page of stream messages on one exchange.
    ListStream(ListStreamRequest),
    /// One stream message, payload included.
    GetStream(StreamMessageId),
}

impl TrafficOperation {
    /// What this operation is called on the wire.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::List(_) => "list_traffic",
            Self::Get(_) => "get_traffic",
            Self::Delete(_) => "delete_traffic",
            Self::Clear => "clear_traffic",
            Self::ListStream(_) => "list_stream_messages",
            Self::GetStream(_) => "get_stream_message",
        }
    }
}

/// A checked request for a page of the traffic list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRequest {
    /// The page to fetch, with its filters.
    pub query: TrafficQuery,
}

/// A checked request for a page of stream messages on one exchange.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListStreamRequest {
    /// The exchange the messages belong to.
    pub exchange_id: ExchangeId,
    /// The last sequence number already held; the next page starts after it.
    pub after: Option<u64>,
    /// How many to return. The storage default when `None`.
    pub limit: Option<usize>,
}

/// Why a line could not be turned into a request.
///
/// Carries the request id when one was read, so the client can tell which of
/// its requests failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseFailure {
    /// The id of the request that failed, if it got that far.
    pub request_id: Option<RequestId>,
    /// What was wrong.
    pub error: ErrorBody,
}

impl ParseFailure {
    /// A failure that happened before the request id could be read.
    fn anonymous(code: ErrorCode, message: &'static str) -> Self {
        Self {
            request_id: None,
            error: ErrorBody::new(code, message),
        }
    }

    /// A failure in a request the server can name.
    fn of(id: &RequestId, code: ErrorCode, message: &'static str) -> Self {
        Self {
            request_id: Some(id.clone()),
            error: ErrorBody::new(code, message),
        }
    }
}

/// The fields every request has, with whatever else it carried left over.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default, rename = "type")]
    operation: Option<String>,
    /// Everything else, which is the operation's own parameters.
    #[serde(flatten)]
    parameters: Map<String, Value>,
}

/// The parameters of a list request, as they arrive.
///
/// Unknown fields are refused rather than ignored: a misspelled filter that
/// is quietly dropped is a client showing the wrong traffic and not knowing
/// it. The protocol is versioned, so a field this build does not know is a
/// client that disagrees about the version.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParameters {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<Cursor>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    status: Option<StatusFilter>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    search_regex: bool,
    #[serde(default)]
    search_case_sensitive: bool,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    http_version: Option<proxy_storage::HttpVersion>,
    #[serde(default)]
    websocket: bool,
    #[serde(default)]
    sse: bool,
    #[serde(default)]
    failed: bool,
    #[serde(default)]
    order: Option<TrafficOrder>,
    #[serde(default)]
    started_at_or_after: Option<Timestamp>,
    #[serde(default)]
    started_before: Option<Timestamp>,
}

/// The parameters of a request naming one exchange.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeParameters {
    exchange_id: String,
}

/// The parameters of a request for stream messages on one exchange.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListStreamParameters {
    exchange_id: String,
    #[serde(default)]
    after: Option<u64>,
    #[serde(default)]
    limit: Option<u32>,
}

/// The parameters of a request naming one stream message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamMessageIdParameters {
    stream_message_id: String,
}

/// The parameters of a request naming one rule.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleIdParameters {
    rule_id: String,
}

/// The parameters of a request that creates a rule.
///
/// The rule's own fields, flattened: a client sends the rule it wants rather
/// than a rule wrapped in an envelope. The id and the timestamps are not
/// among them — those are the server's to mint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRuleParameters {
    #[serde(flatten)]
    rule: NewRule,
}

/// The parameters of a request that changes a rule.
///
/// The rule id, and the fields to change. A field that is not named is not
/// changed, which is what makes "turn this rule off" one field rather than
/// the whole rule read back and sent again.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRuleParameters {
    rule_id: String,
    #[serde(flatten)]
    update: RuleUpdate,
}

/// The parameters of a request carrying a cURL command.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurlParameters {
    curl: String,
}

/// The parameters of a request to listen on the local network.
///
/// The address is text here and nothing more: whether it is an address, and
/// whether it is one of this Mac's own, is decided where the listener is
/// bound and not at the boundary — the set of usable addresses is a property
/// of the machine at that moment, which this module cannot see.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanParameters {
    address: String,
}

/// The parameters of a request to replay.
///
/// The request's own fields, flattened: a client sends the request it wants
/// sent rather than a request wrapped in an envelope, which is the shape
/// `create_rule` carries a rule in too.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayParameters {
    #[serde(flatten)]
    request: RequestParts,
}

/// The parameters of a request naming one breakpoint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BreakpointIdParameters {
    breakpoint_id: String,
}

/// The parameters of a request that configures a breakpoint.
///
/// The breakpoint's own fields, flattened, which is the shape `create_rule`
/// carries a rule in too. The id is not among them: it is the server's to
/// mint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBreakpointParameters {
    #[serde(flatten)]
    breakpoint: NewBreakpoint,
}

/// The parameters of a request that changes a breakpoint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBreakpointParameters {
    breakpoint_id: String,
    #[serde(flatten)]
    update: BreakpointUpdate,
}

/// The parameters of a request naming one paused exchange.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseIdParameters {
    pause_id: String,
}

/// The parameters of a request that lets a paused exchange carry on.
///
/// The edits are not flattened, unlike a rule or a breakpoint: a request edit
/// and a response edit both carry `headers` and `body`, so the two have to be
/// named apart.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParameters {
    pause_id: String,
    #[serde(default)]
    request: Option<RequestEdit>,
    #[serde(default)]
    response: Option<ResponseEdit>,
    #[serde(default)]
    websocket: Option<WebSocketEdit>,
}

/// The parameters of a request that shapes the traffic the proxy carries.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThrottleParameters {
    #[serde(flatten)]
    settings: ThrottleSettings,
}

/// The parameters of a request naming one saved request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRequestIdParameters {
    saved_request_id: String,
}

/// The parameters of a request that keeps a request a client described.
///
/// The request's own fields, flattened, which is the shape `create_rule`
/// carries a rule in too. The id and the timestamps are not among them:
/// those are the server's to mint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSavedParameters {
    #[serde(flatten)]
    request: NewSavedRequest,
}

/// The parameters of a request that keeps one captured exchange's request.
///
/// The exchange, and what to call it. Nothing else: what the request *was*
/// is the core's to read out of its own store, which is the same reason
/// `curl_from_traffic` carries an id and not a request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveFromTrafficParameters {
    exchange_id: String,
    name: String,
}

/// The parameters of a request that changes a saved request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSavedParameters {
    saved_request_id: String,
    #[serde(flatten)]
    update: SavedRequestUpdate,
}

/// The parameters of a request to write a HAR file.
///
/// `exchange_ids` and the filters are alternatives: one names what somebody
/// picked and the others name how their list is narrowed. Both together is
/// refused — see [`HarSelection`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportHarParameters {
    path: String,
    #[serde(default)]
    exchange_ids: Option<Vec<String>>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    status: Option<StatusFilter>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    search_regex: bool,
    #[serde(default)]
    search_case_sensitive: bool,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    http_version: Option<proxy_storage::HttpVersion>,
    #[serde(default)]
    websocket: bool,
    #[serde(default)]
    sse: bool,
    #[serde(default)]
    failed: bool,
    #[serde(default)]
    started_at_or_after: Option<Timestamp>,
    #[serde(default)]
    started_before: Option<Timestamp>,
}

impl ExportHarParameters {
    /// Whether any of the filters was set.
    fn has_filter(&self) -> bool {
        self.host.is_some()
            || self.method.is_some()
            || self.status.is_some()
            || self.search.is_some()
            || self.search_regex
            || self.search_case_sensitive
            || self.content_type.is_some()
            || self.http_version.is_some()
            || self.websocket
            || self.sse
            || self.failed
            || self.started_at_or_after.is_some()
            || self.started_before.is_some()
    }
}

/// The parameters of a request to read a HAR file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathParameters {
    path: String,
}

/// The parameters of a request that takes none.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParameters {}

/// Reads one line as a request, checking everything in it.
pub fn parse(line: &[u8]) -> Result<Request, ParseFailure> {
    let envelope: Envelope = serde_json::from_slice(line).map_err(|_| {
        ParseFailure::anonymous(
            ErrorCode::InvalidJson,
            "a request must be one JSON object on one line",
        )
    })?;

    let id = request_id(envelope.request_id)?;

    if envelope.version != Some(PROTOCOL_VERSION) {
        return Err(ParseFailure::of(
            &id,
            ErrorCode::UnsupportedVersion,
            "this server speaks protocol version 1",
        ));
    }

    let operation = operation(&id, envelope.operation.as_deref(), envelope.parameters)?;

    Ok(Request { id, operation })
}

/// Checks the request id.
fn request_id(id: Option<String>) -> Result<RequestId, ParseFailure> {
    let Some(id) = id else {
        return Err(ParseFailure::anonymous(
            ErrorCode::MissingRequestId,
            "a request must carry a request_id",
        ));
    };

    if id.is_empty() {
        return Err(ParseFailure::anonymous(
            ErrorCode::MissingRequestId,
            "a request_id must not be empty",
        ));
    }

    // Refused rather than echoed: an id goes back out in a response, so one
    // that is unreasonably long or carries control characters is one the
    // server will not repeat.
    if id.chars().count() > MAX_REQUEST_ID_LENGTH {
        return Err(ParseFailure::anonymous(
            ErrorCode::InvalidRequestId,
            "a request_id must be at most 128 characters",
        ));
    }
    if id.chars().any(char::is_control) {
        return Err(ParseFailure::anonymous(
            ErrorCode::InvalidRequestId,
            "a request_id must not contain control characters",
        ));
    }

    Ok(RequestId(id))
}

/// Works out which operation was asked for, and checks its parameters.
fn operation(
    id: &RequestId,
    name: Option<&str>,
    parameters: Map<String, Value>,
) -> Result<Operation, ParseFailure> {
    let Some(name) = name else {
        return Err(ParseFailure::of(
            id,
            ErrorCode::UnknownOperation,
            "a request must name an operation in its type field",
        ));
    };

    match name {
        "list_traffic" => Ok(Operation::Traffic(TrafficOperation::List(list(
            id, parameters,
        )?))),
        "get_traffic" => Ok(Operation::Traffic(TrafficOperation::Get(exchange_id(
            id, parameters,
        )?))),
        "delete_traffic" => Ok(Operation::Traffic(TrafficOperation::Delete(exchange_id(
            id, parameters,
        )?))),
        "clear_traffic" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Traffic(TrafficOperation::Clear))
        }
        "list_stream_messages" => Ok(Operation::Traffic(TrafficOperation::ListStream(
            list_stream(id, parameters)?,
        ))),
        "get_stream_message" => Ok(Operation::Traffic(TrafficOperation::GetStream(
            stream_message_id(id, parameters)?,
        ))),
        "list_rules" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Rule(RuleOperation::List))
        }
        "get_rule" => Ok(Operation::Rule(RuleOperation::Get(rule_id(
            id, parameters,
        )?))),
        "delete_rule" => Ok(Operation::Rule(RuleOperation::Delete(rule_id(
            id, parameters,
        )?))),
        "create_rule" => Ok(Operation::Rule(RuleOperation::Create(new_rule(
            id, parameters,
        )?))),
        "update_rule" => {
            let (rule, update) = rule_update(id, parameters)?;

            Ok(Operation::Rule(RuleOperation::Update(rule, update)))
        }
        "parse_curl" => Ok(Operation::Developer(DeveloperOperation::ParseCurl(curl(
            id, parameters,
        )?))),
        "curl_from_traffic" => Ok(Operation::Developer(DeveloperOperation::CurlFromTraffic(
            exchange_id(id, parameters)?,
        ))),
        "replay_request" => Ok(Operation::Developer(DeveloperOperation::Replay(
            replay_request(id, parameters)?,
        ))),
        "lan_status" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Lan(LanOperation::Status))
        }
        "enable_lan" => Ok(Operation::Lan(LanOperation::Enable(lan_address(
            id, parameters,
        )?))),
        "disable_lan" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Lan(LanOperation::Disable))
        }
        "list_breakpoints" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Breakpoint(BreakpointOperation::List))
        }
        "create_breakpoint" => Ok(Operation::Breakpoint(BreakpointOperation::Create(
            new_breakpoint(id, parameters)?,
        ))),
        "update_breakpoint" => {
            let (breakpoint, update) = breakpoint_update(id, parameters)?;

            Ok(Operation::Breakpoint(BreakpointOperation::Update(
                breakpoint, update,
            )))
        }
        "delete_breakpoint" => Ok(Operation::Breakpoint(BreakpointOperation::Delete(
            breakpoint_id(id, parameters)?,
        ))),
        "continue_breakpoint" => {
            let (pause, decision) = pause_decision(id, parameters)?;

            Ok(Operation::Breakpoint(BreakpointOperation::Continue(
                pause, decision,
            )))
        }
        "cancel_breakpoint" => Ok(Operation::Breakpoint(BreakpointOperation::Cancel(
            pause_id(id, parameters)?,
        ))),
        "watch_breakpoints" => {
            no_parameters(id, parameters)?;
            Ok(Operation::WatchBreakpoints)
        }
        "throttle_status" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Throttle(ThrottleOperation::Status))
        }
        "set_throttle" => Ok(Operation::Throttle(ThrottleOperation::Set(throttle(
            id, parameters,
        )?))),
        "list_saved_requests" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Saved(SavedRequestOperation::List))
        }
        "get_saved_request" => Ok(Operation::Saved(SavedRequestOperation::Get(
            saved_request_id(id, parameters)?,
        ))),
        "delete_saved_request" => Ok(Operation::Saved(SavedRequestOperation::Delete(
            saved_request_id(id, parameters)?,
        ))),
        "create_saved_request" => Ok(Operation::Saved(SavedRequestOperation::Create(
            new_saved_request(id, parameters)?,
        ))),
        "save_request_from_traffic" => {
            let (exchange, name) = save_from_traffic(id, parameters)?;

            Ok(Operation::Saved(SavedRequestOperation::CreateFromTraffic(
                exchange, name,
            )))
        }
        "update_saved_request" => {
            let (saved, update) = saved_request_update(id, parameters)?;

            Ok(Operation::Saved(SavedRequestOperation::Update(
                saved, update,
            )))
        }
        "export_har" => Ok(Operation::Har(HarOperation::Export(export_har(
            id, parameters,
        )?))),
        "import_har" => Ok(Operation::Har(HarOperation::Import(import_har(
            id, parameters,
        )?))),
        "subscribe" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Subscribe)
        }
        "diagnostics" => {
            no_parameters(id, parameters)?;
            Ok(Operation::Diagnostics)
        }
        _ => Err(ParseFailure::of(
            id,
            ErrorCode::UnknownOperation,
            "this server does not implement the operation named",
        )),
    }
}

/// Reads and checks the parameters of a list request.
fn list(id: &RequestId, parameters: Map<String, Value>) -> Result<ListRequest, ParseFailure> {
    let parameters: ListParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a list request accepts limit, cursor, host, method, status, search, \
                 search_regex, search_case_sensitive, content_type, http_version, \
                 websocket, sse, failed, order, started_at_or_after, and started_before",
            )
        })?;

    let limit = match parameters.limit {
        None => proxy_storage::DEFAULT_PAGE_LIMIT,
        Some(limit) => {
            let limit = limit as usize;
            if limit == 0 || limit > MAX_PAGE_LIMIT {
                return Err(ParseFailure::of(
                    id,
                    ErrorCode::InvalidParameters,
                    "a page limit must be between 1 and 1000",
                ));
            }

            limit
        }
    };

    let mut filter = TrafficFilter::all();

    if let Some(host) = parameters.host {
        filter = filter.with_host(checked_text(id, host, MAX_HOST_LENGTH)?);
    }
    if let Some(method) = parameters.method {
        filter = filter.with_method(checked_text(id, method, MAX_METHOD_LENGTH)?);
    }
    if let Some(status) = parameters.status {
        filter = filter.with_status(checked_status(id, status)?);
    }
    filter = apply_search_filters(
        id,
        filter,
        SearchExtras {
            search: parameters.search,
            regex: parameters.search_regex,
            case_sensitive: parameters.search_case_sensitive,
            content_type: parameters.content_type,
            http_version: parameters.http_version,
            websocket: parameters.websocket,
            sse: parameters.sse,
            failed: parameters.failed,
        },
    )?;

    let from = parameters.started_at_or_after;
    let until = parameters.started_before;
    for moment in [from, until].into_iter().flatten() {
        checked_timestamp(id, moment)?;
    }
    if let (Some(from), Some(until)) = (from, until) {
        if from > until {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "the start of a time range must not be after its end",
            ));
        }
    }
    filter = filter.with_time_range(from, until);

    // The order is set before the cursor: `with_order` drops a cursor,
    // because a cursor names a place in the order it was produced in.
    let mut query = TrafficQuery::newest()
        .with_limit(limit)
        .with_filter(filter)
        .with_order(parameters.order.unwrap_or_default());
    if let Some(cursor) = parameters.cursor {
        query = query.after(cursor);
    }

    Ok(ListRequest { query })
}

/// Applies the search box and the extra list filters that sit beside it.
fn apply_search_filters(
    id: &RequestId,
    mut filter: TrafficFilter,
    extras: SearchExtras,
) -> Result<TrafficFilter, ParseFailure> {
    if let Some(search) = extras.search {
        let text = checked_text(id, search, MAX_SEARCH_LENGTH)?;
        if extras.regex && !proxy_storage::regex_is_valid(&text) {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "the search is not a regular expression",
            ));
        }
        filter = filter.with_text(text);
    } else if extras.regex {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a regex search needs a search",
        ));
    }
    if extras.regex {
        filter = filter.with_regex();
    }
    if extras.case_sensitive {
        filter = filter.case_sensitive();
    }
    if let Some(content_type) = extras.content_type {
        filter = filter.with_content_type(checked_text(id, content_type, MAX_HOST_LENGTH)?);
    }
    if let Some(version) = extras.http_version {
        filter = filter.with_http_version(version);
    }
    if extras.websocket {
        filter = filter.websocket_only();
    }
    if extras.sse {
        filter = filter.sse_only();
    }
    if extras.failed {
        filter = filter.failed_only();
    }

    Ok(filter)
}

/// The search box and the filters that were added beside it.
struct SearchExtras {
    search: Option<String>,
    regex: bool,
    case_sensitive: bool,
    content_type: Option<String>,
    http_version: Option<proxy_storage::HttpVersion>,
    websocket: bool,
    sse: bool,
    failed: bool,
}

/// Checks a filter value that is free text.
fn checked_text(id: &RequestId, value: String, limit: usize) -> Result<String, ParseFailure> {
    if value.is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a filter value must not be empty",
        ));
    }
    if value.chars().count() > limit {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a filter value is longer than the value it could match",
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a filter value must not contain control characters",
        ));
    }
    Ok(value)
}

/// Checks a status filter.
fn checked_status(id: &RequestId, filter: StatusFilter) -> Result<StatusFilter, ParseFailure> {
    let valid = match filter {
        StatusFilter::Exact(status) => (100..=599).contains(&status),
        StatusFilter::Class(class) => (1..=5).contains(&class),
    };

    if !valid {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a status filter must be a code from 100 to 599, or a class from 1 to 5",
        ));
    }

    Ok(filter)
}

/// Checks a timestamp.
fn checked_timestamp(id: &RequestId, moment: Timestamp) -> Result<(), ParseFailure> {
    if moment.nanos >= 1_000_000_000 {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a timestamp's nanos must be less than one second",
        ));
    }

    Ok(())
}

/// Reads and checks the exchange id a request names.
fn exchange_id(id: &RequestId, parameters: Map<String, Value>) -> Result<ExchangeId, ParseFailure> {
    let parameters: ExchangeParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, exchange_id",
            )
        })?;

    parameters.exchange_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidExchangeId,
            "an exchange id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks a page of stream messages on one exchange.
fn list_stream(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<ListStreamRequest, ParseFailure> {
    let parameters: ListStreamParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a list_stream_messages request takes exchange_id, and optionally after and limit",
            )
        })?;

    let exchange_id = parameters.exchange_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidExchangeId,
            "an exchange id is a UUID, as the server wrote it",
        )
    })?;

    let limit = match parameters.limit {
        None => None,
        Some(limit) => {
            let limit = limit as usize;
            if limit == 0 || limit > MAX_PAGE_LIMIT {
                return Err(ParseFailure::of(
                    id,
                    ErrorCode::InvalidParameters,
                    "a page limit must be between 1 and 1000",
                ));
            }
            Some(limit)
        }
    };

    Ok(ListStreamRequest {
        exchange_id,
        after: parameters.after,
        limit,
    })
}

/// Reads and checks the stream message id a request names.
fn stream_message_id(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<StreamMessageId, ParseFailure> {
    let parameters: StreamMessageIdParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "this operation takes one parameter, stream_message_id",
        )
    })?;

    parameters.stream_message_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidStreamMessageId,
            "a stream message id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks the rule id a request names.
fn rule_id(id: &RequestId, parameters: Map<String, Value>) -> Result<RuleId, ParseFailure> {
    let parameters: RuleIdParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, rule_id",
            )
        })?;

    parameters.rule_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidRuleId,
            "a rule id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks the rule a create request carries.
///
/// Checked here, before anything is stored: a rule that could not be acted
/// on is refused at the moment it is written rather than at the moment it
/// would have mattered.
fn new_rule(id: &RequestId, parameters: Map<String, Value>) -> Result<NewRule, ParseFailure> {
    let parameters: CreateRuleParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a create_rule request takes name, action, and optionally matcher, \
                 priority, and enabled",
            )
        })?;

    checked_rule_name(id, &parameters.rule.name)?;
    if !parameters.rule.matcher.is_valid() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidRule,
            "a rule's matching criteria are not ones that could match",
        ));
    }
    if let Err(error) = parameters.rule.action.validate() {
        return Err(ParseFailure::of(id, ErrorCode::InvalidRule, error.reason()));
    }

    Ok(parameters.rule)
}

/// Reads and checks the rule and the change an update request carries.
fn rule_update(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<(RuleId, RuleUpdate), ParseFailure> {
    let parameters: UpdateRuleParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an update_rule request takes rule_id and any of name, matcher, action, \
                 priority, and enabled",
            )
        })?;

    let rule = parameters.rule_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidRuleId,
            "a rule id is a UUID, as the server wrote it",
        )
    })?;

    let update = parameters.update;
    if update.is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an update_rule request must change something",
        ));
    }
    if let Some(name) = &update.name {
        checked_rule_name(id, name)?;
    }
    if let Some(matcher) = &update.matcher {
        if !matcher.is_valid() {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidRule,
                "a rule's matching criteria are not ones that could match",
            ));
        }
    }
    if let Some(action) = &update.action {
        if let Err(error) = action.validate() {
            return Err(ParseFailure::of(id, ErrorCode::InvalidRule, error.reason()));
        }
    }

    Ok((rule, update))
}

/// Checks a rule's name.
///
/// The same bounds the rule model applies, checked here so the refusal
/// names the parameter rather than coming back as a storage failure.
fn checked_rule_name(id: &RequestId, name: &str) -> Result<(), ParseFailure> {
    if name.trim().is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidRule,
            "a rule must have a name",
        ));
    }
    if name.chars().count() > rules::MAX_RULE_NAME_LENGTH {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidRule,
            "a rule name must be at most 200 characters",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidRule,
            "a rule name must not contain control characters",
        ));
    }

    Ok(())
}

/// Reads the cURL command a request carries.
///
/// Whether the command *parses* is not decided here: a command that does not
/// is the answer to a successful `parse_curl`, not a failure of the request
/// that carried it. See [`CurlParsed`](super::response::CurlParsed).
fn curl(id: &RequestId, parameters: Map<String, Value>) -> Result<String, ParseFailure> {
    let parameters: CurlParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, curl",
            )
        })?;

    Ok(parameters.curl)
}

/// Reads and checks the saved request id a request names.
fn saved_request_id(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<SavedRequestId, ParseFailure> {
    let parameters: SavedRequestIdParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, saved_request_id",
            )
        })?;

    parameters.saved_request_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidSavedRequestId,
            "a saved request id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks the request a save carries.
///
/// The name is this boundary's to check; whether the URL is a URL and the
/// method a method is `proxy-http`'s, and is checked where the saved request
/// is turned into one — see [`crate::service`].
fn new_saved_request(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<NewSavedRequest, ParseFailure> {
    let parameters: CreateSavedParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a create_saved_request request takes name, method, url, and optionally \
                 headers and body",
            )
        })?;

    checked_saved_name(id, &parameters.request.name)?;

    Ok(parameters.request)
}

/// Reads and checks the exchange and the name a save-from-traffic carries.
fn save_from_traffic(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<(ExchangeId, String), ParseFailure> {
    let parameters: SaveFromTrafficParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a save_request_from_traffic request takes exchange_id and name",
        )
    })?;

    let exchange = parameters.exchange_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidExchangeId,
            "an exchange id is a UUID, as the server wrote it",
        )
    })?;
    checked_saved_name(id, &parameters.name)?;

    Ok((exchange, parameters.name))
}

/// Reads and checks the saved request and the change an update carries.
fn saved_request_update(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<(SavedRequestId, SavedRequestUpdate), ParseFailure> {
    let parameters: UpdateSavedParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an update_saved_request request takes saved_request_id and any of name, \
                 method, url, headers, and body",
            )
        })?;

    let saved = parameters.saved_request_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidSavedRequestId,
            "a saved request id is a UUID, as the server wrote it",
        )
    })?;

    let update = parameters.update;
    if update.is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an update_saved_request request must change something",
        ));
    }
    if let Some(name) = &update.name {
        checked_saved_name(id, name)?;
    }

    Ok((saved, update))
}

/// Checks the name a saved request is being given.
///
/// The model's own checks made early, so a name that will be refused is
/// refused before anything is written — and refused with the model's own
/// phrase, which names nothing the client sent.
fn checked_saved_name(id: &RequestId, name: &str) -> Result<(), ParseFailure> {
    if name.trim().is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidSavedRequest,
            "a saved request must have a name",
        ));
    }
    if name.chars().count() > MAX_SAVED_REQUEST_NAME_LENGTH {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidSavedRequest,
            "a saved request name must be at most 200 characters",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidSavedRequest,
            "a saved request name must not contain control characters",
        ));
    }

    Ok(())
}

/// Reads and checks what an export is for and where it goes.
fn export_har(id: &RequestId, parameters: Map<String, Value>) -> Result<HarExport, ParseFailure> {
    let parameters: ExportHarParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an export_har request takes path and either exchange_ids or the filters \
                 list_traffic takes",
            )
        })?;

    let path = checked_path(id, parameters.path.clone())?;

    if let Some(ids) = &parameters.exchange_ids {
        if parameters.has_filter() {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an export names exchanges or filters, and not both",
            ));
        }
        if ids.is_empty() {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an export must name at least one exchange",
            ));
        }
        if ids.len() > MAX_EXPORT_SELECTION {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an export may name at most 500 exchanges one by one",
            ));
        }

        let exchanges = ids
            .iter()
            .map(|text| {
                text.parse().map_err(|_| {
                    ParseFailure::of(
                        id,
                        ErrorCode::InvalidExchangeId,
                        "an exchange id is a UUID, as the server wrote it",
                    )
                })
            })
            .collect::<Result<Vec<ExchangeId>, ParseFailure>>()?;

        return Ok(HarExport {
            path,
            selection: HarSelection::Exchanges(exchanges),
        });
    }

    // No ids: everything the filters match, which with no filters set is
    // everything stored. That is what "Export All" is, and it is the same
    // filter value `list_traffic` is given.
    let mut filter = TrafficFilter::all();

    if let Some(host) = parameters.host {
        filter = filter.with_host(checked_text(id, host, MAX_HOST_LENGTH)?);
    }
    if let Some(method) = parameters.method {
        filter = filter.with_method(checked_text(id, method, MAX_METHOD_LENGTH)?);
    }
    if let Some(status) = parameters.status {
        filter = filter.with_status(checked_status(id, status)?);
    }
    filter = apply_search_filters(
        id,
        filter,
        SearchExtras {
            search: parameters.search,
            regex: parameters.search_regex,
            case_sensitive: parameters.search_case_sensitive,
            content_type: parameters.content_type,
            http_version: parameters.http_version,
            websocket: parameters.websocket,
            sse: parameters.sse,
            failed: parameters.failed,
        },
    )?;

    let from = parameters.started_at_or_after;
    let until = parameters.started_before;
    for moment in [from, until].into_iter().flatten() {
        checked_timestamp(id, moment)?;
    }
    if let (Some(from), Some(until)) = (from, until) {
        if from > until {
            return Err(ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "the start of a time range must not be after its end",
            ));
        }
    }
    filter = filter.with_time_range(from, until);

    Ok(HarExport {
        path,
        selection: HarSelection::Filtered(filter),
    })
}

/// Reads and checks the path an import names.
fn import_har(id: &RequestId, parameters: Map<String, Value>) -> Result<String, ParseFailure> {
    let parameters: PathParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, path",
            )
        })?;

    checked_path(id, parameters.path)
}

/// Checks the shape of a path a client named.
///
/// Length, and that it is text a path can be made of. Whether it is absolute,
/// whether there is a directory at it, and whether anything can be written
/// there or read from it are decided in `proxy-har`, which is what opens it —
/// the same division a Map Local rule has.
fn checked_path(id: &RequestId, path: String) -> Result<String, ParseFailure> {
    if path.trim().is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a path must not be empty",
        ));
    }
    if path.len() > MAX_PATH_LENGTH {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a path must be at most 4096 bytes",
        ));
    }
    // A NUL ends a path as far as the operating system is concerned, so a
    // path carrying one is a path that means something other than what it
    // reads as. A control character is not in any file name worth writing.
    if path.chars().any(char::is_control) {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "a path must not contain control characters",
        ));
    }

    Ok(path)
}

/// Reads the address an `enable_lan` names.
///
/// Length is all that is checked here, and generously: the address is looked
/// at properly where it is used, against the addresses this Mac actually has.
fn lan_address(id: &RequestId, parameters: Map<String, Value>) -> Result<String, ParseFailure> {
    let parameters: LanParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, address",
            )
        })?;

    if parameters.address.is_empty() || parameters.address.len() > MAX_LAN_ADDRESS_LENGTH {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an address must be an IP address of this machine",
        ));
    }

    Ok(parameters.address)
}

/// Reads and checks the request a replay carries.
///
/// Checked here, before anything is sent: a URL that names no host or a
/// header value that carries a line break is refused at the boundary rather
/// than by whatever would have tried to send it.
fn replay_request(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<RequestSpec, ParseFailure> {
    let parameters: ReplayParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a replay_request takes method and url, and optionally headers and body",
            )
        })?;

    // The reason is one of the request model's own fixed phrases, which name
    // nothing the client sent.
    RequestSpec::from_parts(parameters.request)
        .map_err(|error| ParseFailure::of(id, ErrorCode::InvalidRequest, error.reason()))
}

/// Reads and checks the breakpoint id a request names.
fn breakpoint_id(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<BreakpointId, ParseFailure> {
    let parameters: BreakpointIdParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, breakpoint_id",
            )
        })?;

    parameters.breakpoint_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidBreakpointId,
            "a breakpoint id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks the pause id a request names.
fn pause_id(id: &RequestId, parameters: Map<String, Value>) -> Result<PauseId, ParseFailure> {
    let parameters: PauseIdParameters =
        serde_json::from_value(Value::Object(parameters)).map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes one parameter, pause_id",
            )
        })?;

    parameters.pause_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidPauseId,
            "a pause id is a UUID, as the server wrote it",
        )
    })
}

/// Reads and checks the breakpoint a create request carries.
fn new_breakpoint(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<NewBreakpoint, ParseFailure> {
    let parameters: CreateBreakpointParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a create_breakpoint request takes direction, and optionally matcher and enabled",
            )
        })?;

    if !parameters.breakpoint.matcher.is_valid() {
        return Err(refused(id, BreakpointRefusal::Matcher));
    }

    Ok(parameters.breakpoint)
}

/// Reads and checks the breakpoint and the change an update request carries.
fn breakpoint_update(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<(BreakpointId, BreakpointUpdate), ParseFailure> {
    let parameters: UpdateBreakpointParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "an update_breakpoint request takes breakpoint_id and any of enabled, \
             direction, and matcher",
            )
        })?;

    let breakpoint = parameters.breakpoint_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidBreakpointId,
            "a breakpoint id is a UUID, as the server wrote it",
        )
    })?;

    let update = parameters.update;
    if update.is_empty() {
        return Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an update_breakpoint request must change something",
        ));
    }
    if let Some(matcher) = &update.matcher {
        if !matcher.is_valid() {
            return Err(refused(id, BreakpointRefusal::Matcher));
        }
    }

    Ok((breakpoint, update))
}

/// Reads and checks the decision a `continue_breakpoint` carries.
///
/// The edits are checked here, before anything is held any longer: a status
/// that is not a status, a header a message could not carry, and a body
/// longer than the boundary will pass are all refused at the boundary rather
/// than by whatever would have tried to send them.
fn pause_decision(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<(PauseId, PauseDecision), ParseFailure> {
    let parameters: ContinueParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a continue_breakpoint request takes pause_id, and optionally request, response, or websocket",
            )
        })?;

    let pause = parameters.pause_id.parse().map_err(|_| {
        ParseFailure::of(
            id,
            ErrorCode::InvalidPauseId,
            "a pause id is a UUID, as the server wrote it",
        )
    })?;

    // One pause holds one half, so an edit for each of them is a client that
    // does not know which half it is answering.
    if parameters.request.is_some() as u8
        + parameters.response.is_some() as u8
        + parameters.websocket.is_some() as u8
        > 1
    {
        return Err(refused(id, BreakpointRefusal::WrongDirection));
    }

    if let Some(edit) = &parameters.request {
        if let Some(method) = &edit.method {
            checked_method(id, method)?;
        }
        if let Some(url) = &edit.url {
            checked_url(id, url)?;
        }
        if let Some(headers) = &edit.headers {
            checked_headers(id, headers)?;
        }
        if let Some(body) = &edit.body {
            checked_body(id, body)?;
        }
    }

    if let Some(edit) = &parameters.response {
        if let Some(status) = edit.status {
            if !(100..=599).contains(&status) {
                return Err(refused(id, BreakpointRefusal::Status));
            }
        }
        if let Some(headers) = &edit.headers {
            checked_headers(id, headers)?;
        }
        if let Some(body) = &edit.body {
            checked_body(id, body)?;
        }
    }

    if let Some(edit) = &parameters.websocket {
        if let Some(payload) = &edit.payload {
            checked_body(id, payload)?;
        }
    }

    Ok((
        pause,
        PauseDecision::Continue {
            request: parameters.request,
            response: parameters.response,
            websocket: parameters.websocket,
        },
    ))
}

/// Checks an edited method.
///
/// The same bound a request being replayed is held to, and for the same
/// reason: a method is one HTTP token and never somewhere to put something
/// else.
fn checked_method(id: &RequestId, method: &str) -> Result<(), ParseFailure> {
    let valid = !method.is_empty()
        && method.len() <= proxy_http::MAX_METHOD_LENGTH
        && method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));

    match valid {
        true => Ok(()),
        false => Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an edited method must be one HTTP token",
        )),
    }
}

/// Checks an edited URL.
///
/// Length and shape only. Whether it names a host this proxy can reach is
/// settled where the edited request is rewritten, against the request it is
/// replacing: a request inside an intercepted session cannot be sent to
/// another host, and that is not a fact this module can see.
fn checked_url(id: &RequestId, url: &str) -> Result<(), ParseFailure> {
    let valid = !url.is_empty()
        && url.len() <= proxy_http::MAX_URL_LENGTH
        && !url.chars().any(char::is_control)
        && !url.contains(char::is_whitespace);

    match valid {
        true => Ok(()),
        false => Err(ParseFailure::of(
            id,
            ErrorCode::InvalidParameters,
            "an edited URL must be a URL, and at most 8192 characters",
        )),
    }
}

/// Checks an edited header section.
///
/// A name that is not a token or a value carrying a line break is how a
/// second message is smuggled inside the first, so neither gets past here.
fn checked_headers(id: &RequestId, headers: &proxy_storage::Headers) -> Result<(), ParseFailure> {
    if headers.len() > proxy_http::MAX_HEADERS {
        return Err(refused(id, BreakpointRefusal::Header));
    }

    for header in headers {
        let name_ok = !header.name.is_empty()
            && header.name.len() <= proxy_http::MAX_HEADER_NAME_LENGTH
            && header
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
        let value_ok = !header
            .value
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\r' | b'\n' | 0));

        if !name_ok || !value_ok {
            return Err(refused(id, BreakpointRefusal::Header));
        }
    }

    Ok(())
}

/// Checks an edited body.
fn checked_body(id: &RequestId, body: &[u8]) -> Result<(), ParseFailure> {
    match body.len() <= MAX_BREAKPOINT_BODY_BYTES {
        true => Ok(()),
        false => Err(refused(id, BreakpointRefusal::BodyTooLarge)),
    }
}

/// One of the breakpoint boundary's own refusals, as a parse failure.
fn refused(id: &RequestId, refusal: BreakpointRefusal) -> ParseFailure {
    let error = refusal.error();

    ParseFailure {
        request_id: Some(id.clone()),
        error,
    }
}

/// Reads and checks the settings a `set_throttle` carries.
fn throttle(
    id: &RequestId,
    parameters: Map<String, Value>,
) -> Result<ThrottleSettings, ParseFailure> {
    let parameters: ThrottleParameters = serde_json::from_value(Value::Object(parameters))
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "a set_throttle request takes enabled, and optionally latency_ms, \
                 download_bytes_per_second, and upload_bytes_per_second",
            )
        })?;

    parameters.settings.validate().map_err(|refusal| {
        let error = refusal.error();

        ParseFailure {
            request_id: Some(id.clone()),
            error,
        }
    })?;

    Ok(parameters.settings)
}

/// Checks that an operation which takes no parameters was sent none.
fn no_parameters(id: &RequestId, parameters: Map<String, Value>) -> Result<(), ParseFailure> {
    serde_json::from_value::<NoParameters>(Value::Object(parameters))
        .map(|_| ())
        .map_err(|_| {
            ParseFailure::of(
                id,
                ErrorCode::InvalidParameters,
                "this operation takes no parameters",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::DEFAULT_PAGE_LIMIT;

    /// The request a valid one is built from, so each test changes one thing.
    fn parsed(line: &str) -> Result<Request, ParseFailure> {
        parse(line.as_bytes())
    }

    /// The code a line is refused with.
    fn refused(line: &str) -> ErrorCode {
        parsed(line)
            .expect_err("the request should have been refused")
            .error
            .code
    }

    /// The failure a line is refused with.
    fn failure(line: &str) -> ParseFailure {
        parsed(line).expect_err("the request should have been refused")
    }

    fn list_query(line: &str) -> TrafficQuery {
        match parsed(line).expect("a valid request").operation {
            Operation::Traffic(TrafficOperation::List(request)) => request.query,
            other => panic!("expected a list request, got {other:?}"),
        }
    }

    #[test]
    fn a_list_request_is_read_with_its_defaults() {
        let request = parsed(r#"{"version":1,"request_id":"r1","type":"list_traffic"}"#)
            .expect("a valid request");

        assert_eq!(request.id.as_str(), "r1");
        assert_eq!(
            request.operation,
            Operation::Traffic(TrafficOperation::List(ListRequest {
                query: TrafficQuery::newest().with_limit(DEFAULT_PAGE_LIMIT),
            }))
        );
    }

    #[test]
    fn a_stream_list_request_names_the_parent_exchange() {
        let request = parsed(
            r#"{"version":1,"request_id":"r1","type":"list_stream_messages",
                "exchange_id":"0199c0de-0000-7000-8000-00000000002a","after":3,"limit":25}"#,
        )
        .expect("a valid request");

        match request.operation {
            Operation::Traffic(TrafficOperation::ListStream(list)) => {
                assert_eq!(
                    list.exchange_id.to_string(),
                    "0199c0de-0000-7000-8000-00000000002a"
                );
                assert_eq!(list.after, Some(3));
                assert_eq!(list.limit, Some(25));
            }
            other => panic!("expected a stream list, got {other:?}"),
        }
    }

    #[test]
    fn a_stream_get_request_names_the_message() {
        let request = parsed(
            r#"{"version":1,"request_id":"r1","type":"get_stream_message",
                "stream_message_id":"0199c0de-0000-7000-8000-00000000002b"}"#,
        )
        .expect("a valid request");

        match request.operation {
            Operation::Traffic(TrafficOperation::GetStream(id)) => {
                assert_eq!(id.to_string(), "0199c0de-0000-7000-8000-00000000002b");
            }
            other => panic!("expected a stream get, got {other:?}"),
        }
    }

    #[test]
    fn a_list_request_carries_its_page_size_and_filters() {
        let query = list_query(
            r#"{"version":1,"request_id":"r1","type":"list_traffic","limit":25,
                "host":"api.example.com","method":"GET","status":{"class":4},
                "started_at_or_after":{"seconds":1700000000,"nanos":0},
                "started_before":{"seconds":1700003600,"nanos":0}}"#,
        );

        assert_eq!(query.limit, 25);
        assert_eq!(query.filter.host.as_deref(), Some("api.example.com"));
        assert_eq!(query.filter.method.as_deref(), Some("GET"));
        assert_eq!(query.filter.status, Some(StatusFilter::Class(4)));
        assert_eq!(
            query.filter.started_at_or_after,
            Some(Timestamp {
                seconds: 1_700_000_000,
                nanos: 0
            })
        );
        assert!(query.after.is_none());
    }

    #[test]
    fn a_list_request_can_ask_for_the_page_after_a_cursor() {
        let cursor = Cursor::of(&sample_summary());
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"list_traffic","cursor":{}}}"#,
            serde_json::to_string(&cursor).expect("serializes")
        );

        assert_eq!(list_query(&line).after, Some(cursor));
    }

    /// A summary, only as something to make a cursor out of.
    fn sample_summary() -> proxy_storage::ExchangeSummary {
        let client = "127.0.0.1:54321".parse().expect("a valid address");
        let exchange = proxy_storage::TrafficExchange::new(
            ExchangeId::next(),
            proxy_storage::ConnectionInfo::http(client, None),
            proxy_storage::CapturedRequest::new(
                "GET",
                "http://example.com/",
                proxy_storage::HttpVersion::Http11,
            ),
        );

        proxy_storage::ExchangeSummary::of(&exchange)
    }

    #[test]
    fn a_get_request_names_one_exchange() {
        let id = ExchangeId::next();
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"get_traffic","exchange_id":"{id}"}}"#
        );

        assert_eq!(
            parsed(&line).expect("a valid request").operation,
            Operation::Traffic(TrafficOperation::Get(id))
        );
    }

    #[test]
    fn a_delete_request_names_one_exchange() {
        let id = ExchangeId::next();
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"delete_traffic","exchange_id":"{id}"}}"#
        );

        assert_eq!(
            parsed(&line).expect("a valid request").operation,
            Operation::Traffic(TrafficOperation::Delete(id))
        );
    }

    #[test]
    fn a_clear_request_takes_nothing() {
        assert_eq!(
            parsed(r#"{"version":1,"request_id":"r1","type":"clear_traffic"}"#)
                .expect("a valid request")
                .operation,
            Operation::Traffic(TrafficOperation::Clear)
        );
    }

    #[test]
    fn a_subscribe_request_is_not_a_traffic_operation() {
        assert_eq!(
            parsed(r#"{"version":1,"request_id":"r1","type":"subscribe"}"#)
                .expect("a valid request")
                .operation,
            Operation::Subscribe
        );
    }

    #[test]
    fn a_line_that_is_not_json_is_refused() {
        for line in ["not json", "{", "[]", r#"{"version":1"#, "42", "null"] {
            assert_eq!(refused(line), ErrorCode::InvalidJson, "{line}");
        }
    }

    #[test]
    fn a_request_that_is_not_json_cannot_be_correlated() {
        assert_eq!(failure("not json").request_id, None);
    }

    #[test]
    fn a_request_with_no_request_id_is_refused() {
        assert_eq!(
            refused(r#"{"version":1,"type":"clear_traffic"}"#),
            ErrorCode::MissingRequestId
        );
        assert_eq!(
            refused(r#"{"version":1,"request_id":"","type":"clear_traffic"}"#),
            ErrorCode::MissingRequestId
        );
        assert_eq!(
            refused(r#"{"version":1,"request_id":null,"type":"clear_traffic"}"#),
            ErrorCode::MissingRequestId
        );
    }

    #[test]
    fn a_request_id_that_would_not_be_safe_to_echo_is_refused() {
        let long = "x".repeat(MAX_REQUEST_ID_LENGTH + 1);
        let line = format!(r#"{{"version":1,"request_id":"{long}","type":"clear_traffic"}}"#);
        assert_eq!(refused(&line), ErrorCode::InvalidRequestId);

        let control = r#"{"version":1,"request_id":"a\nb","type":"clear_traffic"}"#;
        assert_eq!(refused(control), ErrorCode::InvalidRequestId);

        // Refused, and not repeated back.
        assert_eq!(failure(control).request_id, None);
    }

    #[test]
    fn a_request_id_of_the_greatest_length_allowed_is_accepted() {
        let id = "x".repeat(MAX_REQUEST_ID_LENGTH);
        let line = format!(r#"{{"version":1,"request_id":"{id}","type":"clear_traffic"}}"#);

        assert_eq!(parsed(&line).expect("a valid request").id.as_str(), id);
    }

    #[test]
    fn a_request_naming_another_protocol_version_is_refused() {
        for line in [
            r#"{"version":2,"request_id":"r1","type":"clear_traffic"}"#,
            r#"{"version":0,"request_id":"r1","type":"clear_traffic"}"#,
            r#"{"request_id":"r1","type":"clear_traffic"}"#,
        ] {
            assert_eq!(refused(line), ErrorCode::UnsupportedVersion, "{line}");
        }
    }

    #[test]
    fn a_refusal_after_the_request_id_was_read_names_the_request() {
        let failure = failure(r#"{"version":2,"request_id":"r1","type":"clear_traffic"}"#);

        assert_eq!(
            failure.request_id.as_ref().map(RequestId::as_str),
            Some("r1")
        );
    }

    #[test]
    fn an_operation_this_server_does_not_have_is_refused() {
        for line in [
            r#"{"version":1,"request_id":"r1","type":"drop_database"}"#,
            r#"{"version":1,"request_id":"r1","type":""}"#,
            r#"{"version":1,"request_id":"r1"}"#,
        ] {
            assert_eq!(refused(line), ErrorCode::UnknownOperation, "{line}");
        }
    }

    #[test]
    fn an_unknown_operation_is_not_repeated_back() {
        let failure =
            failure(r#"{"version":1,"request_id":"r1","type":"<script>alert(1)</script>"}"#);

        assert!(
            !failure.error.message.contains("script"),
            "{:?}",
            failure.error
        );
    }

    #[test]
    fn an_exchange_id_that_is_not_an_id_is_refused() {
        for id in ["", "7", "not-a-uuid", "0199c0de-0000-7000-8000-00000000002"] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"get_traffic","exchange_id":"{id}"}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidExchangeId, "{id}");
        }
    }

    #[test]
    fn an_operation_that_needs_an_exchange_id_and_has_none_is_refused() {
        assert_eq!(
            refused(r#"{"version":1,"request_id":"r1","type":"get_traffic"}"#),
            ErrorCode::InvalidParameters
        );
        assert_eq!(
            refused(r#"{"version":1,"request_id":"r1","type":"get_traffic","exchange_id":7}"#),
            ErrorCode::InvalidParameters
        );
    }

    #[test]
    fn a_page_size_outside_what_a_page_can_be_is_refused() {
        for limit in ["0", "1001", "4294967295"] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","limit":{limit}}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{limit}");
        }
    }

    #[test]
    fn a_page_size_that_is_not_a_page_size_is_refused() {
        for limit in ["-1", "\"many\"", "1.5"] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","limit":{limit}}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{limit}");
        }
    }

    #[test]
    fn the_largest_page_allowed_is_accepted() {
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"list_traffic","limit":{MAX_PAGE_LIMIT}}}"#
        );

        assert_eq!(list_query(&line).limit, MAX_PAGE_LIMIT);
    }

    #[test]
    fn a_filter_value_that_is_empty_or_unreasonable_is_refused() {
        let long = "x".repeat(MAX_HOST_LENGTH + 1);

        for value in [String::new(), long, "bad\nhost".to_owned()] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","host":{}}}"#,
                serde_json::to_string(&value).expect("serializes")
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{value:?}");
        }
    }

    #[test]
    fn a_method_filter_that_is_unreasonable_is_refused() {
        let long = "X".repeat(MAX_METHOD_LENGTH + 1);
        let line =
            format!(r#"{{"version":1,"request_id":"r1","type":"list_traffic","method":"{long}"}}"#);

        assert_eq!(refused(&line), ErrorCode::InvalidParameters);
    }

    #[test]
    fn a_list_request_can_carry_a_search() {
        let query = list_query(
            r#"{"version":1,"request_id":"r1","type":"list_traffic","search":"api/users"}"#,
        );

        assert_eq!(query.filter.text.as_deref(), Some("api/users"));
        assert!(!query.filter.search_regex);
    }

    #[test]
    fn a_list_request_can_ask_for_a_regex_search() {
        let query = list_query(
            r#"{"version":1,"request_id":"r1","type":"list_traffic","search":"user-\\d+","search_regex":true}"#,
        );

        assert_eq!(query.filter.text.as_deref(), Some(r"user-\d+"));
        assert!(query.filter.search_regex);
    }

    #[test]
    fn a_regex_that_is_not_one_is_refused() {
        assert_eq!(
            refused(
                r#"{"version":1,"request_id":"r1","type":"list_traffic","search":"(","search_regex":true}"#
            ),
            ErrorCode::InvalidParameters
        );
    }

    #[test]
    fn extra_list_filters_are_read() {
        let query = list_query(
            r#"{"version":1,"request_id":"r1","type":"list_traffic",
                "content_type":"json","http_version":"HTTP/2.0",
                "websocket":true,"failed":true,"search_case_sensitive":true}"#,
        );

        assert_eq!(query.filter.content_type.as_deref(), Some("json"));
        assert_eq!(
            query.filter.http_version,
            Some(proxy_storage::HttpVersion::Http2)
        );
        assert!(query.filter.websocket);
        assert!(query.filter.failed);
        assert!(query.filter.search_case_sensitive);
        assert!(!query.filter.sse);
    }

    #[test]
    fn a_list_request_with_no_search_searches_for_nothing() {
        let query = list_query(r#"{"version":1,"request_id":"r1","type":"list_traffic"}"#);

        assert_eq!(query.filter.text, None);
    }

    #[test]
    fn a_search_that_is_empty_or_unreasonable_is_refused() {
        let long = "x".repeat(MAX_SEARCH_LENGTH + 1);

        for search in [String::new(), long, "bad\nsearch".to_owned()] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","search":{}}}"#,
                serde_json::to_string(&search).expect("serializes")
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{search:?}");
        }
    }

    #[test]
    fn a_search_of_the_greatest_length_allowed_is_accepted() {
        let search = "x".repeat(MAX_SEARCH_LENGTH);
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"list_traffic","search":"{search}"}}"#
        );

        assert_eq!(list_query(&line).filter.text, Some(search));
    }

    #[test]
    fn a_search_is_not_repeated_back_when_it_is_refused() {
        let long = "needle".repeat(MAX_SEARCH_LENGTH);
        let line =
            format!(r#"{{"version":1,"request_id":"r1","type":"list_traffic","search":"{long}"}}"#);

        assert!(
            !failure(&line).error.message.contains("needle"),
            "{:?}",
            failure(&line).error
        );
    }

    #[test]
    fn a_list_request_can_ask_for_either_end_of_the_order() {
        for (order, expected) in [
            ("newest", TrafficOrder::Newest),
            ("oldest", TrafficOrder::Oldest),
        ] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","order":"{order}"}}"#
            );

            assert_eq!(list_query(&line).order, expected, "{order}");
        }
    }

    #[test]
    fn a_list_request_that_names_no_order_gets_the_newest_first() {
        // What every client before this parameter existed asked for, and
        // what it still gets.
        let query = list_query(r#"{"version":1,"request_id":"r1","type":"list_traffic"}"#);

        assert_eq!(query.order, TrafficOrder::Newest);
    }

    #[test]
    fn an_order_this_server_does_not_have_is_refused() {
        for order in [r#""duration""#, r#""NEWEST""#, r#""""#, "1"] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","order":{order}}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{order}");
        }
    }

    #[test]
    fn a_parameter_sent_as_null_is_the_parameter_not_being_sent() {
        // The convention every optional parameter here already follows: a
        // client that spells "no filter" as null gets no filter, rather than
        // a refusal it cannot act on.
        let query = list_query(
            r#"{"version":1,"request_id":"r1","type":"list_traffic",
                "order":null,"search":null,"host":null}"#,
        );

        assert_eq!(query.order, TrafficOrder::Newest);
        assert_eq!(query.filter.text, None);
        assert_eq!(query.filter.host, None);
    }

    #[test]
    fn a_cursor_is_refused_alongside_an_order_it_did_not_come_from() {
        // Not refused — dropped. A cursor names a place in the order it was
        // produced in, so the server starts the other direction again rather
        // than answering with the rows on the far side of it.
        let cursor = Cursor::of(&sample_summary());
        let line = format!(
            r#"{{"version":1,"request_id":"r1","type":"list_traffic","order":"oldest","cursor":{}}}"#,
            serde_json::to_string(&cursor).expect("serializes")
        );
        let query = list_query(&line);

        assert_eq!(query.order, TrafficOrder::Oldest);
        assert_eq!(query.after, Some(cursor));
    }

    #[test]
    fn a_status_filter_that_is_not_a_status_is_refused() {
        for status in [
            r#"{"exact":99}"#,
            r#"{"exact":600}"#,
            r#"{"class":0}"#,
            r#"{"class":6}"#,
        ] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","status":{status}}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{status}");
        }
    }

    #[test]
    fn a_status_filter_that_is_not_the_shape_of_one_is_refused() {
        for status in ["404", r#""4xx""#, r#"{"between":[400,499]}"#] {
            let line = format!(
                r#"{{"version":1,"request_id":"r1","type":"list_traffic","status":{status}}}"#
            );

            assert_eq!(refused(&line), ErrorCode::InvalidParameters, "{status}");
        }
    }

    #[test]
    fn a_timestamp_that_is_not_a_time_is_refused() {
        let line = r#"{"version":1,"request_id":"r1","type":"list_traffic",
            "started_at_or_after":{"seconds":1,"nanos":1000000000}}"#;

        assert_eq!(refused(line), ErrorCode::InvalidParameters);
    }

    #[test]
    fn a_time_range_that_runs_backwards_is_refused() {
        let line = r#"{"version":1,"request_id":"r1","type":"list_traffic",
            "started_at_or_after":{"seconds":200,"nanos":0},
            "started_before":{"seconds":100,"nanos":0}}"#;

        assert_eq!(refused(line), ErrorCode::InvalidParameters);
    }

    #[test]
    fn a_time_range_that_starts_and_ends_together_is_accepted() {
        let line = r#"{"version":1,"request_id":"r1","type":"list_traffic",
            "started_at_or_after":{"seconds":100,"nanos":0},
            "started_before":{"seconds":100,"nanos":0}}"#;

        assert!(parsed(line).is_ok());
    }

    #[test]
    fn a_parameter_this_server_does_not_know_is_refused_rather_than_ignored() {
        // A misspelled filter that was quietly dropped would be a client
        // showing the wrong traffic and not knowing it.
        for line in [
            r#"{"version":1,"request_id":"r1","type":"list_traffic","hosts":"example.com"}"#,
            r#"{"version":1,"request_id":"r1","type":"clear_traffic","everything":true}"#,
            r#"{"version":1,"request_id":"r1","type":"subscribe","filter":"all"}"#,
        ] {
            assert_eq!(refused(line), ErrorCode::InvalidParameters, "{line}");
        }
    }

    #[test]
    fn every_message_a_refusal_carries_is_this_servers_own_words() {
        let lines = [
            "not json",
            r#"{"version":1,"type":"clear_traffic"}"#,
            r#"{"version":9,"request_id":"needle","type":"clear_traffic"}"#,
            r#"{"version":1,"request_id":"needle","type":"needle_operation"}"#,
            r#"{"version":1,"request_id":"needle","type":"get_traffic","exchange_id":"needle"}"#,
            r#"{"version":1,"request_id":"needle","type":"list_traffic","host":"needle.example.com"}"#,
        ];

        for line in lines {
            if let Err(failure) = parsed(line) {
                assert!(
                    !failure.error.message.contains("needle"),
                    "{line} was repeated back: {}",
                    failure.error.message
                );
            }
        }
    }
}
