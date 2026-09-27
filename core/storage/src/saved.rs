//! A request somebody kept, so they can send it again tomorrow.
//!
//! ```text
//! SavedRequest
//! ├── id          a UUIDv7, durable and ordered by when it was minted
//! ├── name        what a person calls it
//! ├── method      the method it will be sent with
//! ├── url         where it will be sent
//! ├── headers     ordered, repeatable, binary-safe
//! ├── body        as much of one as there is
//! └── timestamps  when it was saved, and last changed
//! ```
//!
//! Only what is needed to build the request again. A saved request is not a
//! captured exchange: there is no response, no timing, no connection, and no
//! record of what happened when it was last sent — what happened is an
//! exchange in the traffic list, with an id of its own, like every other
//! request the proxy has carried.
//!
//! # Why the id is a UUIDv7
//!
//! The same two reasons [`ExchangeId`](crate::ExchangeId) and `RuleId` are
//! ones: it is unique across runs, so a saved request keeps its identity
//! across restarts, and it is ordered by the moment it was minted, so "the
//! order they were saved in" is a tie-break the database can answer with
//! `ORDER BY id` rather than a sequence column that could disagree with it.
//!
//! # The body may be less than the request sent
//!
//! A request saved out of captured traffic carries the captured body, and a
//! capture is bounded by the proxy's own limit. So the body is a
//! [`BodyCapture`] rather than plain bytes: it knows how long the body was as
//! well as how much of it is here, and a saved request built from a truncated
//! capture says so rather than quietly becoming a request that sends less. A
//! body somebody typed is a whole body and is marked as one.
//!
//! # What is not checked here
//!
//! Whether the URL is a URL and the method is a method. This crate persists;
//! `proxy-http` is what decides what a request may be, and it cannot be
//! depended on from here — it depends on this crate. So a saved request is
//! checked into a sendable one at the IPC boundary, which has both, and what
//! reaches these columns has already been through that check. What *is*
//! checked here is the part that is this model's own: a name.
//!
//! # Sensitive data
//!
//! A saved request carries whatever the request carried — an `Authorization`
//! header, a `Cookie`, an API key. That is the point: a saved request that
//! dropped the credential would reproduce nothing. So no type here renders a
//! header value, a body, or a URL through [`Debug`], for the same reason
//! nothing on the cURL and replay path does.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::body::BodyCapture;
use crate::headers::Headers;

/// The longest name a saved request may carry.
///
/// A name is a label in a list, not a description. The same limit a rule's
/// name has, and for the same reason.
pub const MAX_SAVED_REQUEST_NAME_LENGTH: usize = 200;

/// Identifies one saved request, for as long as it is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SavedRequestId(Uuid);

impl SavedRequestId {
    /// A new id, for a request being saved now.
    pub fn next() -> Self {
        Self(Uuid::now_v7())
    }

    /// The id as the UUID it is.
    pub fn as_uuid(self) -> Uuid {
        self.0
    }

    /// The id in the form it is written down as: hyphenated and lowercase.
    pub fn as_text(self) -> String {
        self.to_string()
    }
}

impl From<Uuid> for SavedRequestId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl FromStr for SavedRequestId {
    type Err = uuid::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(text).map(Self)
    }
}

impl fmt::Display for SavedRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_hyphenated().fmt(f)
    }
}

/// One request somebody kept.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequest {
    /// Which saved request this is.
    pub id: SavedRequestId,
    /// What a person calls it.
    pub name: String,
    /// The method it will be sent with.
    pub method: String,
    /// The absolute URL it will be sent to.
    pub url: String,
    /// The headers, in the order they will be sent.
    pub headers: Headers,
    /// The body, and how much of one there was.
    pub body: BodyCapture,
    /// When it was saved, in milliseconds since the Unix epoch.
    pub created_at_ms: u64,
    /// When it was last changed, in milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
}

impl SavedRequest {
    /// Saves `new`, minting its id and its timestamps.
    pub fn create(new: NewSavedRequest) -> Result<Self, SavedRequestError> {
        let now = now_ms();
        let saved = Self {
            id: SavedRequestId::next(),
            name: new.name,
            method: new.method,
            url: new.url,
            headers: new.headers,
            body: BodyCapture::complete(new.body),
            created_at_ms: now,
            updated_at_ms: now,
        };
        saved.validate()?;

        Ok(saved)
    }

    /// The same request with `update` applied, and its `updated_at` moved on.
    ///
    /// A field the update does not name is left as it was, which is what
    /// makes renaming one field rather than the whole request read back and
    /// sent again.
    pub fn updated(&self, update: SavedRequestUpdate) -> Result<Self, SavedRequestError> {
        let mut saved = self.clone();

        if let Some(name) = update.name {
            saved.name = name;
        }
        if let Some(method) = update.method {
            saved.method = method;
        }
        if let Some(url) = update.url {
            saved.url = url;
        }
        if let Some(headers) = update.headers {
            saved.headers = headers;
        }
        if let Some(body) = update.body {
            // A body somebody sent is the whole body: the truncation an
            // earlier capture had is not carried over onto bytes that did
            // not come from it.
            saved.body = BodyCapture::complete(body);
        }
        saved.updated_at_ms = now_ms();
        saved.validate()?;

        Ok(saved)
    }

    /// Whether this is a saved request this build can store and hand back.
    ///
    /// Checked when one is saved, when it is changed, and again when it is
    /// read back from the database, for the reason a rule is: a row edited by
    /// hand should not become a nameless entry in somebody's list.
    pub fn validate(&self) -> Result<(), SavedRequestError> {
        if self.name.trim().is_empty() {
            return Err(SavedRequestError::NameEmpty);
        }
        if self.name.chars().count() > MAX_SAVED_REQUEST_NAME_LENGTH {
            return Err(SavedRequestError::NameTooLong);
        }
        if self.name.chars().any(char::is_control) {
            return Err(SavedRequestError::NameInvalid);
        }
        if self.method.trim().is_empty() {
            return Err(SavedRequestError::MethodEmpty);
        }
        if self.url.trim().is_empty() {
            return Err(SavedRequestError::UrlEmpty);
        }

        Ok(())
    }
}

/// Metadata only: never the URL, a header value, or the body. See the module
/// documentation for why.
impl fmt::Debug for SavedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedRequest")
            .field("id", &self.id)
            .field("method", &self.method)
            .field("headers", &self.headers.len())
            .field("body", &self.body)
            .field("created_at_ms", &self.created_at_ms)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

/// A request that has not been saved yet.
///
/// What a client sends to save one: everything but the id and the
/// timestamps, which are the server's to mint. The body is plain bytes here
/// rather than a [`BodyCapture`] because a client cannot be the one to say
/// how long a body *was* — a request being saved carries the body it carries,
/// and the only place a truncated one comes from is a capture the core
/// already holds.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSavedRequest {
    /// What a person calls it.
    pub name: String,
    /// The method it will be sent with.
    pub method: String,
    /// The absolute URL it will be sent to.
    pub url: String,
    /// The headers, in the order they will be sent.
    #[serde(default)]
    pub headers: Headers,
    /// The body, which is bytes and need not be text.
    #[serde(default, with = "crate::base64_bytes")]
    pub body: Vec<u8>,
}

impl NewSavedRequest {
    /// A request called `name` that sends `method` to `url`.
    pub fn new(name: impl Into<String>, method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            method: method.into(),
            url: url.into(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }
}

/// Metadata only. See the module documentation.
impl fmt::Debug for NewSavedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewSavedRequest")
            .field("method", &self.method)
            .field("headers", &self.headers.len())
            .field("body", &self.body.len())
            .finish()
    }
}

/// A change to a request that is already saved.
///
/// Every field is optional, and one that is not named is not changed. That is
/// what makes a rename one field rather than the whole request sent back —
/// which would race with anything else editing it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRequestUpdate {
    /// What a person calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The method it will be sent with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The absolute URL it will be sent to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The headers, in the order they will be sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Headers>,
    /// The body, which is bytes and need not be text.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::base64_bytes::optional"
    )]
    pub body: Option<Vec<u8>>,
}

impl SavedRequestUpdate {
    /// An update that changes nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// An update that only renames.
    pub fn name(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    /// Whether this update changes anything at all.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Metadata only. See the module documentation.
impl fmt::Debug for SavedRequestUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedRequestUpdate")
            .field("renames", &self.name.is_some())
            .field("method", &self.method.is_some())
            .field("url", &self.url.is_some())
            .field("headers", &self.headers.as_ref().map(Headers::len))
            .field("body", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// Why a saved request is not one this build can keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedRequestError {
    /// It has no name.
    NameEmpty,
    /// The name is longer than [`MAX_SAVED_REQUEST_NAME_LENGTH`].
    NameTooLong,
    /// The name carries characters a name cannot.
    NameInvalid,
    /// It names no method.
    MethodEmpty,
    /// It names no URL.
    UrlEmpty,
}

impl SavedRequestError {
    /// What went wrong, in words that name nothing the caller supplied.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NameEmpty => "a saved request must have a name",
            Self::NameTooLong => "a saved request name must be at most 200 characters",
            Self::NameInvalid => "a saved request name must not contain control characters",
            Self::MethodEmpty => "a saved request must name a method",
            Self::UrlEmpty => "a saved request must name a URL",
        }
    }
}

impl fmt::Display for SavedRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for SavedRequestError {}

/// The wall clock, in milliseconds since the Unix epoch.
///
/// A clock set before the epoch gives zero rather than an error: a nonsensical
/// clock is not a reason to refuse to save somebody's request.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::headers::HeaderValue;

    fn new() -> NewSavedRequest {
        let mut request =
            NewSavedRequest::new("Create a user", "POST", "https://example.com/users");
        request.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        request.body = br#"{"name":"ada"}"#.to_vec();

        request
    }

    #[test]
    fn saving_a_request_mints_an_id_and_timestamps() {
        let saved = SavedRequest::create(new()).expect("saves");

        assert_eq!(saved.name, "Create a user");
        assert_eq!(saved.method, "POST");
        assert_eq!(saved.url, "https://example.com/users");
        assert_eq!(
            saved.headers.text("authorization"),
            Some("Bearer sk-secret")
        );
        assert_eq!(saved.body.as_text(), Some(r#"{"name":"ada"}"#));
        assert!(saved.created_at_ms > 0);
        assert_eq!(saved.created_at_ms, saved.updated_at_ms);
        assert_eq!(saved.id.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn a_body_somebody_sent_is_a_whole_body() {
        let saved = SavedRequest::create(new()).expect("saves");

        assert!(!saved.body.is_truncated());
        assert_eq!(saved.body.size(), 14);
    }

    #[test]
    fn two_saved_requests_have_different_ids() {
        let first = SavedRequest::create(new()).expect("saves");
        let second = SavedRequest::create(new()).expect("saves");

        assert_ne!(first.id, second.id);
        assert!(
            second.id > first.id,
            "{:?} is not after {:?}",
            second.id,
            first.id
        );
    }

    #[test]
    fn an_update_changes_only_what_it_names() {
        let saved = SavedRequest::create(new()).expect("saves");

        let renamed = saved
            .updated(SavedRequestUpdate::name("Create Ada"))
            .expect("updates");

        assert_eq!(renamed.name, "Create Ada");
        assert_eq!(renamed.method, saved.method);
        assert_eq!(renamed.url, saved.url);
        assert_eq!(renamed.headers, saved.headers);
        assert_eq!(renamed.body, saved.body);
        assert_eq!(renamed.id, saved.id);
        assert_eq!(renamed.created_at_ms, saved.created_at_ms);
        assert!(renamed.updated_at_ms >= saved.updated_at_ms);
    }

    #[test]
    fn an_update_can_change_the_whole_request() {
        let saved = SavedRequest::create(new()).expect("saves");

        let changed = saved
            .updated(SavedRequestUpdate {
                method: Some("PUT".to_owned()),
                url: Some("https://example.com/users/1".to_owned()),
                headers: Some(Headers::new()),
                body: Some(b"{}".to_vec()),
                ..SavedRequestUpdate::none()
            })
            .expect("updates");

        assert_eq!(changed.method, "PUT");
        assert_eq!(changed.url, "https://example.com/users/1");
        assert!(changed.headers.is_empty());
        assert_eq!(changed.body.as_text(), Some("{}"));
    }

    #[test]
    fn editing_the_body_of_a_truncated_request_makes_it_whole() {
        let mut saved = SavedRequest::create(new()).expect("saves");
        saved.body = BodyCapture::truncated(b"the first bit".to_vec(), 5_000);

        let edited = saved
            .updated(SavedRequestUpdate {
                body: Some(b"all of it".to_vec()),
                ..SavedRequestUpdate::none()
            })
            .expect("updates");

        assert!(!edited.body.is_truncated());
        assert_eq!(edited.body.size(), 9);
    }

    #[test]
    fn an_update_that_leaves_the_body_alone_keeps_its_truncation() {
        let mut saved = SavedRequest::create(new()).expect("saves");
        saved.body = BodyCapture::truncated(b"the first bit".to_vec(), 5_000);

        let renamed = saved
            .updated(SavedRequestUpdate::name("Still truncated"))
            .expect("updates");

        assert!(renamed.body.is_truncated());
        assert_eq!(renamed.body.size(), 5_000);
    }

    #[test]
    fn a_request_with_no_name_is_refused() {
        let mut request = new();
        request.name = "   ".to_owned();

        assert_eq!(
            SavedRequest::create(request),
            Err(SavedRequestError::NameEmpty)
        );
    }

    #[test]
    fn a_name_that_is_too_long_or_carries_control_characters_is_refused() {
        let mut long = new();
        long.name = "n".repeat(MAX_SAVED_REQUEST_NAME_LENGTH + 1);
        assert_eq!(
            SavedRequest::create(long),
            Err(SavedRequestError::NameTooLong)
        );

        let mut control = new();
        control.name = "one\nline".to_owned();
        assert_eq!(
            SavedRequest::create(control),
            Err(SavedRequestError::NameInvalid)
        );
    }

    #[test]
    fn a_request_with_no_method_or_no_url_is_refused() {
        let mut without_method = new();
        without_method.method = String::new();
        assert_eq!(
            SavedRequest::create(without_method),
            Err(SavedRequestError::MethodEmpty)
        );

        let mut without_url = new();
        without_url.url = "  ".to_owned();
        assert_eq!(
            SavedRequest::create(without_url),
            Err(SavedRequestError::UrlEmpty)
        );
    }

    #[test]
    fn a_rename_to_nothing_is_refused_and_leaves_the_request_alone() {
        let saved = SavedRequest::create(new()).expect("saves");

        assert_eq!(
            saved.updated(SavedRequestUpdate::name(" ")),
            Err(SavedRequestError::NameEmpty)
        );
        assert_eq!(saved.name, "Create a user");
    }

    #[test]
    fn every_reason_says_something_different() {
        let reasons: std::collections::BTreeSet<&str> = [
            SavedRequestError::NameEmpty,
            SavedRequestError::NameTooLong,
            SavedRequestError::NameInvalid,
            SavedRequestError::MethodEmpty,
            SavedRequestError::UrlEmpty,
        ]
        .into_iter()
        .map(SavedRequestError::reason)
        .collect();

        assert_eq!(reasons.len(), 5);
    }

    #[test]
    fn a_saved_request_round_trips_through_json() {
        let saved = SavedRequest::create(new()).expect("saves");

        let json = serde_json::to_string(&saved).expect("serializes");
        let back: SavedRequest = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, saved);
    }

    #[test]
    fn a_binary_body_round_trips_through_json() {
        let mut request = new();
        request.body = (0..=255).collect();
        let saved = SavedRequest::create(request).expect("saves");

        let json = serde_json::to_string(&saved).expect("serializes");
        let back: SavedRequest = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back.body.data(), saved.body.data());
    }

    #[test]
    fn the_json_names_the_parts_a_client_would_look_for() {
        let saved = SavedRequest::create(new()).expect("saves");
        let json = serde_json::to_value(&saved).expect("serializes");

        assert_eq!(json["name"], "Create a user");
        assert_eq!(json["method"], "POST");
        assert_eq!(json["body"]["data"], "eyJuYW1lIjoiYWRhIn0=");
        assert_eq!(json["body"]["size"], 14);
        assert_eq!(json["body"]["truncated"], false);
    }

    #[test]
    fn a_new_request_is_sent_with_its_body_as_base64() {
        let json = serde_json::to_value(new()).expect("serializes");

        assert_eq!(json["body"], "eyJuYW1lIjoiYWRhIn0=");
        assert_eq!(json["url"], "https://example.com/users");
    }

    #[test]
    fn an_update_sends_only_the_fields_it_changes() {
        let json = serde_json::to_value(SavedRequestUpdate::name("New name")).expect("serializes");

        assert_eq!(json, serde_json::json!({"name": "New name"}));
    }

    #[test]
    fn an_update_carrying_a_body_sends_it_as_base64() {
        let update = SavedRequestUpdate {
            body: Some(b"hello".to_vec()),
            ..SavedRequestUpdate::none()
        };

        let json = serde_json::to_value(&update).expect("serializes");
        assert_eq!(json, serde_json::json!({"body": "aGVsbG8="}));

        let back: SavedRequestUpdate = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, update);
    }

    #[test]
    fn an_update_that_names_nothing_changes_nothing() {
        assert!(SavedRequestUpdate::none().is_empty());
        assert!(!SavedRequestUpdate::name("x").is_empty());
    }

    #[test]
    fn an_id_round_trips_through_its_text_form() {
        let id = SavedRequestId::next();

        assert_eq!(id.as_text().parse::<SavedRequestId>(), Ok(id));
        assert!("not-an-id".parse::<SavedRequestId>().is_err());
    }

    #[test]
    fn debug_never_renders_the_url_a_header_or_the_body() {
        let saved = SavedRequest::create(new()).expect("saves");

        for rendered in [
            format!("{saved:?}"),
            format!("{:?}", new()),
            format!(
                "{:?}",
                SavedRequestUpdate {
                    url: Some("https://example.com/secret".to_owned()),
                    body: Some(b"ada".to_vec()),
                    ..SavedRequestUpdate::none()
                }
            ),
        ] {
            assert!(!rendered.contains("sk-secret"), "{rendered}");
            assert!(!rendered.contains("example.com"), "{rendered}");
            assert!(!rendered.contains("ada"), "{rendered}");
        }
    }
}
