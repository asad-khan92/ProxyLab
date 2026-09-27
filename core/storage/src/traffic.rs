//! The canonical record of one HTTP exchange.
//!
//! ```text
//! TrafficExchange
//! ├── id
//! ├── connection   client address, scheme, destination
//! ├── request      method, URL, version, headers, body
//! ├── response     status, reason, version, headers, body
//! ├── error        why there is no response, when there is none
//! └── timing       when it started and how long it took
//! ```
//!
//! One structure serves both routes through the proxy. A plaintext request
//! forwarded to an origin and an HTTPS request read out of an intercepted TLS
//! session differ in [`ConnectionInfo::scheme`] and in nothing else, so
//! anything reading captured traffic reads one shape.
//!
//! The connection details sit on the exchange rather than on the request
//! because they describe the pair: the client address and the destination are
//! as much the response's as the request's, and duplicating them would leave
//! two places for them to disagree.

use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::body::BodyCapture;
use crate::error::TrafficError;
use crate::headers::Headers;
use crate::timing::Timing;

/// Identifies one exchange, for as long as the exchange is kept.
///
/// A UUID, version 7: 48 bits of millisecond timestamp followed by a counter
/// and random bits. Two properties matter here.
///
/// It is unique across runs of the proxy. An exchange stored today and an
/// exchange stored after a restart cannot collide, which a counter that
/// starts again at one could not promise — and a persisted exchange has to
/// keep its identity for as long as the database does.
///
/// It is ordered. Ids minted in this process increase, so a larger id was
/// started later; across processes they are ordered by the clock they were
/// minted against, to the millisecond. That makes an id a stable tie-break
/// for the newest-first ordering the traffic list is built on, rather than
/// something a query has to fall back to an arbitrary order on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExchangeId(Uuid);

impl ExchangeId {
    /// A new id, for an exchange starting now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }

    /// The id as the UUID it is.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }

    /// The id in the form it is written down as: hyphenated and lowercase.
    ///
    /// This is what is stored and what a UI shows; two ids in this form sort
    /// the same way the ids themselves do.
    pub fn as_text(self) -> String {
        self.to_string()
    }
}

impl From<Uuid> for ExchangeId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

/// Reads an id back from the form [`ExchangeId::as_text`] writes.
impl FromStr for ExchangeId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for ExchangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// How the client reached the proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    /// A plaintext request, forwarded to the origin.
    Http,
    /// A request read out of an intercepted TLS session.
    Https,
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http => "http",
            Self::Https => "https",
        })
    }
}

/// The server an exchange was for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    /// The host, without the brackets an IPv6 literal wears in a URL.
    pub host: String,
    /// The port the proxy connected to.
    pub port: u16,
}

impl Destination {
    /// The destination `host` and `port` name.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// The connection an exchange happened on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// The address the client connected from.
    pub client: SocketAddr,
    /// Whether the exchange was plaintext or inside a TLS session.
    pub scheme: Scheme,
    /// Where the proxy sent it, when that was worked out.
    ///
    /// `None` for a request the proxy refused before it had a destination —
    /// a request target that named no host, for instance.
    pub destination: Option<Destination>,
}

impl ConnectionInfo {
    /// A plaintext connection from `client`.
    pub fn http(client: SocketAddr, destination: Option<Destination>) -> Self {
        Self {
            client,
            scheme: Scheme::Http,
            destination,
        }
    }

    /// An intercepted TLS connection from `client`.
    pub fn https(client: SocketAddr, destination: Option<Destination>) -> Self {
        Self {
            client,
            scheme: Scheme::Https,
            destination,
        }
    }
}

/// The HTTP version a message was framed with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpVersion {
    /// HTTP/0.9.
    #[serde(rename = "HTTP/0.9")]
    Http09,
    /// HTTP/1.0.
    #[serde(rename = "HTTP/1.0")]
    Http10,
    /// HTTP/1.1, which is what ProxyLab speaks.
    #[default]
    #[serde(rename = "HTTP/1.1")]
    Http11,
    /// HTTP/2.
    #[serde(rename = "HTTP/2.0")]
    Http2,
    /// HTTP/3.
    #[serde(rename = "HTTP/3.0")]
    Http3,
    /// A version this build does not have a name for.
    #[serde(rename = "unknown")]
    Unknown,
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http09 => "HTTP/0.9",
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
            Self::Http2 => "HTTP/2.0",
            Self::Http3 => "HTTP/3.0",
            Self::Unknown => "unknown",
        })
    }
}

/// The request half of an exchange.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedRequest {
    /// The method, as it was sent.
    pub method: String,
    /// The absolute URL the request was for.
    ///
    /// Assembled by the proxy rather than copied from the request line, so
    /// that an intercepted HTTPS request — which travels in origin form
    /// inside its session — still names the host it was sent to, and so that
    /// credentials a client put in a URL authority never appear here.
    pub url: String,
    /// The version the request was framed with.
    pub version: HttpVersion,
    /// The headers, in the order they arrived.
    pub headers: Headers,
    /// As much of the body as the capture limit allowed.
    pub body: BodyCapture,
}

impl CapturedRequest {
    /// A request with no headers and no body yet.
    pub fn new(method: impl Into<String>, url: impl Into<String>, version: HttpVersion) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            version,
            headers: Headers::new(),
            body: BodyCapture::empty(),
        }
    }
}

/// Metadata only, never a header value or a body.
impl fmt::Debug for CapturedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("version", &self.version)
            .field("headers", &self.headers.len())
            .field("body", &self.body)
            .finish()
    }
}

/// The response half of an exchange.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedResponse {
    /// The status code the origin answered with.
    pub status: u16,
    /// The reason phrase, when there is one to give.
    ///
    /// HTTP/1.1 carries a reason phrase on the wire but almost nothing keeps
    /// it, so this is usually the standard phrase for the status code, and
    /// `None` for a status code that has no standard phrase.
    pub reason: Option<String>,
    /// The version the response was framed with.
    pub version: HttpVersion,
    /// The headers, in the order they arrived.
    pub headers: Headers,
    /// As much of the body as the capture limit allowed.
    pub body: BodyCapture,
}

impl CapturedResponse {
    /// A response with no headers and no body yet.
    pub fn new(status: u16, version: HttpVersion) -> Self {
        Self {
            status,
            reason: None,
            version,
            headers: Headers::new(),
            body: BodyCapture::empty(),
        }
    }
}

/// Metadata only, never a header value or a body.
impl fmt::Debug for CapturedResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedResponse")
            .field("status", &self.status)
            .field("reason", &self.reason)
            .field("version", &self.version)
            .field("headers", &self.headers.len())
            .field("body", &self.body)
            .finish()
    }
}

/// One complete request/response lifecycle, as the proxy saw it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficExchange {
    /// Identifies this exchange within the run.
    pub id: ExchangeId,
    /// The connection it happened on.
    pub connection: ConnectionInfo,
    /// What the client asked for.
    pub request: CapturedRequest,
    /// What came back, if anything did.
    pub response: Option<CapturedResponse>,
    /// Why nothing came back, when nothing did.
    ///
    /// A transport failure is never turned into a status code: an exchange
    /// with no response says so, and says why here.
    pub error: Option<TrafficError>,
    /// When it started and how long its parts took.
    pub timing: Timing,
}

impl TrafficExchange {
    /// An exchange that has a request and nothing else yet.
    pub fn new(id: ExchangeId, connection: ConnectionInfo, request: CapturedRequest) -> Self {
        Self {
            id,
            connection,
            request,
            response: None,
            error: None,
            timing: Timing::default(),
        }
    }

    /// Whether a response came back and nothing went wrong.
    pub fn succeeded(&self) -> bool {
        self.response.is_some() && self.error.is_none()
    }

    /// The status the origin answered with, if it answered.
    pub fn status(&self) -> Option<u16> {
        self.response.as_ref().map(|response| response.status)
    }

    /// How long the exchange took, in milliseconds, if it finished.
    pub fn duration_ms(&self) -> Option<f64> {
        self.timing.duration_ms()
    }

    /// The one-line form that is safe to write to a log.
    ///
    /// Method, URL, and outcome: the same metadata the proxy already reports
    /// as it works, and no header, body, or captured byte.
    pub fn summary(&self) -> String {
        let outcome = match (&self.response, &self.error) {
            (Some(response), _) => response.status.to_string(),
            (None, Some(error)) => format!("failed: {error}"),
            (None, None) => "no response".to_owned(),
        };
        let duration = match self.duration_ms() {
            Some(ms) => format!(" in {ms:.1} ms"),
            None => String::new(),
        };

        format!(
            "{} {} {} -> {outcome}{duration}",
            self.id, self.request.method, self.request.url
        )
    }
}

/// The safe summary, and the counts and sizes behind it. Never a header value
/// and never a body: an exchange is not something to write to a log whole,
/// and this makes sure that writing one anyway costs nothing.
impl fmt::Debug for TrafficExchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrafficExchange")
            .field("id", &self.id)
            .field("connection", &self.connection)
            .field("request", &self.request)
            .field("response", &self.response)
            .field("error", &self.error)
            .field("timing", &self.timing)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::error::TrafficErrorKind;
    use crate::headers::HeaderValue;
    use crate::timing::Timestamp;

    /// A fixed id, so the tests that assert on rendered text have something
    /// to assert against.
    const ID: &str = "0199c0de-0000-7000-8000-00000000002a";

    fn id() -> ExchangeId {
        ID.parse().expect("a valid exchange id")
    }

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    fn request() -> CapturedRequest {
        let mut request = CapturedRequest::new(
            "POST",
            "http://example.com/api/users?active=true",
            HttpVersion::Http11,
        );
        request
            .headers
            .push("Content-Type", HeaderValue::Text("application/json".into()));
        request.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        request.body = BodyCapture::complete(br#"{"name":"ada"}"#.to_vec());

        request
    }

    fn response() -> CapturedResponse {
        let mut response = CapturedResponse::new(201, HttpVersion::Http11);
        response.reason = Some("Created".to_owned());
        response
            .headers
            .push("Set-Cookie", HeaderValue::Text("session=hunter2".into()));
        response.body = BodyCapture::complete(br#"{"id":1}"#.to_vec());

        response
    }

    fn exchange() -> TrafficExchange {
        let mut exchange = TrafficExchange::new(
            id(),
            ConnectionInfo::http(client(), Some(Destination::new("example.com", 80))),
            request(),
        );
        exchange.response = Some(response());
        exchange.timing = Timing {
            started_at: Timestamp {
                seconds: 1_700_000_000,
                nanos: 500,
            },
            upstream_connect_micros: Some(100),
            upstream_response_micros: Some(2_000),
            completed_micros: Some(3_500),
        };

        exchange
    }

    #[test]
    fn ids_are_unique_and_increasing() {
        let first = ExchangeId::next();
        let second = ExchangeId::next();

        assert_ne!(first, second);
        assert!(second > first, "{first} is not before {second}");
    }

    #[test]
    fn ids_minted_in_a_burst_are_all_different() {
        let ids: std::collections::BTreeSet<ExchangeId> =
            (0..1_000).map(|_| ExchangeId::next()).collect();

        assert_eq!(ids.len(), 1_000);
    }

    #[test]
    fn an_id_reads_as_a_hyphenated_uuid() {
        assert_eq!(id().to_string(), ID);
        assert_eq!(id().as_text(), ID);
    }

    #[test]
    fn an_id_round_trips_through_its_text_form() {
        let id = ExchangeId::next();

        assert_eq!(id.as_text().parse::<ExchangeId>(), Ok(id));
    }

    #[test]
    fn text_that_is_not_an_id_is_refused() {
        assert!("not-an-id".parse::<ExchangeId>().is_err());
        assert!("".parse::<ExchangeId>().is_err());
    }

    #[test]
    fn an_id_is_a_version_7_uuid() {
        // Version 7 is what makes an id ordered by the time it was minted,
        // which the newest-first traffic query relies on.
        assert_eq!(ExchangeId::next().as_uuid().get_version_num(), 7);
    }

    #[test]
    fn ids_sort_the_same_way_their_text_forms_do() {
        let mut ids: Vec<ExchangeId> = (0..64).map(|_| ExchangeId::next()).collect();
        let mut by_text: Vec<ExchangeId> = ids.clone();

        ids.sort();
        by_text.sort_by_key(|id| id.as_text());

        assert_eq!(ids, by_text);
    }

    #[test]
    fn a_request_carries_its_method_url_headers_and_body() {
        let request = request();

        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "http://example.com/api/users?active=true");
        assert_eq!(request.version, HttpVersion::Http11);
        assert_eq!(
            request.headers.text("content-type"),
            Some("application/json")
        );
        assert_eq!(request.body.as_text(), Some(r#"{"name":"ada"}"#));
    }

    #[test]
    fn a_response_carries_its_status_reason_headers_and_body() {
        let response = response();

        assert_eq!(response.status, 201);
        assert_eq!(response.reason.as_deref(), Some("Created"));
        assert_eq!(response.headers.text("set-cookie"), Some("session=hunter2"));
        assert_eq!(response.body.as_text(), Some(r#"{"id":1}"#));
    }

    #[test]
    fn a_complete_exchange_has_both_halves_and_no_error() {
        let exchange = exchange();

        assert!(exchange.succeeded());
        assert_eq!(exchange.status(), Some(201));
        assert_eq!(exchange.duration_ms(), Some(3.5));
        assert_eq!(exchange.connection.scheme, Scheme::Http);
        assert_eq!(
            exchange.connection.destination,
            Some(Destination::new("example.com", 80))
        );
    }

    #[test]
    fn a_failed_exchange_has_an_error_and_no_response() {
        let mut exchange = exchange();
        exchange.response = None;
        exchange.error = Some(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        assert!(!exchange.succeeded());
        assert_eq!(exchange.status(), None);
        assert_eq!(
            exchange.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::UpstreamConnect)
        );
    }

    #[test]
    fn an_exchange_round_trips_through_json() {
        let exchange = exchange();

        let json = serde_json::to_string(&exchange).expect("serializes");
        let back: TrafficExchange = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, exchange);
    }

    #[test]
    fn a_failed_exchange_round_trips_through_json() {
        let mut exchange = exchange();
        exchange.response = None;
        exchange.error = Some(TrafficError::new(
            TrafficErrorKind::ClientTls,
            "the client refused the proxy's certificate",
        ));

        let json = serde_json::to_string(&exchange).expect("serializes");
        let back: TrafficExchange = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, exchange);
    }

    #[test]
    fn an_exchange_with_a_binary_body_round_trips_through_json() {
        let mut exchange = exchange();
        let bytes: Vec<u8> = (0..=255).collect();
        exchange.request.body = BodyCapture::complete(bytes.clone());
        exchange.response.as_mut().expect("a response").body =
            BodyCapture::truncated(bytes.clone(), 4_096);

        let json = serde_json::to_string(&exchange).expect("serializes");
        let back: TrafficExchange = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, exchange);
        assert_eq!(back.request.body.data(), bytes);
        let response_body = &back.response.expect("a response").body;
        assert!(response_body.is_truncated());
        assert_eq!(response_body.size(), 4_096);
    }

    #[test]
    fn an_exchange_with_no_bodies_round_trips_through_json() {
        let mut exchange = exchange();
        exchange.request.body = BodyCapture::empty();
        exchange.response.as_mut().expect("a response").body = BodyCapture::empty();

        let json = serde_json::to_string(&exchange).expect("serializes");
        let back: TrafficExchange = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, exchange);
        assert!(back.request.body.is_empty());
    }

    #[test]
    fn json_names_the_parts_a_reader_would_look_for() {
        let json = serde_json::to_value(exchange()).expect("serializes");

        assert_eq!(json["id"], ID);
        assert_eq!(json["request"]["method"], "POST");
        assert_eq!(json["request"]["version"], "HTTP/1.1");
        assert_eq!(json["response"]["status"], 201);
        assert_eq!(json["connection"]["client"], "127.0.0.1:54321");
        assert_eq!(json["connection"]["scheme"], "http");
        assert_eq!(json["connection"]["destination"]["host"], "example.com");
        assert_eq!(json["error"], serde_json::Value::Null);
    }

    #[test]
    fn every_version_round_trips_through_json() {
        for version in [
            HttpVersion::Http09,
            HttpVersion::Http10,
            HttpVersion::Http11,
            HttpVersion::Http2,
            HttpVersion::Http3,
            HttpVersion::Unknown,
        ] {
            let json = serde_json::to_string(&version).expect("serializes");
            let back: HttpVersion = serde_json::from_str(&json).expect("deserializes");

            assert_eq!(back, version);
            assert_eq!(json.trim_matches('"'), version.to_string());
        }
    }

    #[test]
    fn a_destination_reads_as_a_host_and_port() {
        assert_eq!(
            Destination::new("example.com", 443).to_string(),
            "example.com:443"
        );
        assert_eq!(Destination::new("::1", 8443).to_string(), "[::1]:8443");
    }

    #[test]
    fn the_summary_names_the_exchange_without_its_contents() {
        let summary = exchange().summary();

        assert_eq!(
            summary,
            format!("{ID} POST http://example.com/api/users?active=true -> 201 in 3.5 ms")
        );
        assert!(!summary.contains("sk-secret"), "{summary}");
        assert!(!summary.contains("hunter2"), "{summary}");
    }

    #[test]
    fn the_summary_of_a_failure_says_what_failed() {
        let mut exchange = exchange();
        exchange.response = None;
        exchange.timing.completed_micros = None;
        exchange.error = Some(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        assert_eq!(
            exchange.summary(),
            format!(
                "{ID} POST http://example.com/api/users?active=true -> failed: \
                 could not connect to the destination server"
            )
        );
    }

    #[test]
    fn debug_never_renders_a_header_value_or_a_body() {
        let rendered = format!("{:?}", exchange());

        assert!(!rendered.contains("sk-secret"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("ada"), "{rendered}");
        assert!(!rendered.contains(r#"{"id":1}"#), "{rendered}");
        assert!(rendered.contains("status: 201"), "{rendered}");
    }
}
