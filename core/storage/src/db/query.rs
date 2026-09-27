//! Asking the database for traffic.
//!
//! ```text
//! TrafficQuery
//! ├── filter      host, method, status, time range
//! ├── limit       how many rows
//! └── after       where the previous page stopped
//!         ↓
//!    TrafficPage
//!    ├── exchanges   newest first
//!    └── next        the cursor for the page after this one
//! ```
//!
//! # One order, read in either direction
//!
//! Exchanges are ordered by the millisecond they started in, and two that
//! started in the same millisecond are ordered by their ids — which are
//! themselves ordered by when they were minted. So the order is total and
//! never depends on the order rows happen to be stored in.
//!
//! [`TrafficOrder`] chooses which end that order is read from. Both ends use
//! the same index: `exchanges_newest` is declared descending, and a database
//! reads an index backwards as readily as forwards, so oldest-first costs
//! no second index and no sort.
//!
//! # Paging by cursor rather than by offset
//!
//! A [`Cursor`] is the position of the last row of a page. The next page is
//! the rows that come after it in that order, which is a comparison the
//! index answers directly. Offsets were the alternative and were rejected
//! for the reason they always are: traffic arrives while a UI is paging
//! through it, and with an offset every new exchange shifts every later page
//! along by one, so rows get shown twice or skipped. A cursor names a place
//! in the order, so new traffic appears where it belongs — at the top, on
//! the first page — and pages already fetched stay put.
//!
//! # What the filters are for
//!
//! The traffic list: *this host*, *these requests*, *what failed*, *the last
//! ten minutes*, *the ones with `login` in them*. They combine with `and`.
//!
//! [`TrafficFilter::text`] is the search box. There is no query language and
//! no operators. The pattern is a substring by default, optionally a regular
//! expression, and it is matched against the URL (which includes the path),
//! the host, the method, the status, the headers, the captured bodies, and
//! stream-message text. Binary WebSocket payloads are matched only as
//! metadata — size and direction. File-backed bodies are read one at a time.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::error::TrafficError;
use crate::timing::Timestamp;
use crate::traffic::{Destination, ExchangeId, HttpVersion, Scheme, TrafficExchange};

use super::encoding;

/// How many exchanges a page holds when nothing says otherwise.
pub const DEFAULT_PAGE_LIMIT: usize = 100;

/// The most exchanges one page may hold.
///
/// A page is something a UI draws, and a query that asks for the whole
/// database is a query that has forgotten it is paging. Anything larger is
/// clamped to this rather than refused.
pub const MAX_PAGE_LIMIT: usize = 1_000;

/// Which exchanges a query is asking for.
///
/// Every field is optional and they combine with `and`: an empty filter
/// matches everything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficFilter {
    /// Only exchanges whose destination host is this one.
    ///
    /// Compared without regard to case, because host names are.
    pub host: Option<String>,
    /// Only exchanges whose request used this method, spelled exactly.
    pub method: Option<String>,
    /// Only exchanges whose response matched this.
    pub status: Option<StatusFilter>,
    /// Only exchanges with this text somewhere in the captured traffic.
    ///
    /// Matched against the host, the request URL (and so the path), the
    /// method, the status, both header lists, both captured bodies, SSE
    /// event fields, and WebSocket text payloads. Binary WebSocket payloads
    /// contribute only their size and direction.
    ///
    /// Unlike the other filters this one cannot use an index: a substring
    /// that may start anywhere is a scan of the rows the other filters left.
    /// That is the trade a search box makes, and it is why the filters that
    /// *can* narrow by index are applied alongside it rather than replaced
    /// by it.
    pub text: Option<String>,
    /// Whether [`text`](Self::text) is a regular expression rather than a
    /// substring.
    ///
    /// False when omitted, which is what every client sent before the field
    /// existed.
    #[serde(default)]
    pub search_regex: bool,
    /// Whether [`text`](Self::text) distinguishes letter case.
    ///
    /// False when omitted: a search box is case-insensitive unless asked.
    #[serde(default)]
    pub search_case_sensitive: bool,
    /// Only exchanges whose request or response `Content-Type` contains this.
    ///
    /// Compared without regard to case. Matched as a substring of the header
    /// value as it was stored, so `json` finds `application/json`.
    pub content_type: Option<String>,
    /// Only exchanges framed with this HTTP version on either half.
    pub http_version: Option<HttpVersion>,
    /// Only WebSocket handshakes (`101` switching protocols).
    #[serde(default)]
    pub websocket: bool,
    /// Only Server-Sent Event responses.
    #[serde(default)]
    pub sse: bool,
    /// Only exchanges that ended in a proxy error rather than a response.
    #[serde(default)]
    pub failed: bool,
    /// Only exchanges that started at this time or later.
    ///
    /// Compared to the millisecond the exchange started in.
    pub started_at_or_after: Option<Timestamp>,
    /// Only exchanges that started before this time.
    ///
    /// Compared to the millisecond the exchange started in.
    pub started_before: Option<Timestamp>,
}

impl TrafficFilter {
    /// A filter that matches every stored exchange.
    pub fn all() -> Self {
        Self::default()
    }

    /// The same filter, narrowed to `host`.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// The same filter, narrowed to `method`.
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// The same filter, narrowed to responses matching `status`.
    pub fn with_status(mut self, status: StatusFilter) -> Self {
        self.status = Some(status);
        self
    }

    /// The same filter, narrowed to rows containing `text`.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// The same filter, treating the search as a regular expression.
    pub fn with_regex(mut self) -> Self {
        self.search_regex = true;
        self
    }

    /// The same filter, distinguishing letter case in the search.
    pub fn case_sensitive(mut self) -> Self {
        self.search_case_sensitive = true;
        self
    }

    /// The same filter, narrowed to a `Content-Type` containing `content_type`.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// The same filter, narrowed to this HTTP version on either half.
    pub fn with_http_version(mut self, version: HttpVersion) -> Self {
        self.http_version = Some(version);
        self
    }

    /// The same filter, keeping only WebSocket handshakes.
    pub fn websocket_only(mut self) -> Self {
        self.websocket = true;
        self
    }

    /// The same filter, keeping only Server-Sent Event responses.
    pub fn sse_only(mut self) -> Self {
        self.sse = true;
        self
    }

    /// The same filter, keeping only exchanges that ended in a proxy error.
    pub fn failed_only(mut self) -> Self {
        self.failed = true;
        self
    }

    /// The same filter, narrowed to exchanges that started in `from..until`.
    ///
    /// The start is included and the end is not, so two ranges that meet do
    /// not both contain the exchange on the boundary.
    pub fn with_time_range(mut self, from: Option<Timestamp>, until: Option<Timestamp>) -> Self {
        self.started_at_or_after = from;
        self.started_before = until;
        self
    }

    /// Whether this filter narrows anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Which responses a query is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFilter {
    /// One status code exactly: `404`.
    Exact(u16),
    /// A class of status code: `4` for every `4xx`.
    Class(u8),
}

impl StatusFilter {
    /// The inclusive range of status codes this matches.
    ///
    /// A class outside `1..=5` matches nothing, which is what an empty range
    /// says.
    pub(super) fn range(self) -> (u16, u16) {
        match self {
            Self::Exact(status) => (status, status),
            Self::Class(class) if (1..=5).contains(&class) => {
                let first = u16::from(class) * 100;
                (first, first + 99)
            }
            Self::Class(_) => (1, 0),
        }
    }
}

/// Where a page of traffic stopped.
///
/// Names one exchange's place in the newest-first order: the millisecond it
/// started in and its id. Held by a caller between pages and handed back to
/// ask for the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// The millisecond the exchange started in.
    started_at_ms: i64,
    /// Which exchange, for the exchanges that started in that millisecond.
    id: ExchangeId,
}

impl Cursor {
    /// The place `summary` holds in the order.
    pub fn of(summary: &ExchangeSummary) -> Self {
        let (started_at_ms, _) = encoding::timestamp_columns(summary.started_at);

        Self {
            started_at_ms,
            id: summary.id,
        }
    }

    /// The exchange this cursor is the place of.
    pub fn id(&self) -> ExchangeId {
        self.id
    }

    /// The millisecond column this cursor compares against.
    pub(super) fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }
}

/// Which end of the order a page is read from.
///
/// Not a sort: the order is the same one either way — the millisecond an
/// exchange started in, then its id — and this says which end to start at.
/// That is what keeps paging correct, because a [`Cursor`] names a place in
/// that one order and means the same thing read from either end.
///
/// Ordering by anything else — duration, status, host — is deliberately not
/// here. Each would need its own index and its own cursor shape, which is a
/// change to the stored schema rather than to a query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficOrder {
    /// The most recent exchange first. What a traffic list shows by default:
    /// the request a developer just made is the one being looked for.
    #[default]
    Newest,
    /// The earliest exchange first, for reading a session in the order it
    /// happened.
    Oldest,
}

impl TrafficOrder {
    /// The `ORDER BY` direction this reads the order in.
    pub(super) fn direction(self) -> &'static str {
        match self {
            Self::Newest => "DESC",
            Self::Oldest => "ASC",
        }
    }

    /// The comparison that means "after the cursor" in this direction.
    pub(super) fn after_cursor(self) -> &'static str {
        match self {
            Self::Newest => "(started_at_ms < ? OR (started_at_ms = ? AND id < ?))",
            Self::Oldest => "(started_at_ms > ? OR (started_at_ms = ? AND id > ?))",
        }
    }
}

/// A request for a page of traffic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficQuery {
    /// Which exchanges to include.
    pub filter: TrafficFilter,
    /// How many to return, clamped to [`MAX_PAGE_LIMIT`].
    pub limit: usize,
    /// Where the previous page stopped, or `None` for the first page.
    ///
    /// A cursor belongs to the order it was produced in. Paging on with one
    /// after changing [`order`](Self::order) asks for the rows on the other
    /// side of it, which is a different page than the caller meant; a caller
    /// that changes the order starts again without one.
    pub after: Option<Cursor>,
    /// Which end of the order to read from.
    #[serde(default)]
    pub order: TrafficOrder,
}

impl TrafficQuery {
    /// The newest [`DEFAULT_PAGE_LIMIT`] exchanges.
    pub fn newest() -> Self {
        Self {
            filter: TrafficFilter::all(),
            limit: DEFAULT_PAGE_LIMIT,
            after: None,
            order: TrafficOrder::Newest,
        }
    }

    /// The oldest [`DEFAULT_PAGE_LIMIT`] exchanges.
    pub fn oldest() -> Self {
        Self::newest().with_order(TrafficOrder::Oldest)
    }

    /// The same query, read from the other end of the order.
    ///
    /// Drops the cursor: a cursor names a place in the order, and the page
    /// after it is a different page read the other way.
    pub fn with_order(mut self, order: TrafficOrder) -> Self {
        self.order = order;
        self.after = None;
        self
    }

    /// The same query, returning at most `limit` exchanges.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// The same query, narrowed by `filter`.
    pub fn with_filter(mut self, filter: TrafficFilter) -> Self {
        self.filter = filter;
        self
    }

    /// The same query, asking for the page after `cursor`.
    pub fn after(mut self, cursor: Cursor) -> Self {
        self.after = Some(cursor);
        self
    }

    /// How many rows this query may actually return.
    ///
    /// At least one — a page of nothing is not a page — and never more than
    /// [`MAX_PAGE_LIMIT`].
    pub(super) fn effective_limit(&self) -> usize {
        self.limit.clamp(1, MAX_PAGE_LIMIT)
    }
}

impl Default for TrafficQuery {
    fn default() -> Self {
        Self::newest()
    }
}

/// One page of traffic, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficPage {
    /// The exchanges, newest first.
    pub exchanges: Vec<ExchangeSummary>,
    /// Where to carry on from, if there may be more.
    ///
    /// `Some` when the page was filled, which is the only point at which
    /// there might be another one. Asking for the page after a full last
    /// page returns an empty page, which is how the end is reached.
    pub next: Option<Cursor>,
}

impl TrafficPage {
    /// Whether the page holds no exchanges.
    pub fn is_empty(&self) -> bool {
        self.exchanges.is_empty()
    }

    /// How many exchanges the page holds.
    pub fn len(&self) -> usize {
        self.exchanges.len()
    }
}

/// One exchange as a traffic list shows it.
///
/// Everything here is a column of the exchange row, so a page of these is
/// one indexed read: no headers, no bodies, and nothing that has to be
/// fetched per row. The whole exchange — headers, captured bodies — is what
/// [`Storage::get`](super::Storage::get) is for, and is what an inspector
/// asks for when a row is opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeSummary {
    /// Which exchange.
    pub id: ExchangeId,
    /// When the proxy received the request.
    pub started_at: Timestamp,
    /// When the response head arrived, if one did.
    pub response_at: Option<Timestamp>,
    /// How long the whole exchange took, if it finished.
    pub duration_micros: Option<u64>,
    /// The address the client connected from.
    pub client: SocketAddr,
    /// Whether it was plaintext or inside an intercepted TLS session.
    pub scheme: Scheme,
    /// Where it was going, when that was worked out.
    pub destination: Option<Destination>,
    /// The request's method.
    pub method: String,
    /// The request's absolute URL.
    pub url: String,
    /// The version the request was framed with.
    pub version: HttpVersion,
    /// The status the origin answered with, if it answered.
    pub status: Option<u16>,
    /// The reason phrase that came with it.
    pub reason: Option<String>,
    /// The version the response was framed with.
    pub response_version: Option<HttpVersion>,
    /// The request `Content-Type`, when the message declared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_content_type: Option<String>,
    /// The response `Content-Type`, when the message declared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_content_type: Option<String>,
    /// Whether the exchange ran to completion.
    pub completed: bool,
    /// Why there is no response, when there is none.
    pub error: Option<TrafficError>,
}

impl ExchangeSummary {
    /// The summary of an exchange that has not been through the database.
    ///
    /// The same summary the traffic list would show for it once it is
    /// stored, built from the exchange itself. It exists for the moment
    /// before storage has it: a finished exchange is published to
    /// subscribers as it happens, and a subscriber that wants to show it in
    /// a traffic list wants exactly this — the row, not the whole exchange
    /// with its captured bodies.
    pub fn of(exchange: &TrafficExchange) -> Self {
        let timing = &exchange.timing;
        let response = exchange.response.as_ref();

        Self {
            id: exchange.id,
            started_at: timing.started_at,
            // Worked out the same way the stored column is, so a summary
            // built here and one read back later are the same summary.
            response_at: encoding::response_at_ms(
                timing.started_at,
                timing.upstream_response_micros,
            )
            .map(|millis| encoding::timestamp_from_columns(millis, 0)),
            duration_micros: timing.completed_micros,
            client: exchange.connection.client,
            scheme: exchange.connection.scheme,
            destination: exchange.connection.destination.clone(),
            method: exchange.request.method.clone(),
            url: exchange.request.url.clone(),
            version: exchange.request.version,
            status: response.map(|response| response.status),
            reason: response.and_then(|response| response.reason.clone()),
            response_version: response.map(|response| response.version),
            request_content_type: exchange
                .request
                .headers
                .text("content-type")
                .map(str::to_owned),
            response_content_type: response
                .and_then(|response| response.headers.text("content-type"))
                .map(str::to_owned),
            completed: timing.completed_micros.is_some(),
            error: exchange.error.clone(),
        }
    }

    /// Whether a response came back and nothing went wrong.
    pub fn succeeded(&self) -> bool {
        self.status.is_some() && self.error.is_none()
    }

    /// How long the exchange took, in milliseconds, if it finished.
    pub fn duration_ms(&self) -> Option<f64> {
        self.duration_micros.map(|micros| micros as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(started_at: Timestamp) -> ExchangeSummary {
        ExchangeSummary {
            id: ExchangeId::next(),
            started_at,
            response_at: None,
            duration_micros: Some(3_500),
            client: "127.0.0.1:54321".parse().expect("a valid address"),
            scheme: Scheme::Http,
            destination: Some(Destination::new("example.com", 80)),
            method: "GET".to_owned(),
            url: "http://example.com/x".to_owned(),
            version: HttpVersion::Http11,
            status: Some(200),
            reason: Some("OK".to_owned()),
            response_version: Some(HttpVersion::Http11),
            request_content_type: None,
            response_content_type: None,
            completed: true,
            error: None,
        }
    }

    #[test]
    fn an_empty_filter_narrows_nothing() {
        assert!(TrafficFilter::all().is_empty());
        assert!(!TrafficFilter::all().with_method("GET").is_empty());
    }

    #[test]
    fn a_filter_is_built_up_a_field_at_a_time() {
        let filter = TrafficFilter::all()
            .with_host("api.example.com")
            .with_method("POST")
            .with_status(StatusFilter::Class(4));

        assert_eq!(filter.host.as_deref(), Some("api.example.com"));
        assert_eq!(filter.method.as_deref(), Some("POST"));
        assert_eq!(filter.status, Some(StatusFilter::Class(4)));
    }

    #[test]
    fn a_status_class_is_the_hundred_codes_it_names() {
        assert_eq!(StatusFilter::Class(4).range(), (400, 499));
        assert_eq!(StatusFilter::Class(2).range(), (200, 299));
        assert_eq!(StatusFilter::Exact(404).range(), (404, 404));
    }

    #[test]
    fn a_status_class_that_is_not_a_class_matches_nothing() {
        let (first, last) = StatusFilter::Class(9).range();

        assert!(first > last, "{first}..={last} matches something");
    }

    #[test]
    fn a_limit_is_kept_inside_what_a_page_can_be() {
        assert_eq!(TrafficQuery::newest().effective_limit(), DEFAULT_PAGE_LIMIT);
        assert_eq!(TrafficQuery::newest().with_limit(0).effective_limit(), 1);
        assert_eq!(
            TrafficQuery::newest()
                .with_limit(usize::MAX)
                .effective_limit(),
            MAX_PAGE_LIMIT
        );
        assert_eq!(TrafficQuery::newest().with_limit(7).effective_limit(), 7);
    }

    #[test]
    fn a_cursor_is_the_place_a_summary_holds_in_the_order() {
        let summary = summary(Timestamp {
            seconds: 100,
            nanos: 500_000_000,
        });

        let cursor = Cursor::of(&summary);

        assert_eq!(cursor.id(), summary.id);
        assert_eq!(cursor.started_at_ms(), 100_500);
    }

    #[test]
    fn a_query_round_trips_through_json() {
        let query = TrafficQuery::newest()
            .with_limit(25)
            .with_filter(
                TrafficFilter::all()
                    .with_host("example.com")
                    .with_status(StatusFilter::Exact(404)),
            )
            .after(Cursor::of(&summary(Timestamp::now())));

        let json = serde_json::to_string(&query).expect("serializes");
        let back: TrafficQuery = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, query);
    }

    #[test]
    fn a_page_round_trips_through_json() {
        let page = TrafficPage {
            exchanges: vec![summary(Timestamp::now())],
            next: None,
        };

        let json = serde_json::to_string(&page).expect("serializes");
        let back: TrafficPage = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, page);
        assert_eq!(back.len(), 1);
        assert!(!back.is_empty());
    }

    #[test]
    fn a_summary_says_whether_the_exchange_worked() {
        let mut summary = summary(Timestamp::now());
        assert!(summary.succeeded());
        assert_eq!(summary.duration_ms(), Some(3.5));

        summary.status = None;
        summary.error = Some(TrafficError::of(
            crate::error::TrafficErrorKind::UpstreamConnect,
        ));

        assert!(!summary.succeeded());
    }
}
