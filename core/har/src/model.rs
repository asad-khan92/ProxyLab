//! The HAR 1.2 document, as the specification spells it.
//!
//! ```text
//! log
//! ├── version   "1.2"
//! ├── creator   who wrote the file
//! └── entries
//!     └── entry
//!         ├── startedDateTime
//!         ├── time
//!         ├── request   method, url, httpVersion, headers, queryString, postData
//!         ├── response  status, statusText, headers, content
//!         ├── cache
//!         └── timings   send, wait, receive
//! ```
//!
//! One set of types serves both directions: what is written is what is read,
//! so a field that exports and a field that imports cannot disagree. The
//! names are HAR's own — `startedDateTime`, `httpVersion`, `redirectURL` —
//! because the file is read by other tools and this crate does not get to
//! choose them.
//!
//! # Writing is strict, reading is not
//!
//! Every field HAR 1.2 requires is written, with `-1` where a value is not
//! known, so the output is a document another tool will accept. Reading is
//! the opposite: every field has a default, unknown fields are ignored rather
//! than refused, and an entry missing something optional is still an entry.
//! A HAR file this crate did not write is the usual case, and a tool that
//! spells `cache` as `{}` or leaves `queryString` out should not cost a
//! developer the import.
//!
//! # What is deliberately empty
//!
//! `cookies` is written as an empty list. HAR wants cookies parsed out into
//! name, value, path, domain, expiry and flags; ProxyLab captured the
//! `Cookie` and `Set-Cookie` headers, which are in `headers` where they can
//! be read exactly as they were sent. Parsing them into a second
//! representation would be a cookie parser whose bugs would be silent, for a
//! field nothing here reads back.

use serde::{Deserialize, Serialize};

/// The HAR version this crate writes and reads.
pub const HAR_VERSION: &str = "1.2";

/// A whole HAR file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Har {
    /// The one thing a HAR file contains.
    pub log: Log,
}

/// The log a HAR file is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Log {
    /// The HAR version the file claims to be.
    #[serde(default)]
    pub version: String,
    /// What wrote it.
    #[serde(default)]
    pub creator: Creator,
    /// The exchanges.
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Anything the writer had to say about the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// The application that wrote a HAR file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Creator {
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Its version.
    #[serde(default)]
    pub version: String,
}

/// One exchange.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// When the request was made, as ISO 8601.
    #[serde(default)]
    pub started_date_time: String,
    /// How long the whole exchange took, in milliseconds, or `-1`.
    #[serde(default = "unknown_duration")]
    pub time: f64,
    /// What was asked for.
    #[serde(default)]
    pub request: Request,
    /// What came back.
    #[serde(default)]
    pub response: Response,
    /// Required by HAR, and always empty here: ProxyLab is a proxy and has no
    /// browser cache to report.
    #[serde(default)]
    pub cache: Cache,
    /// How long the parts of it took.
    #[serde(default)]
    pub timings: Timings,
    /// Why there is no response, when there is none.
    ///
    /// HAR has no way to say "the request was made and nothing answered", so
    /// an exchange that failed is written with a status of zero and the
    /// reason here. The field is HAR's own `comment`, which every object may
    /// carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// The request half of an entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The method.
    #[serde(default)]
    pub method: String,
    /// The absolute URL.
    #[serde(default)]
    pub url: String,
    /// The HTTP version it was framed with.
    #[serde(default)]
    pub http_version: String,
    /// Required by HAR, and always empty here. See the module documentation.
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// The headers, in the order they were sent.
    #[serde(default)]
    pub headers: Vec<NameValue>,
    /// The query, read out of the URL.
    #[serde(default)]
    pub query_string: Vec<NameValue>,
    /// The body, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_data: Option<PostData>,
    /// How many bytes the headers were, or `-1` when that was not measured.
    #[serde(default = "unknown_size")]
    pub headers_size: i64,
    /// How many bytes the body was, or `-1`.
    #[serde(default = "unknown_size")]
    pub body_size: i64,
}

/// The response half of an entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// The status code, or zero when nothing answered.
    #[serde(default)]
    pub status: u16,
    /// The reason phrase.
    #[serde(default)]
    pub status_text: String,
    /// The HTTP version it was framed with.
    #[serde(default)]
    pub http_version: String,
    /// Required by HAR, and always empty here. See the module documentation.
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// The headers, in the order they arrived.
    #[serde(default)]
    pub headers: Vec<NameValue>,
    /// The body.
    #[serde(default)]
    pub content: Content,
    /// Where a redirect pointed, or an empty string.
    ///
    /// Spelled out rather than left to the camel-case rule: HAR writes this
    /// one `redirectURL`, and a reader looking for that field would not find
    /// a `redirectUrl`.
    #[serde(default, rename = "redirectURL")]
    pub redirect_url: String,
    /// How many bytes the headers were, or `-1`.
    #[serde(default = "unknown_size")]
    pub headers_size: i64,
    /// How many bytes the body was, or `-1`.
    #[serde(default = "unknown_size")]
    pub body_size: i64,
}

/// A size and a duration nobody measured are `-1` whichever way an object is
/// built, so an entry assembled field by field says "not measured" rather
/// than "zero" about everything it did not fill in.
impl Default for Entry {
    fn default() -> Self {
        Self {
            started_date_time: String::new(),
            time: unknown_duration(),
            request: Request::default(),
            response: Response::default(),
            cache: Cache {},
            timings: Timings::default(),
            comment: None,
        }
    }
}

impl Default for Request {
    fn default() -> Self {
        Self {
            method: String::new(),
            url: String::new(),
            http_version: String::new(),
            cookies: Vec::new(),
            headers: Vec::new(),
            query_string: Vec::new(),
            post_data: None,
            headers_size: unknown_size(),
            body_size: unknown_size(),
        }
    }
}

impl Default for Response {
    fn default() -> Self {
        Self {
            status: 0,
            status_text: String::new(),
            http_version: String::new(),
            cookies: Vec::new(),
            headers: Vec::new(),
            content: Content::default(),
            redirect_url: String::new(),
            headers_size: unknown_size(),
            body_size: unknown_size(),
        }
    }
}

/// A response body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    /// How long the body was, in bytes.
    ///
    /// The length of the whole body, which is larger than what `text` holds
    /// when the capture was cut short at ProxyLab's own limit.
    #[serde(default)]
    pub size: i64,
    /// What the body is.
    #[serde(default)]
    pub mime_type: String,
    /// The body itself, or nothing when none was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `base64` when `text` is base64 rather than the body's own characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Anything worth saying about the body — that it was truncated, for
    /// instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// A request body.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostData {
    /// What the body is.
    #[serde(default)]
    pub mime_type: String,
    /// Required by HAR for a form body, and always empty here: the body is
    /// carried whole in `text`, exactly as it was sent.
    #[serde(default)]
    pub params: Vec<PostParam>,
    /// The body itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `base64` when `text` is base64 rather than the body's own characters.
    ///
    /// HAR 1.2 names this field on a response's `content` and not on
    /// `postData`, which leaves a request body that is not text with no
    /// spelling at all. Chrome's own exports carry it here, so this is what
    /// other tools already read; a request body that *is* text is written as
    /// text and carries no encoding, so the common case is untouched by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// Anything worth saying about the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// One name and value: a header, or a query parameter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NameValue {
    /// The name.
    #[serde(default)]
    pub name: String,
    /// The value.
    #[serde(default)]
    pub value: String,
    /// Anything worth saying about it — that the value is not text, for
    /// instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl NameValue {
    /// A name and a value, with nothing to say about them.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            comment: None,
        }
    }
}

/// One parameter of a form body. Never written by this crate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PostParam {
    /// The name.
    #[serde(default)]
    pub name: String,
    /// The value.
    #[serde(default)]
    pub value: String,
}

/// One cookie. Never written by this crate — see the module documentation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    /// The name.
    #[serde(default)]
    pub name: String,
    /// The value.
    #[serde(default)]
    pub value: String,
}

/// The browser cache, which a proxy has none of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {}

/// How long the parts of an exchange took, in milliseconds.
///
/// `-1` means "not measured", which is what HAR says it means. ProxyLab
/// measures three of the seven: how long a connection took to get hold of,
/// how long the origin took to start answering, and how long the body took
/// to arrive. Blocked, DNS and SSL are not separated out anywhere in the
/// proxy, so they are not invented here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// How long the request waited before anything happened to it.
    #[serde(default = "unknown_duration")]
    pub blocked: f64,
    /// How long the name took to resolve.
    #[serde(default = "unknown_duration")]
    pub dns: f64,
    /// How long the connection took to establish or to get hold of.
    #[serde(default = "unknown_duration")]
    pub connect: f64,
    /// How long the request took to send.
    #[serde(default)]
    pub send: f64,
    /// How long the origin took to start answering.
    #[serde(default)]
    pub wait: f64,
    /// How long the response body took to arrive.
    #[serde(default)]
    pub receive: f64,
    /// How long the TLS handshake took.
    #[serde(default = "unknown_duration")]
    pub ssl: f64,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            blocked: -1.0,
            dns: -1.0,
            connect: -1.0,
            send: 0.0,
            wait: 0.0,
            receive: 0.0,
            ssl: -1.0,
        }
    }
}

/// What HAR writes for a duration nobody measured.
fn unknown_duration() -> f64 {
    -1.0
}

/// What HAR writes for a size nobody measured.
fn unknown_size() -> i64 {
    -1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_names_are_the_ones_har_uses() {
        let entry = Entry::default();
        let json = serde_json::to_value(&entry).expect("serializes");

        for field in [
            "startedDateTime",
            "time",
            "request",
            "response",
            "cache",
            "timings",
        ] {
            assert!(json.get(field).is_some(), "{field} is missing from {json}");
        }
        assert!(json["request"].get("httpVersion").is_some(), "{json}");
        assert!(json["request"].get("queryString").is_some(), "{json}");
        assert!(json["request"].get("headersSize").is_some(), "{json}");
        assert!(json["response"].get("statusText").is_some(), "{json}");
        assert!(json["response"].get("redirectURL").is_some(), "{json}");
        assert_eq!(json["request"]["headersSize"], -1);
        assert_eq!(json["time"], -1.0);
        assert!(
            json["response"]["content"].get("mimeType").is_some(),
            "{json}"
        );
    }

    #[test]
    fn a_document_with_only_its_required_fields_is_read() {
        // What a terse writer produces: no cookies, no queryString, no cache,
        // and fields this build has never heard of.
        let json = r#"{
            "log": {
                "version": "1.2",
                "creator": {"name": "something", "version": "9"},
                "pages": [],
                "entries": [{
                    "startedDateTime": "2026-09-20T10:11:12.345Z",
                    "time": 12.0,
                    "_resourceType": "xhr",
                    "request": {"method": "GET", "url": "https://example.com/", "headers": []},
                    "response": {"status": 200, "content": {"size": 0, "mimeType": ""}},
                    "timings": {"send": 0, "wait": 12, "receive": 0}
                }]
            }
        }"#;

        let har: Har = serde_json::from_str(json).expect("reads");

        assert_eq!(har.log.entries.len(), 1);
        assert_eq!(har.log.entries[0].request.method, "GET");
        assert_eq!(har.log.entries[0].response.status, 200);
        assert_eq!(har.log.entries[0].request.headers_size, -1);
        assert_eq!(har.log.entries[0].timings.connect, -1.0);
        assert!(har.log.entries[0].request.cookies.is_empty());
    }

    #[test]
    fn an_unmeasured_timing_is_minus_one() {
        let json = serde_json::to_value(Timings::default()).expect("serializes");

        assert_eq!(json["blocked"], -1.0);
        assert_eq!(json["dns"], -1.0);
        assert_eq!(json["connect"], -1.0);
        assert_eq!(json["ssl"], -1.0);
        assert_eq!(json["send"], 0.0);
    }

    #[test]
    fn a_document_round_trips_through_json() {
        let har = Har {
            log: Log {
                version: HAR_VERSION.to_owned(),
                creator: Creator {
                    name: "ProxyLab".to_owned(),
                    version: "0.1.0".to_owned(),
                },
                entries: vec![Entry {
                    started_date_time: "2026-09-20T10:11:12.345Z".to_owned(),
                    time: 3.5,
                    request: Request {
                        method: "POST".to_owned(),
                        url: "https://example.com/api?a=b".to_owned(),
                        http_version: "HTTP/1.1".to_owned(),
                        headers: vec![NameValue::new("Content-Type", "application/json")],
                        query_string: vec![NameValue::new("a", "b")],
                        post_data: Some(PostData {
                            mime_type: "application/json".to_owned(),
                            text: Some("{}".to_owned()),
                            ..PostData::default()
                        }),
                        body_size: 2,
                        ..Request::default()
                    },
                    ..Entry::default()
                }],
                comment: None,
            },
        };

        let text = serde_json::to_string(&har).expect("serializes");
        let back: Har = serde_json::from_str(&text).expect("deserializes");

        assert_eq!(back, har);
    }
}
