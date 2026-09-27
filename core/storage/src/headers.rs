//! The headers of a captured message.
//!
//! Headers are kept as ProxyLab's own structure rather than as the HTTP
//! library's: nothing downstream — IPC, storage, a UI — should have to depend
//! on hyper to read a captured exchange.
//!
//! Order and repetition are preserved. A message may carry the same header
//! name more than once, and which value came first can be the thing a
//! developer is looking at, so this is a list of pairs and not a map.
//!
//! # Sensitive data
//!
//! Header values are exactly where credentials live: `Authorization`,
//! `Cookie`, `Set-Cookie`, and every bearer token and API key that a service
//! puts in a header of its own naming. The capture keeps them, because the
//! debugging UI is going to need them. [`Debug`] never renders one — not for
//! a name this crate happens to recognise, but for any header at all, because
//! a list of names to redact is a list that will be missing the next one.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The header section of a captured request or response.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Headers {
    entries: Vec<Header>,
}

impl Headers {
    /// A header section with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a header, keeping it after every header already added.
    pub fn push(&mut self, name: impl Into<String>, value: HeaderValue) {
        self.entries.push(Header {
            name: name.into(),
            value,
        });
    }

    /// Every header, in the order the message carried them.
    pub fn iter(&self) -> std::slice::Iter<'_, Header> {
        self.entries.iter()
    }

    /// How many headers the message carried, repetitions counted separately.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the message carried no headers at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The first value of `name`, matched without regard to case.
    ///
    /// Header names are case-insensitive, and a captured message names them
    /// however the sender spelled them.
    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| &header.value)
    }

    /// Every value of `name`, matched without regard to case.
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a HeaderValue> + 'a {
        self.iter()
            .filter(move |header| header.name.eq_ignore_ascii_case(name))
            .map(|header| &header.value)
    }

    /// Whether the message carried `name` at all.
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The first value of `name` as text, when it is text.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(HeaderValue::as_text)
    }
}

impl FromIterator<Header> for Headers {
    fn from_iter<I: IntoIterator<Item = Header>>(entries: I) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}

impl<'a> IntoIterator for &'a Headers {
    type Item = &'a Header;
    type IntoIter = std::slice::Iter<'a, Header>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Names and sizes only. See the module documentation for why.
impl fmt::Debug for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// One header of a captured message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The name, spelled as the sender spelled it.
    pub name: String,
    /// The value, exactly as it arrived.
    pub value: HeaderValue,
}

impl Header {
    /// A header with a text value.
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: HeaderValue::Text(value.into()),
        }
    }

    /// A header whose value is read from raw bytes.
    pub fn from_bytes(name: impl Into<String>, value: &[u8]) -> Self {
        Self {
            name: name.into(),
            value: HeaderValue::from_bytes(value),
        }
    }
}

/// The name and the size of the value, never the value. See the module
/// documentation for why.
impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: <{} bytes>", self.name, self.value.len())
    }
}

/// The value of one header.
///
/// Almost every header value is text, and is kept as text so that anything
/// reading the JSON can read it. A value that is not valid UTF-8 is still a
/// value the sender sent, so it is kept as raw bytes rather than lost to a
/// lossy conversion.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderValue {
    /// A value that is valid UTF-8.
    Text(String),
    /// A value that is not, kept as it arrived and written as base64 in JSON.
    Bytes(#[serde(with = "crate::base64_bytes")] Vec<u8>),
}

impl HeaderValue {
    /// Reads a value from the bytes a message carried.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::Text(text.to_owned()),
            Err(_) => Self::Bytes(bytes.to_vec()),
        }
    }

    /// The value as text, or `None` if it is not text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Bytes(_) => None,
        }
    }

    /// The value as the bytes it arrived as.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }

    /// How many bytes the value is.
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the value is empty, which a header value is allowed to be.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The size of the value, never the value. See the module documentation.
impl fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> Headers {
        let mut headers = Headers::new();
        headers.push("Content-Type", HeaderValue::Text("application/json".into()));
        headers.push("Set-Cookie", HeaderValue::Text("session=abc".into()));
        headers.push("Set-Cookie", HeaderValue::Text("theme=dark".into()));
        headers
    }

    #[test]
    fn an_empty_header_section_has_nothing_in_it() {
        let headers = Headers::new();

        assert!(headers.is_empty());
        assert_eq!(headers.len(), 0);
        assert!(headers.get("accept").is_none());
    }

    #[test]
    fn headers_keep_the_order_they_arrived_in() {
        let headers = headers();
        let names: Vec<&str> = headers.iter().map(|header| header.name.as_str()).collect();

        assert_eq!(names, ["Content-Type", "Set-Cookie", "Set-Cookie"]);
    }

    #[test]
    fn a_repeated_header_keeps_every_value() {
        let headers = headers();
        let values: Vec<Option<&str>> = headers
            .get_all("set-cookie")
            .map(HeaderValue::as_text)
            .collect();

        assert_eq!(values, [Some("session=abc"), Some("theme=dark")]);
    }

    #[test]
    fn a_header_is_found_whatever_case_it_was_spelled_in() {
        assert_eq!(headers().text("CONTENT-TYPE"), Some("application/json"));
        assert_eq!(headers().text("content-type"), Some("application/json"));
        assert!(headers().contains("Content-Type"));
        assert!(!headers().contains("accept"));
    }

    #[test]
    fn a_value_that_is_not_utf8_is_kept_as_bytes() {
        let value = HeaderValue::from_bytes(&[0xff, 0xfe]);

        assert_eq!(value, HeaderValue::Bytes(vec![0xff, 0xfe]));
        assert_eq!(value.as_text(), None);
        assert_eq!(value.as_bytes(), [0xff, 0xfe]);
        assert_eq!(value.len(), 2);
    }

    #[test]
    fn a_value_that_is_utf8_is_kept_as_text() {
        let value = HeaderValue::from_bytes(b"application/json");

        assert_eq!(value.as_text(), Some("application/json"));
        assert_eq!(value.as_bytes(), b"application/json");
    }

    #[test]
    fn an_empty_value_is_a_value() {
        let value = HeaderValue::from_bytes(b"");

        assert!(value.is_empty());
        assert_eq!(value.as_text(), Some(""));
    }

    #[test]
    fn headers_round_trip_through_json() {
        let json = serde_json::to_string(&headers()).expect("serializes");
        let back: Headers = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, headers());
    }

    #[test]
    fn a_binary_header_value_round_trips_through_json() {
        let mut original = Headers::new();
        original.push("X-Raw", HeaderValue::from_bytes(&[0x00, 0x80, 0xff]));

        let json = serde_json::to_string(&original).expect("serializes");
        let back: Headers = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, original);
        assert!(json.contains("bytes"), "{json}");
    }

    #[test]
    fn headers_are_a_json_array_of_name_and_value() {
        let mut headers = Headers::new();
        headers.push("Accept", HeaderValue::Text("*/*".into()));

        let json = serde_json::to_string(&headers).expect("serializes");

        assert_eq!(json, r#"[{"name":"Accept","value":{"text":"*/*"}}]"#);
    }

    #[test]
    fn debug_never_renders_a_header_value() {
        let mut headers = Headers::new();
        headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        headers.push("Cookie", HeaderValue::Text("session=hunter2".into()));

        let rendered = format!("{headers:?}");

        assert!(!rendered.contains("sk-secret"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("Authorization"), "{rendered}");
        assert!(rendered.contains("bytes"), "{rendered}");
    }
}
