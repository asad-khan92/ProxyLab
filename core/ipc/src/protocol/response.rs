//! What the server tells a client.
//!
//! ```text
//! {"version":1,"request_id":"r1","ok":true,"type":"list_traffic","result":{…}}
//! {"version":1,"request_id":"r1","ok":false,"error":{"code":"not_found","message":"…"}}
//! ```
//!
//! A response always carries `version`, `request_id`, and `ok`. A successful
//! one also names the operation it is answering and carries its `result`; a
//! failed one carries an `error` instead. Both shapes are on every response,
//! so a client never has to guess which it got.
//!
//! # Errors are structured
//!
//! An [`ErrorCode`] is what a client matches on: a fixed set of names that
//! mean the same thing in every version of the protocol. The message beside
//! it is for a person reading a log, and is a phrase written here — never
//! something a client sent, and never anything out of a captured exchange.
//! [`ErrorBody::new`] takes a `&'static str`, so there is no way to pass one
//! in.

use serde::{Deserialize, Serialize};

use proxy_curl::{CurlErrorCode, CurlNote};
use proxy_http::RequestSpec;
use proxy_storage::{
    Cursor, ExchangeId, ExchangeSummary, SavedRequest, SavedRequestId, StreamMessage,
    TrafficExchange,
};
use rules::{Rule, RuleId};

use crate::breakpoint::{Breakpoint, BreakpointId, PauseId};
use crate::lan::LanStatus;
use crate::replay::ReplayOutcome;
use crate::throttle::ThrottleSettings;

use super::request::RequestId;
use super::PROTOCOL_VERSION;

/// One answer to one request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The protocol version this server speaks.
    pub version: u32,
    /// The request being answered, or `null` if it could not be read.
    pub request_id: Option<String>,
    /// Whether the request was carried out.
    pub ok: bool,
    /// What it produced, when it was.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// Why it was not, when it was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    /// A response saying the request was carried out.
    pub fn ok(id: &RequestId, outcome: Outcome) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id: Some(id.as_str().to_owned()),
            ok: true,
            outcome: Some(outcome),
            error: None,
        }
    }

    /// A response saying it was not, and why.
    ///
    /// `id` is `None` when the request could not be read as far as its id,
    /// which is the one case where a client cannot be told which of its
    /// requests this answers.
    pub fn failed(id: Option<&RequestId>, error: ErrorBody) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id: id.map(|id| id.as_str().to_owned()),
            ok: false,
            outcome: None,
            error: Some(error),
        }
    }
}

/// What a request produced.
///
/// Adjacently tagged, so the operation being answered is named beside its
/// result: a client that is multiplexing requests reads `type` rather than
/// having to remember what it asked.
///
/// A whole exchange is much larger than a count, which is what it is: an
/// outcome is built, serialized, and dropped. Boxing it would add an
/// allocation per response and put a `Box` in the middle of a type clients
/// match on, for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "result", rename_all = "snake_case")]
pub enum Outcome {
    /// A page of the traffic list.
    ListTraffic(TrafficList),
    /// One exchange, whole.
    GetTraffic(TrafficDetail),
    /// One exchange removed, or not there to remove.
    DeleteTraffic(Deleted),
    /// The history emptied.
    ClearTraffic(Cleared),
    /// A page of stream messages on one exchange.
    ListStreamMessages(proxy_storage::StreamMessagePage),
    /// One stream message, payload included.
    GetStreamMessage(StreamMessageDetail),
    /// This connection is now receiving traffic events.
    Subscribe(Subscribed),
    /// Every rule, in the order they are consulted in.
    ListRules(RuleList),
    /// One rule.
    GetRule(RuleDetail),
    /// A rule that has just been created.
    CreateRule(RuleDetail),
    /// A rule as it is after being changed.
    UpdateRule(RuleDetail),
    /// One rule removed, or not there to remove.
    DeleteRule(RuleDeleted),
    /// A cURL command read as the request it describes, or as the reason it
    /// describes none.
    ParseCurl(CurlParsed),
    /// A stored request written out as a cURL command.
    CurlFromTraffic(CurlGenerated),
    /// A request sent again, and what came back.
    ReplayRequest(ReplayOutcome),
    /// Where the proxy is listening, and where it could.
    LanStatus(LanStatus),
    /// The same, after being put on the local network.
    EnableLan(LanStatus),
    /// The same, after being taken off it.
    DisableLan(LanStatus),
    /// Every configured breakpoint.
    ListBreakpoints(BreakpointList),
    /// A breakpoint that has just been configured.
    CreateBreakpoint(BreakpointDetail),
    /// A breakpoint as it is after being changed.
    UpdateBreakpoint(BreakpointDetail),
    /// One breakpoint removed, or not there to remove.
    DeleteBreakpoint(BreakpointDeleted),
    /// A paused exchange told to carry on, or not there to tell.
    ContinueBreakpoint(PauseResolved),
    /// A paused exchange told to stop, or not there to tell.
    CancelBreakpoint(PauseResolved),
    /// This connection is now receiving paused exchanges.
    WatchBreakpoints(Watching),
    /// How the proxy is shaping the traffic it carries.
    ThrottleStatus(ThrottleSettings),
    /// The same, after being changed.
    SetThrottle(ThrottleSettings),
    /// Every saved request, in the order they were saved.
    ListSavedRequests(SavedRequestList),
    /// One saved request.
    GetSavedRequest(SavedRequestDetail),
    /// A request that has just been saved.
    CreateSavedRequest(SavedRequestDetail),
    /// The same, saved out of one captured exchange.
    SaveRequestFromTraffic(SavedRequestDetail),
    /// A saved request as it is after being changed.
    UpdateSavedRequest(SavedRequestDetail),
    /// One saved request removed, or not there to remove.
    DeleteSavedRequest(SavedRequestDeleted),
    /// Captured traffic written out as a HAR file.
    ExportHar(HarExported),
    /// A HAR file read in as captured traffic.
    ImportHar(HarImported),
}

/// Every saved request there is.
///
/// Not paged, for the reason [`RuleList`] is not: they are written by a
/// person, there are tens of them rather than thousands, and the order they
/// were saved in is the one thing the list has to get right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequestList {
    /// The requests, in the order they were saved.
    pub saved_requests: Vec<SavedRequest>,
}

/// One saved request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequestDetail {
    /// The request, as the server now holds it.
    pub saved_request: SavedRequest,
}

/// The result of removing one saved request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequestDeleted {
    /// Which request.
    pub saved_request_id: SavedRequestId,
    /// Whether there was one to remove.
    pub deleted: bool,
}

/// The result of writing a HAR file.
///
/// What was written, and not where: the path came from the client and this
/// server does not repeat back what a client sent. The counts are what a
/// person is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarExported {
    /// How many exchanges were written.
    pub entries: u64,
    /// How many bytes the file is.
    pub bytes: u64,
    /// Whether there were more matching exchanges than one export writes.
    ///
    /// An export of everything a filter matches stops at a limit, and a
    /// person who asked for their whole history should be told the file has
    /// some of it rather than being left to count.
    pub truncated: bool,
}

/// The result of reading a HAR file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarImported {
    /// How many entries became stored exchanges.
    pub imported: u64,
    /// How many did not.
    ///
    /// An entry that names no request, and an entry past the limit one
    /// import takes. Counted rather than passed over, so a shorter list than
    /// the file had is a thing a person is told about.
    pub skipped: u64,
}

/// Every breakpoint there is.
///
/// Not paged, for the reason [`RuleList`] is not: breakpoints are written by
/// a person and there are a handful of them, and the order they are consulted
/// in is the one thing the list has to get right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointList {
    /// The breakpoints, in the order they are consulted in.
    pub breakpoints: Vec<Breakpoint>,
}

/// One breakpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointDetail {
    /// The breakpoint, as the server now holds it.
    pub breakpoint: Breakpoint,
}

/// The result of removing one breakpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakpointDeleted {
    /// Which breakpoint.
    pub breakpoint_id: BreakpointId,
    /// Whether there was one to remove.
    pub deleted: bool,
}

/// The result of deciding what happens to a paused exchange.
///
/// `resolved` is false for a pause that is already over — one another client
/// answered, one whose deadline passed, or one released because the last
/// watcher went away. Not an error: the decision was carried out as far as
/// there was anything to carry it out on, and the client is told there was
/// nothing left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseResolved {
    /// Which pause.
    pub pause_id: PauseId,
    /// Whether it was still being held.
    pub resolved: bool,
}

/// The result of watching for paused exchanges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watching {
    /// Always true: a failure to watch is an error response instead.
    pub watching: bool,
}

/// A page of the traffic list.
///
/// Summaries only: a page is what a list draws, and the captured bodies of
/// fifty exchanges are not part of drawing it. An exchange's headers and
/// bodies come from [`Outcome::GetTraffic`], one exchange at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficList {
    /// The exchanges, newest first.
    pub exchanges: Vec<ExchangeSummary>,
    /// Where to carry on from, or `null` at the end of the list.
    pub next_cursor: Option<Cursor>,
    /// How many exchanges the filter matches in total, across every page.
    pub total: u64,
}

/// One stream message, payload included.
///
/// The payload is as much as capture kept, which is already bounded. A list
/// of messages never carries it; a client that wants one payload asks for
/// that message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMessageDetail {
    /// The message, as storage holds it.
    pub message: StreamMessage,
}

/// One stored exchange, with everything that was kept of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficDetail {
    /// The exchange: request and response, headers, captured bodies, timing,
    /// and the error that explains a missing response.
    pub exchange: TrafficExchange,
}

/// The result of removing one exchange.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    /// Which exchange.
    pub exchange_id: ExchangeId,
    /// Whether there was one to remove.
    pub deleted: bool,
}

/// The result of emptying the history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cleared {
    /// How many exchanges were removed.
    pub removed: u64,
}

/// The result of subscribing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribed {
    /// Always true: a failure to subscribe is an error response instead.
    pub subscribed: bool,
}

/// Every rule there is.
///
/// Not paged: rules are written by a person, there are tens of them rather
/// than thousands, and a rule list that arrived in pages could be shown in an
/// order that is not the order they are consulted in — which is the one thing
/// a rule list has to get right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleList {
    /// The rules, in the order they are consulted: priority, then age.
    pub rules: Vec<Rule>,
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDetail {
    /// The rule, as the server now holds it.
    pub rule: Rule,
}

/// The result of removing one rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDeleted {
    /// Which rule.
    pub rule_id: RuleId,
    /// Whether there was one to remove.
    pub deleted: bool,
}

/// What a cURL command came to.
///
/// A command that does not parse is carried here rather than as an IPC
/// error, and on purpose. An [`ErrorBody`] message is a fixed phrase chosen
/// in this crate and may name nothing a client sent — which is right, and
/// which would make "unsupported cURL option" unable to say *which* option.
/// Parsing is the operation, and a command that cannot be parsed is its
/// result: the request was carried out, and the answer is no.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurlParsed {
    /// The request the command describes, or `null` when it describes none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestSpec>,
    /// What the parser has to say about a command it did read.
    #[serde(default)]
    pub notes: Vec<CurlNote>,
    /// Why there is no request, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<CurlProblem>,
}

/// Why a cURL command describes no request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurlProblem {
    /// What kind of problem it is, for a client to match on.
    pub code: CurlErrorCode,
    /// What it is, for a person to read.
    pub message: String,
    /// The option it is about, when it is about one.
    ///
    /// The only thing out of a client's own command that ever comes back,
    /// and it is checked before it does: see `proxy-curl`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
}

/// A stored request, written out as a cURL command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurlGenerated {
    /// The command.
    pub curl: String,
    /// Whether the captured body it was written from was truncated, so the
    /// command sends less than the original request did.
    pub truncated: bool,
}

/// Why a request was not carried out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What kind of failure it was, for a client to match on.
    pub code: ErrorCode,
    /// What it was, for a person to read.
    pub message: String,
}

impl ErrorBody {
    /// A failure of `code`, described by `message`.
    ///
    /// `message` is a `&'static str` on purpose: a phrase written for this
    /// purpose, and never the text of whatever went wrong. An error goes to
    /// a client and into a log, and neither is a place for what a client
    /// sent or what an exchange carried.
    pub fn new(code: ErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_owned(),
        }
    }
}

/// What kind of failure a response is reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The line was not one JSON object.
    InvalidJson,
    /// The request named a protocol version this server does not speak.
    UnsupportedVersion,
    /// The request carried no request id.
    MissingRequestId,
    /// The request id was not one this server will echo.
    InvalidRequestId,
    /// The request named no operation, or one this server does not have.
    UnknownOperation,
    /// The operation's parameters were wrong.
    InvalidParameters,
    /// The exchange id was not an exchange id.
    InvalidExchangeId,
    /// The stream message id was not a stream message id.
    InvalidStreamMessageId,
    /// The rule id was not a rule id.
    InvalidRuleId,
    /// The breakpoint id was not a breakpoint id.
    InvalidBreakpointId,
    /// The breakpoint itself was not one this server can act on.
    InvalidBreakpoint,
    /// The pause id was not a pause id.
    InvalidPauseId,
    /// The rule itself was not one this server can store or act on.
    InvalidRule,
    /// The saved request id was not a saved request id.
    InvalidSavedRequestId,
    /// The saved request itself was not one this server can keep or send.
    InvalidSavedRequest,
    /// The request to send was not one this server can send.
    InvalidRequest,
    /// There is no such exchange.
    NotFound,
    /// The request was longer than this server will read.
    MessageTooLarge,
    /// This connection is already receiving traffic events.
    AlreadySubscribed,
    /// The stored traffic could not be read or written.
    StorageUnavailable,
    /// The proxy could not be put on the local network at the address asked
    /// for: it is not one this machine has, or the socket could not be bound.
    LanUnavailable,
    /// The HAR file could not be written or read: the path is not one the
    /// core will use, the file is not a HAR document, or the file system
    /// refused the work.
    HarUnavailable,
    /// The server failed in a way it has nothing more specific to say about.
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn id() -> RequestId {
        let request =
            super::super::parse(br#"{"version":1,"request_id":"r1","type":"clear_traffic"}"#)
                .expect("a valid request");

        request.id
    }

    #[test]
    fn a_successful_response_names_the_request_the_operation_and_the_result() {
        let response = Response::ok(&id(), Outcome::ClearTraffic(Cleared { removed: 3 }));

        assert_eq!(
            serde_json::to_value(&response).expect("serializes"),
            json!({
                "version": 1,
                "request_id": "r1",
                "ok": true,
                "type": "clear_traffic",
                "result": {"removed": 3},
            })
        );
    }

    #[test]
    fn a_failed_response_carries_a_code_and_a_message() {
        let response = Response::failed(
            Some(&id()),
            ErrorBody::new(ErrorCode::NotFound, "there is no such exchange"),
        );

        assert_eq!(
            serde_json::to_value(&response).expect("serializes"),
            json!({
                "version": 1,
                "request_id": "r1",
                "ok": false,
                "error": {"code": "not_found", "message": "there is no such exchange"},
            })
        );
    }

    #[test]
    fn a_failure_before_the_request_id_was_read_says_so() {
        let response = Response::failed(
            None,
            ErrorBody::new(ErrorCode::InvalidJson, "a request must be one JSON object"),
        );
        let json = serde_json::to_value(&response).expect("serializes");

        assert_eq!(json["request_id"], serde_json::Value::Null);
        assert_eq!(json["ok"], false);
    }

    #[test]
    fn every_error_code_has_a_name_of_its_own() {
        let codes = [
            ErrorCode::InvalidJson,
            ErrorCode::UnsupportedVersion,
            ErrorCode::MissingRequestId,
            ErrorCode::InvalidRequestId,
            ErrorCode::UnknownOperation,
            ErrorCode::InvalidParameters,
            ErrorCode::InvalidExchangeId,
            ErrorCode::InvalidStreamMessageId,
            ErrorCode::NotFound,
            ErrorCode::InvalidRuleId,
            ErrorCode::InvalidRule,
            ErrorCode::InvalidBreakpointId,
            ErrorCode::InvalidBreakpoint,
            ErrorCode::InvalidPauseId,
            ErrorCode::InvalidSavedRequestId,
            ErrorCode::InvalidSavedRequest,
            ErrorCode::MessageTooLarge,
            ErrorCode::AlreadySubscribed,
            ErrorCode::StorageUnavailable,
            ErrorCode::LanUnavailable,
            ErrorCode::HarUnavailable,
            ErrorCode::Internal,
        ];

        let names: std::collections::BTreeSet<String> = codes
            .iter()
            .map(|code| serde_json::to_string(code).expect("serializes"))
            .collect();

        assert_eq!(names.len(), codes.len());
        assert!(names.contains(r#""invalid_json""#), "{names:?}");
    }

    #[test]
    fn a_response_round_trips_through_json() {
        let response = Response::ok(
            &id(),
            Outcome::DeleteTraffic(Deleted {
                exchange_id: ExchangeId::next(),
                deleted: true,
            }),
        );

        let line = serde_json::to_string(&response).expect("serializes");
        let back: Response = serde_json::from_str(&line).expect("deserializes");

        assert_eq!(back, response);
    }

    #[test]
    fn an_empty_page_is_a_page() {
        let response = Response::ok(
            &id(),
            Outcome::ListTraffic(TrafficList {
                exchanges: Vec::new(),
                next_cursor: None,
                total: 0,
            }),
        );
        let json = serde_json::to_value(&response).expect("serializes");

        assert_eq!(json["result"]["exchanges"], json!([]));
        assert_eq!(json["result"]["next_cursor"], serde_json::Value::Null);
        assert_eq!(json["result"]["total"], 0);
    }
}
