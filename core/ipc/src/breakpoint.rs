//! Holding a request or a response still while a developer edits it.
//!
//! ```text
//! client ──▶ request ──┐
//!                      │  a breakpoint matches
//!                      ▼
//!              ┌───────────────┐   breakpoint_paused   ┌────┐
//!              │    paused     │ ────────────────────▶ │ UI │
//!              │  (held here)  │ ◀──────────────────── │    │
//!              └───────┬───────┘  continue / cancel    └────┘
//!                      │
//!         ┌────────────┴────────────┐
//!         ▼                         ▼
//!  the edited request         nothing is sent
//!  goes upstream              upstream at all
//! ```
//!
//! This module is the *boundary*, not the implementation, for the same
//! reason [`replay`](crate::replay) and [`lan`](crate::lan) are: holding a
//! request still means holding a connection the proxy engine owns, and that
//! crate depends on this one and cannot be depended on by it. So the reach
//! is declared here as a trait and implemented in `proxy-engine`.
//!
//! # A breakpoint is not a rule
//!
//! A rule is a decision the proxy makes by itself, on every matching request,
//! for as long as it exists — see the `rules` crate, which Map Local, Map
//! Remote, and Block all belong to. A breakpoint decides nothing: it stops,
//! asks a person, and does what they say. So the two are separate things with
//! separate operations, and a breakpoint is not persisted — a ProxyLab that
//! has just started pauses nothing, exactly as one that has just started is
//! not on the local network.
//!
//! What they share is the matching. [`RuleMatcher`] already says "this host,
//! this path, this method", which is the whole of what a breakpoint needs to
//! match on, and a second matcher spelled the same way would be two to keep
//! in step.
//!
//! # Nothing pauses unless somebody is watching
//!
//! A pause is only ever created while at least one client has asked to
//! receive them, and every pending pause is released the moment the last of
//! those clients goes away. That is the whole of the disconnect story: a UI
//! that crashes, is quit, or loses its socket cannot leave a request held
//! still, because the pause that held it does not outlive the connection that
//! would have answered it. A watcher that stays connected and stops answering
//! is covered separately, by a deadline the engine applies.
//!
//! # What may be edited, and how much of it
//!
//! A paused request carries its method, its URL, its headers, and as much of
//! its body as [`MAX_BREAKPOINT_BODY_BYTES`]; a paused response carries its
//! status, its headers, and the same much of its body. An edit carries
//! whichever of those it changes and nothing else, so a client that changes
//! one header sends one header section and leaves the body alone.
//!
//! A body longer than the limit is carried truncated and **cannot be
//! edited** — a client that sends an edit for one is refused. That is the
//! honest answer rather than a silent one: the bytes the client would be
//! editing are not all the bytes there are, and continuing without a body
//! edit forwards the original body in full, however long it is. See
//! [`MAX_BREAKPOINT_BODY_BYTES`] for why the limit is where it is.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use proxy_storage::{BodyCapture, Headers, StreamError};
use rules::RuleMatcher;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::protocol::{ErrorBody, ErrorCode};

/// The most of a body that is carried to a client, and the most an edit may
/// carry back.
///
/// A request line is bounded by
/// [`MAX_REQUEST_BYTES`](crate::protocol::MAX_REQUEST_BYTES), which is 64 KiB,
/// and a body travels inside one as base64 — a third larger than the bytes it
/// describes. 32 KiB of body is 44 KiB of base64, which leaves room for a
/// URL, a header section, and the JSON around them inside one line.
///
/// It is not a limit on what the proxy carries. A body longer than this is
/// forwarded in full; what it cannot be is edited, because the client was
/// only shown the first part of it.
pub const MAX_BREAKPOINT_BODY_BYTES: usize = 32 * 1024;

/// How many pauses a watcher may fall behind by before it starts missing
/// them.
///
/// A pause waits for a person, so they arrive at human speed and a watcher
/// has as long as it likes to read each one. This is generous for that, and
/// bounded so that a client which has stopped reading costs a fixed amount of
/// memory rather than an unbounded one.
pub const PAUSE_STREAM_CAPACITY: usize = 256;

/// Identifies one configured breakpoint, for as long as it is configured.
///
/// A UUIDv7 for the same reason [`RuleId`](rules::RuleId) is one: it is
/// unique across runs and ordered by the moment it was minted, so the order
/// breakpoints are consulted in is the order they were created in without a
/// second field to say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BreakpointId(Uuid);

impl BreakpointId {
    /// A new id, for a breakpoint being configured now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }

    /// The id as the UUID it is.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl FromStr for BreakpointId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for BreakpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// Identifies one exchange that is being held still.
///
/// Minted when the pause starts and forgotten when it ends, so it names a
/// moment rather than a thing: a pause id that has been resolved is a pause
/// id nothing will answer to again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PauseId(Uuid);

impl PauseId {
    /// A new id, for an exchange being paused now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }
}

impl FromStr for PauseId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for PauseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// Which half of an exchange a breakpoint stops.
///
/// Two halves, two breakpoints. A developer who wants to see both sides of
/// one request configures one of each rather than one that means both,
/// because a request breakpoint and a response breakpoint stop at different
/// moments and offer different things to edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointDirection {
    /// Before the request is sent upstream.
    Request,
    /// After the response has arrived and before the client is given it.
    Response,
    /// A text WebSocket message, before it continues in its direction.
    WebSocket,
}

impl BreakpointDirection {
    /// The direction as it is named on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::WebSocket => "websocket",
        }
    }
}

impl fmt::Display for BreakpointDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One configured breakpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// Which breakpoint this is.
    pub id: BreakpointId,
    /// Whether it is consulted at all.
    pub enabled: bool,
    /// Which half of an exchange it stops.
    pub direction: BreakpointDirection,
    /// Which exchanges it is for: host, path, and method, as a rule spells
    /// them. A criterion that is not set matches anything.
    pub matcher: RuleMatcher,
}

impl Breakpoint {
    /// The one-line form that is safe to write to a log.
    ///
    /// The id, whether it is on, and which half it stops. Not its matcher,
    /// which names hosts and paths a developer is working on.
    pub fn summary(&self) -> String {
        format!(
            "{} {} ({})",
            self.id,
            self.direction,
            if self.enabled { "enabled" } else { "disabled" }
        )
    }
}

/// A breakpoint that has not been configured yet.
///
/// What a client sends to configure one: everything but the id, which is the
/// server's to mint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewBreakpoint {
    /// Whether the breakpoint is consulted once it exists.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Which half of an exchange it stops.
    pub direction: BreakpointDirection,
    /// Which exchanges it is for.
    #[serde(default)]
    pub matcher: RuleMatcher,
}

impl NewBreakpoint {
    /// A breakpoint on `direction` for what `matcher` describes.
    pub fn new(direction: BreakpointDirection, matcher: RuleMatcher) -> Self {
        Self {
            enabled: true,
            direction,
            matcher,
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

/// A change to a breakpoint that already exists.
///
/// Every field is optional, and one that is not named is not changed — the
/// same shape [`RuleUpdate`](rules::RuleUpdate) has, and for the same reason:
/// "turn this breakpoint off" is one field rather than the whole breakpoint
/// read back, changed, and sent again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointUpdate {
    /// Whether the breakpoint is consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Which half of an exchange it stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<BreakpointDirection>,
    /// Which exchanges it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<RuleMatcher>,
}

impl BreakpointUpdate {
    /// Whether this update changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// One exchange being held still, as a client is shown it.
///
/// `request` is present for a request pause and `response` for a response
/// pause; never both, because a breakpoint stops one half. The method and
/// the URL are on the outside either way, so a client can label a paused
/// response without having to be sent the request that caused it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedExchange {
    /// Which pause this is, and what a decision about it names.
    pub pause_id: PauseId,
    /// Which breakpoint stopped it.
    pub breakpoint_id: BreakpointId,
    /// Which half is being held.
    pub direction: BreakpointDirection,
    /// The request's method, for a client to label the pause with.
    pub method: String,
    /// The request's URL, for the same.
    pub url: String,
    /// The request as it stands, when the request is what is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<PausedRequest>,
    /// The response as the origin sent it, when the response is what is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<PausedResponse>,
    /// The text WebSocket message, when a WebSocket message is what is held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket: Option<PausedWebSocketMessage>,
}

/// A text WebSocket message that has not been forwarded yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedWebSocketMessage {
    /// Which way the message is travelling.
    pub direction: proxy_storage::StreamDirection,
    /// As much of the payload as [`MAX_BREAKPOINT_BODY_BYTES`].
    pub payload: BodyCapture,
}

/// A request that has not been sent yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedRequest {
    /// The method it would be sent with.
    pub method: String,
    /// The URL it would be sent to.
    pub url: String,
    /// The headers it would be sent with, in order.
    pub headers: Headers,
    /// As much of the body as [`MAX_BREAKPOINT_BODY_BYTES`], and how long it
    /// was. A truncated body cannot be edited.
    pub body: BodyCapture,
}

/// A response that has arrived and has not been delivered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PausedResponse {
    /// The status the origin answered with.
    pub status: u16,
    /// The headers it answered with, in order.
    pub headers: Headers,
    /// As much of the body as [`MAX_BREAKPOINT_BODY_BYTES`], and how long it
    /// was. A truncated body cannot be edited.
    pub body: BodyCapture,
}

/// What to change about a paused request before it is sent.
///
/// A field that is not named is not changed. `body` is the one worth saying
/// twice: leaving it out forwards the body the client sent, in full and
/// still streaming, which is the only thing that can be done with a body too
/// long to have been shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEdit {
    /// The method to send it with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The URL to send it to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The headers to send it with, replacing all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Headers>,
    /// The body to send, replacing all of it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_bytes"
    )]
    pub body: Option<Vec<u8>>,
}

impl RequestEdit {
    /// Whether this edit changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// What to change about a paused response before it is delivered.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEdit {
    /// The status to deliver it with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// The headers to deliver it with, replacing all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Headers>,
    /// The body to deliver, replacing all of it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_bytes"
    )]
    pub body: Option<Vec<u8>>,
}

impl ResponseEdit {
    /// Whether this edit changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// What to change about a paused text WebSocket message before it continues.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSocketEdit {
    /// The payload to send, replacing all of it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_bytes"
    )]
    pub payload: Option<Vec<u8>>,
}

impl WebSocketEdit {
    /// Whether this edit changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// An optional byte string, written as base64 when it is there.
///
/// `proxy-storage`'s own base64 module handles the bytes; this is the
/// `Option` around them, which serde cannot derive through a `with`.
mod optional_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// The bytes as base64, for a field that is only serialized when present.
    pub fn serialize<S: Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => Wrapped(bytes.clone()).serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    /// The same, back again.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(Option::<Wrapped>::deserialize(deserializer)?.map(|wrapped| wrapped.0))
    }

    /// One byte string in the encoding the model already uses for one.
    #[derive(Serialize, Deserialize)]
    #[serde(transparent)]
    struct Wrapped(#[serde(with = "proxy_storage::base64_bytes")] Vec<u8>);
}

/// What a client decided about a paused exchange.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseDecision {
    /// Let it carry on, with these changes.
    ///
    /// Exactly one of the two is ever acted on — the one matching the
    /// direction the pause was for — and neither has to be there: continuing
    /// with no edit at all is what a developer who only wanted to look does.
    Continue {
        /// What to change about the request, for a request pause.
        request: Option<RequestEdit>,
        /// What to change about the response, for a response pause.
        response: Option<ResponseEdit>,
        /// What to change about a text WebSocket message.
        websocket: Option<WebSocketEdit>,
    },
    /// Stop it here. Nothing is sent upstream, and nothing is delivered.
    Cancel,
}

impl PauseDecision {
    /// Carrying on with nothing changed.
    ///
    /// What a pause that nobody answered comes to: the exchange goes through
    /// exactly as it would have if no breakpoint had matched.
    pub fn unchanged() -> Self {
        Self::Continue {
            request: None,
            response: None,
            websocket: None,
        }
    }
}

/// Why a pause ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    /// A client said to carry on.
    Continued,
    /// A client said to stop it.
    Cancelled,
    /// Nobody was left to answer, so it carried on unchanged.
    Abandoned,
    /// Nobody answered in time, so it carried on unchanged.
    TimedOut,
}

/// What a watching client is told.
///
/// One variant is much larger than the other, which is what it is: a notice
/// is built, sent, and dropped. Boxing the paused exchange would add an
/// allocation per pause and put a `Box` in the middle of a type clients match
/// on, for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseNotice {
    /// An exchange is being held still.
    Paused(PausedExchange),
    /// One that was being held is not any more, and this is why.
    ///
    /// Sent for every ending, including the client's own: a watcher that did
    /// not send the decision still has to stop showing the pause, and two
    /// watchers are told the same thing.
    Released {
        /// Which pause ended.
        pause_id: PauseId,
        /// Why it did.
        reason: ReleaseReason,
    },
}

/// Something that can hold exchanges still and be told what to do with them.
///
/// Implemented in `proxy-engine`. Every method is synchronous: what is behind
/// it is a list of breakpoints and a table of pending pauses, both in memory,
/// and none of them waits for anything.
pub trait BreakpointControl: Send + Sync + fmt::Debug {
    /// Every configured breakpoint, in the order they are consulted in.
    fn list(&self) -> Vec<Breakpoint>;

    /// Configures a new breakpoint.
    fn create(&self, new: NewBreakpoint) -> Breakpoint;

    /// Changes one that exists, or says there is no such breakpoint.
    fn update(&self, id: BreakpointId, update: BreakpointUpdate) -> Option<Breakpoint>;

    /// Removes one, and says whether there was one to remove.
    fn delete(&self, id: BreakpointId) -> bool;

    /// Tells a paused exchange what to do, and says whether there was one
    /// still waiting to be told.
    fn resolve(&self, id: PauseId, decision: PauseDecision) -> bool;

    /// Starts receiving pauses, and counts this client as watching.
    ///
    /// Paired with [`detach`](Self::detach) by [`PauseStream`], which is the
    /// only thing that should call either.
    fn attach(&self) -> broadcast::Receiver<PauseNotice>;

    /// Stops counting a client as watching.
    ///
    /// When the last one goes, every pending pause is released unchanged:
    /// nothing may be left held still with nobody to free it.
    fn detach(&self);
}

/// One client's view of the exchanges being held still.
///
/// Holding one is what makes the proxy pause at all, and dropping it is what
/// stops it: the count of these is the count of clients watching, and the
/// engine releases everything pending when it reaches zero. So a connection
/// keeps its stream for exactly as long as it lives, and the disconnect case
/// needs no handling of its own.
#[derive(Debug)]
pub struct PauseStream {
    receiver: broadcast::Receiver<PauseNotice>,
    control: Arc<dyn BreakpointControl>,
}

impl PauseStream {
    /// Starts watching `control`.
    pub fn attach(control: Arc<dyn BreakpointControl>) -> Self {
        let receiver = control.attach();

        Self { receiver, control }
    }

    /// Waits for the next notice.
    ///
    /// Cancel safe: a wait that is interrupted consumes nothing, so the
    /// notices already queued are still there next time.
    pub async fn recv(&mut self) -> Result<PauseNotice, StreamError> {
        match self.receiver.recv().await {
            Ok(notice) => Ok(notice),
            Err(broadcast::error::RecvError::Lagged(missed)) => Err(StreamError::Lagged(missed)),
            Err(broadcast::error::RecvError::Closed) => Err(StreamError::Closed),
        }
    }
}

/// A client that has gone away is a client that is no longer watching, and
/// that is what releases anything it was holding.
impl Drop for PauseStream {
    fn drop(&mut self) {
        self.control.detach();
    }
}

/// Why a breakpoint or an edit was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakpointRefusal {
    /// A matching criterion was set to something that matches nothing.
    Matcher,
    /// An edited status is not one a response may carry.
    Status,
    /// An edited header name or value is not one a message may carry.
    Header,
    /// An edited body is longer than [`MAX_BREAKPOINT_BODY_BYTES`].
    BodyTooLarge,
    /// A `continue_breakpoint` carried an edit for the half it is not for.
    WrongDirection,
}

impl BreakpointRefusal {
    /// The error a client is told, which is a phrase chosen here.
    pub fn error(self) -> ErrorBody {
        match self {
            Self::Matcher => ErrorBody::new(
                ErrorCode::InvalidBreakpoint,
                "a breakpoint's matching criteria are not ones that could match",
            ),
            Self::Status => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "an edited status must be a code from 100 to 599",
            ),
            Self::Header => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "an edited header name or value is not one a message may carry",
            ),
            Self::BodyTooLarge => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "an edited body must be at most 32768 bytes",
            ),
            Self::WrongDirection => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "a request edit answers a request pause, a response edit a response pause, and a websocket edit a websocket pause",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::HeaderValue;
    use serde_json::json;

    fn matcher() -> RuleMatcher {
        RuleMatcher::any().with_host("api.example.com")
    }

    fn breakpoint() -> Breakpoint {
        Breakpoint {
            id: BreakpointId::next(),
            enabled: true,
            direction: BreakpointDirection::Request,
            matcher: matcher(),
        }
    }

    fn headers() -> Headers {
        let mut headers = Headers::new();
        headers.push("Accept", HeaderValue::Text("application/json".into()));
        headers
    }

    #[test]
    fn a_breakpoint_names_its_direction_and_its_matcher() {
        let json = serde_json::to_value(breakpoint()).expect("serializes");

        assert_eq!(json["enabled"], true);
        assert_eq!(json["direction"], "request");
        assert_eq!(json["matcher"]["host"]["exact"], "api.example.com");
    }

    #[test]
    fn a_breakpoint_round_trips_through_json() {
        let breakpoint = breakpoint();
        let line = serde_json::to_string(&breakpoint).expect("serializes");
        let back: Breakpoint = serde_json::from_str(&line).expect("deserializes");

        assert_eq!(back, breakpoint);
    }

    #[test]
    fn a_summary_says_what_the_breakpoint_is_without_naming_the_host() {
        let summary = breakpoint().summary();

        assert!(summary.contains("request"), "{summary}");
        assert!(summary.contains("enabled"), "{summary}");
        assert!(!summary.contains("api.example.com"), "{summary}");
    }

    #[test]
    fn a_new_breakpoint_is_enabled_unless_it_says_otherwise() {
        let new: NewBreakpoint =
            serde_json::from_value(json!({"direction": "response"})).expect("deserializes");

        assert!(new.enabled);
        assert_eq!(new.direction, BreakpointDirection::Response);
        assert!(new.matcher.is_any());
    }

    #[test]
    fn an_update_that_names_nothing_changes_nothing() {
        assert!(BreakpointUpdate::default().is_empty());
        assert!(!BreakpointUpdate {
            enabled: Some(false),
            ..BreakpointUpdate::default()
        }
        .is_empty());
    }

    #[test]
    fn an_edit_carries_its_body_as_base64() {
        let edit = RequestEdit {
            body: Some(b"hello".to_vec()),
            ..RequestEdit::default()
        };

        let json = serde_json::to_value(&edit).expect("serializes");

        assert_eq!(json["body"], "aGVsbG8=");
        assert!(json.get("method").is_none(), "{json}");
    }

    #[test]
    fn an_edit_that_leaves_the_body_alone_carries_no_body_at_all() {
        let edit = RequestEdit {
            method: Some("PUT".to_owned()),
            ..RequestEdit::default()
        };
        let json = serde_json::to_value(&edit).expect("serializes");

        assert!(json.get("body").is_none(), "{json}");
    }

    #[test]
    fn an_edit_round_trips_through_json() {
        for edit in [
            RequestEdit::default(),
            RequestEdit {
                method: Some("POST".to_owned()),
                url: Some("https://api.example.com/v2".to_owned()),
                headers: Some(headers()),
                body: Some(vec![0, 159, 146, 150]),
            },
        ] {
            let line = serde_json::to_string(&edit).expect("serializes");
            let back: RequestEdit = serde_json::from_str(&line).expect("deserializes");

            assert_eq!(back, edit);
        }
    }

    #[test]
    fn a_response_edit_round_trips_through_json() {
        let edit = ResponseEdit {
            status: Some(503),
            headers: Some(headers()),
            body: Some(b"{}".to_vec()),
        };

        let line = serde_json::to_string(&edit).expect("serializes");
        let back: ResponseEdit = serde_json::from_str(&line).expect("deserializes");

        assert_eq!(back, edit);
    }

    #[test]
    fn a_paused_request_carries_the_parts_that_can_be_edited() {
        let paused = PausedExchange {
            pause_id: PauseId::next(),
            breakpoint_id: BreakpointId::next(),
            direction: BreakpointDirection::Request,
            method: "POST".to_owned(),
            url: "https://api.example.com/orders".to_owned(),
            request: Some(PausedRequest {
                method: "POST".to_owned(),
                url: "https://api.example.com/orders".to_owned(),
                headers: headers(),
                body: BodyCapture::complete(b"{}".to_vec()),
            }),
            response: None,
            websocket: None,
        };

        let json = serde_json::to_value(&paused).expect("serializes");

        assert_eq!(json["direction"], "request");
        assert_eq!(json["request"]["method"], "POST");
        assert_eq!(json["request"]["body"]["data"], "e30=");
        assert!(json.get("response").is_none(), "{json}");
    }

    #[test]
    fn a_decision_that_changes_nothing_still_carries_on() {
        assert_eq!(
            PauseDecision::unchanged(),
            PauseDecision::Continue {
                request: None,
                response: None,
                websocket: None,
            }
        );
    }

    #[test]
    fn every_release_reason_has_a_name_of_its_own() {
        let names: std::collections::BTreeSet<String> = [
            ReleaseReason::Continued,
            ReleaseReason::Cancelled,
            ReleaseReason::Abandoned,
            ReleaseReason::TimedOut,
        ]
        .iter()
        .map(|reason| serde_json::to_string(reason).expect("serializes"))
        .collect();

        assert_eq!(names.len(), 4);
        assert!(names.contains(r#""timed_out""#), "{names:?}");
    }

    #[test]
    fn a_refusal_never_repeats_what_the_client_sent() {
        for refusal in [
            BreakpointRefusal::Matcher,
            BreakpointRefusal::Status,
            BreakpointRefusal::Header,
            BreakpointRefusal::BodyTooLarge,
            BreakpointRefusal::WrongDirection,
        ] {
            let message = refusal.error().message;

            assert!(!message.is_empty());
            assert!(!message.contains("example.com"), "{message}");
        }
    }

    #[test]
    fn an_id_is_written_the_way_a_uuid_is_written() {
        let id = BreakpointId::next();
        let text = id.to_string();

        assert_eq!(text.len(), 36);
        assert_eq!(text.parse::<BreakpointId>().expect("parses"), id);
    }
}
