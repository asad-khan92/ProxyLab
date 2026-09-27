//! Changing a request on its way out, or a response on its way back.
//!
//! ```text
//! Modify Request    method · path · query · headers · body
//! Modify Response   status · headers · body
//! ```
//!
//! A transformation is not a decision. It does not say where a request goes
//! or who answers it — the rule that decides that is still the first
//! matching one of `Allow`, `Block`, `MapLocal`, `MapRemote` or `Redirect` —
//! it says what the message looks like when it gets there. See
//! [`crate::engine`] for how the two kinds of rule are consulted together.
//!
//! # Everything literal
//!
//! Every edit is a literal operation on the message: set this header to this
//! value, replace this path prefix with that one, set this query parameter,
//! replace the body with this text, set this JSON field to this value. No
//! pattern, no regex, no template, no expression. A transformation is read at
//! a glance and does the one thing it says.
//!
//! # Deciding and doing, again
//!
//! What is here is pure: a transformation is a value, validated when the rule
//! is written, and the functions that apply one to a path, a query string or
//! a body are functions of their arguments. The proxy pipeline reads the
//! body, applies them, and fixes up the framing — see `proxy-engine`'s
//! `modify`.

use serde::{Deserialize, Serialize};

use crate::action::RuleActionError;

/// The most edits of one kind a transformation may carry.
///
/// A transformation is something a person wrote in an editor. Thirty-two
/// header edits is more than anyone writes by hand; a thousand is a rule
/// built by something else, and not what this is for.
pub const MAX_EDITS: usize = 32;

/// The longest body text a transformation may carry.
///
/// A rule crosses the IPC boundary in one request line, which is 64 KiB, and
/// the text is in it. Anything longer is a file, and Map Local is the rule
/// that answers with a file.
pub const MAX_BODY_TEXT_BYTES: usize = 32 * 1024;

/// Headers no transformation may touch.
///
/// Framing is the proxy's: a rule that set `Content-Length` or
/// `Transfer-Encoding` would describe a body that is not the one being sent.
/// The hop-by-hop headers are removed on every hop anyway, so an edit to one
/// would do nothing. And `Host` is where a request is going, which is Map
/// Remote's to change — a request that names one host and connects to
/// another is not a thing to do by accident.
pub const PROTECTED_HEADERS: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
    "host",
];

/// What a Modify Request rule changes.
///
/// Every part is optional, and one that is not set is left alone. A
/// transformation that sets nothing is refused: it would be a rule that does
/// nothing and looks as if it does something.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifyRequest {
    /// The method the request is sent with instead of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// What happens to the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathEdit>,
    /// What happens to the query parameters, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<QueryEdit>,
    /// What happens to the headers, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderEdit>,
    /// What happens to the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyEdit>,
}

impl ModifyRequest {
    /// Whether this is a transformation this build can carry out.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if self.method.is_none()
            && self.path.is_none()
            && self.query.is_empty()
            && self.headers.is_empty()
            && self.body.is_none()
        {
            return Err(RuleActionError::ModifyNothing);
        }
        if let Some(method) = &self.method {
            if !is_token(method) {
                return Err(RuleActionError::ModifyMethod);
            }
        }
        if let Some(path) = &self.path {
            path.validate()?;
        }
        if self.query.len() > MAX_EDITS {
            return Err(RuleActionError::ModifyTooMany);
        }
        for edit in &self.query {
            edit.validate()?;
        }
        validate_headers(&self.headers)?;
        if let Some(body) = &self.body {
            body.validate()?;
        }

        Ok(())
    }
}

/// What a Modify Response rule changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifyResponse {
    /// The status the client is given instead of the origin's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// What happens to the headers, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderEdit>,
    /// What happens to the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyEdit>,
}

impl ModifyResponse {
    /// Whether this is a transformation this build can carry out.
    ///
    /// The status is a final one — `200` to `599`. An informational status
    /// is not an answer, and a response rewritten to `101` would tell the
    /// client the connection had stopped being HTTP when it had not.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if self.status.is_none() && self.headers.is_empty() && self.body.is_none() {
            return Err(RuleActionError::ModifyNothing);
        }
        if let Some(status) = self.status {
            if !(200..=599).contains(&status) {
                return Err(RuleActionError::ModifyStatus);
            }
        }
        validate_headers(&self.headers)?;
        if let Some(body) = &self.body {
            body.validate()?;
        }

        Ok(())
    }
}

/// What happens to a request path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathEdit {
    /// The path becomes exactly this.
    Set {
        /// The new path, starting with `/`.
        path: String,
    },
    /// A path that starts with `from` has that part replaced by `to`.
    ///
    /// A string prefix, like the matcher's: `/v1` turns `/v10` into `/v20`.
    /// A rule that means the folder writes the trailing slash on both.
    ReplacePrefix {
        /// The prefix to look for.
        from: String,
        /// What it becomes.
        to: String,
    },
}

impl PathEdit {
    /// The path `path` becomes.
    pub fn apply(&self, path: &str) -> String {
        match self {
            Self::Set { path } => path.clone(),
            Self::ReplacePrefix { from, to } => match path.strip_prefix(from.as_str()) {
                Some(rest) => format!("{to}{rest}"),
                None => path.to_owned(),
            },
        }
    }

    fn validate(&self) -> Result<(), RuleActionError> {
        let valid = match self {
            Self::Set { path } => is_path(path),
            Self::ReplacePrefix { from, to } => is_path(from) && is_path(to),
        };

        if valid {
            Ok(())
        } else {
            Err(RuleActionError::ModifyPath)
        }
    }
}

/// What happens to one query parameter.
///
/// Names and values are compared and written exactly as they appear in the
/// URL: nothing is percent-decoded or encoded, so `a%20b` and `a b` are two
/// different names, and a value that needs encoding is written encoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueryEdit {
    /// The parameter has exactly this value: the first occurrence is changed,
    /// any others are removed, and one that is not there is added at the end.
    Set {
        /// The parameter's name.
        name: String,
        /// Its value.
        value: String,
    },
    /// Every occurrence of the parameter is removed.
    Remove {
        /// The parameter's name.
        name: String,
    },
}

impl QueryEdit {
    fn validate(&self) -> Result<(), RuleActionError> {
        let (name, value) = match self {
            Self::Set { name, value } => (name, Some(value)),
            Self::Remove { name } => (name, None),
        };

        let name_ok = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte > 0x20 && byte != 0x7F && !b"&=#".contains(&byte));
        let value_ok = value.map_or(true, |value| {
            value
                .bytes()
                .all(|byte| byte > 0x20 && byte != 0x7F && !b"&#".contains(&byte))
        });

        if name_ok && value_ok {
            Ok(())
        } else {
            Err(RuleActionError::ModifyQuery)
        }
    }
}

/// Applies `edits` to `query`, in order, and gives back the query that
/// results — `None` when no parameter is left.
///
/// Parameters the edits do not name keep their place and their spelling. An
/// empty segment (`a=1&&b=2`) is dropped, which is the one normalisation
/// made, and only when an edit applies at all.
pub fn apply_query(query: Option<&str>, edits: &[QueryEdit]) -> Option<String> {
    if edits.is_empty() {
        return query.map(str::to_owned);
    }

    let mut pairs: Vec<String> = query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(str::to_owned)
        .collect();

    for edit in edits {
        match edit {
            QueryEdit::Set { name, value } => {
                let written = format!("{name}={value}");
                let mut seen = false;
                pairs.retain_mut(|pair| {
                    if parameter_name(pair) != name {
                        return true;
                    }
                    if seen {
                        return false;
                    }
                    seen = true;
                    pair.clone_from(&written);
                    true
                });
                if !seen {
                    pairs.push(written);
                }
            }
            QueryEdit::Remove { name } => pairs.retain(|pair| parameter_name(pair) != name),
        }
    }

    (!pairs.is_empty()).then(|| pairs.join("&"))
}

/// The name of one `name=value` pair: everything before the first `=`.
fn parameter_name(pair: &str) -> &str {
    pair.split_once('=').map_or(pair, |(name, _)| name)
}

/// What happens to one header.
///
/// Names are compared without regard to case, which is how HTTP compares
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum HeaderEdit {
    /// The header has exactly this one value, replacing any it had.
    Set {
        /// The header's name.
        name: String,
        /// Its value.
        value: String,
    },
    /// One more value for the header, after any it already has.
    Add {
        /// The header's name.
        name: String,
        /// The value added.
        value: String,
    },
    /// The header is removed, every value of it.
    Remove {
        /// The header's name.
        name: String,
    },
}

impl HeaderEdit {
    /// The header this edit is about.
    pub fn name(&self) -> &str {
        match self {
            Self::Set { name, .. } | Self::Add { name, .. } | Self::Remove { name } => name,
        }
    }

    fn validate(&self) -> Result<(), RuleActionError> {
        let name = self.name();
        if !is_token(name) {
            return Err(RuleActionError::ModifyHeaderName);
        }
        if PROTECTED_HEADERS
            .iter()
            .any(|protected| protected.eq_ignore_ascii_case(name))
        {
            return Err(RuleActionError::ModifyHeaderProtected);
        }

        let value = match self {
            Self::Set { value, .. } | Self::Add { value, .. } => value,
            Self::Remove { .. } => return Ok(()),
        };
        // Visible characters, spaces and tabs: what a field value may carry.
        // A line break here is a second header, or a second request.
        if value
            .bytes()
            .any(|byte| (byte < 0x20 && byte != b'\t') || byte == 0x7F)
        {
            return Err(RuleActionError::ModifyHeaderValue);
        }

        Ok(())
    }
}

fn validate_headers(edits: &[HeaderEdit]) -> Result<(), RuleActionError> {
    if edits.len() > MAX_EDITS {
        return Err(RuleActionError::ModifyTooMany);
    }

    edits.iter().try_for_each(HeaderEdit::validate)
}

/// What happens to a body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyEdit {
    /// The body becomes exactly this text, in UTF-8.
    Replace {
        /// The new body.
        text: String,
    },
    /// One field of a JSON object body is set to a JSON value.
    ///
    /// `path` is object keys joined by dots — `user.address.city` — and
    /// nothing else: no array indexes, no wildcards, no escaping. A key that
    /// is missing along the way is created as an object. A body that is not
    /// a JSON object, or a key along the path that holds something other
    /// than an object, leaves the body exactly as it was.
    SetJson {
        /// Where the field is.
        path: String,
        /// The value it is set to, written as JSON: `"staging"`, `42`,
        /// `true`, `null`, `{"a":1}`.
        value: String,
    },
}

impl BodyEdit {
    /// The body `body` becomes, or `None` when the edit does not apply to it.
    ///
    /// `Replace` always applies. `SetJson` applies to a body that parses as a
    /// JSON object, and the result is that object written back compactly —
    /// with its keys in sorted order, because the JSON library keeps objects
    /// sorted. It is the same JSON, and not the same bytes.
    pub fn apply(&self, body: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Replace { text } => Some(text.as_bytes().to_vec()),
            Self::SetJson { path, value } => {
                let value: serde_json::Value = serde_json::from_str(value).ok()?;
                let mut document: serde_json::Value = serde_json::from_slice(body).ok()?;

                let mut keys = path.split('.').peekable();
                let mut node = document.as_object_mut()?;
                while let Some(key) = keys.next() {
                    if keys.peek().is_none() {
                        node.insert(key.to_owned(), value);
                        break;
                    }

                    node = node
                        .entry(key.to_owned())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                        .as_object_mut()?;
                }

                serde_json::to_vec(&document).ok()
            }
        }
    }

    /// Whether the edit needs the body it replaces.
    ///
    /// A replacement does not: it is the same whatever was there. A JSON edit
    /// does, and the proxy reads the whole body before applying one.
    pub fn reads_body(&self) -> bool {
        matches!(self, Self::SetJson { .. })
    }

    fn validate(&self) -> Result<(), RuleActionError> {
        match self {
            Self::Replace { text } => {
                if text.len() > MAX_BODY_TEXT_BYTES {
                    return Err(RuleActionError::ModifyBodyTooLong);
                }
            }
            Self::SetJson { path, value } => {
                if path.is_empty() || path.split('.').any(str::is_empty) {
                    return Err(RuleActionError::ModifyJsonPath);
                }
                if value.len() > MAX_BODY_TEXT_BYTES {
                    return Err(RuleActionError::ModifyBodyTooLong);
                }
                if serde_json::from_str::<serde_json::Value>(value).is_err() {
                    return Err(RuleActionError::ModifyJsonValue);
                }
            }
        }

        Ok(())
    }
}

/// Whether `text` is one HTTP token: what a method and a header name are.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Whether `text` is a path a request line can carry, with no query.
fn is_path(text: &str) -> bool {
    text.starts_with('/')
        && text
            .bytes()
            .all(|byte| byte > 0x20 && byte != 0x7F && !b"?#".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(name: &str, value: &str) -> QueryEdit {
        QueryEdit::Set {
            name: name.into(),
            value: value.into(),
        }
    }

    fn remove(name: &str) -> QueryEdit {
        QueryEdit::Remove { name: name.into() }
    }

    #[test]
    fn a_parameter_that_is_there_is_changed_where_it_is() {
        assert_eq!(
            apply_query(Some("a=1&env=prod&b=2"), &[set("env", "staging")]).as_deref(),
            Some("a=1&env=staging&b=2")
        );
    }

    #[test]
    fn a_parameter_that_is_not_there_is_added_at_the_end() {
        assert_eq!(
            apply_query(Some("a=1"), &[set("debug", "1")]).as_deref(),
            Some("a=1&debug=1")
        );
        assert_eq!(
            apply_query(None, &[set("debug", "1")]).as_deref(),
            Some("debug=1")
        );
    }

    #[test]
    fn setting_a_repeated_parameter_leaves_one() {
        assert_eq!(
            apply_query(Some("id=1&x=0&id=2"), &[set("id", "9")]).as_deref(),
            Some("id=9&x=0")
        );
    }

    #[test]
    fn removing_the_last_parameter_removes_the_query() {
        assert_eq!(apply_query(Some("debug=1"), &[remove("debug")]), None);
    }

    #[test]
    fn names_are_compared_as_written() {
        assert_eq!(
            apply_query(Some("Env=prod&env=prod"), &[remove("env")]).as_deref(),
            Some("Env=prod")
        );
        // A bare name is a parameter with no value.
        assert_eq!(
            apply_query(Some("flag&a=1"), &[remove("flag")]).as_deref(),
            Some("a=1")
        );
    }

    #[test]
    fn no_edits_is_the_query_untouched() {
        assert_eq!(apply_query(Some("a=1&&b"), &[]).as_deref(), Some("a=1&&b"));
    }

    #[test]
    fn a_path_is_set_or_has_its_prefix_replaced() {
        let set = PathEdit::Set {
            path: "/v2/users".into(),
        };
        let prefix = PathEdit::ReplacePrefix {
            from: "/api/v1/".into(),
            to: "/api/v2/".into(),
        };

        assert_eq!(set.apply("/anything"), "/v2/users");
        assert_eq!(prefix.apply("/api/v1/users"), "/api/v2/users");
        assert_eq!(prefix.apply("/other/v1/users"), "/other/v1/users");
    }

    #[test]
    fn a_json_field_is_set_and_missing_objects_are_made() {
        let edit = BodyEdit::SetJson {
            path: "user.address.city".into(),
            value: "\"Lahore\"".into(),
        };

        let edited = edit
            .apply(br#"{"user":{"name":"a"},"z":1}"#)
            .expect("applies");

        assert_eq!(
            edited,
            br#"{"user":{"address":{"city":"Lahore"},"name":"a"},"z":1}"#
        );
    }

    #[test]
    fn a_json_edit_leaves_a_body_it_cannot_apply_to_alone() {
        let edit = BodyEdit::SetJson {
            path: "a.b".into(),
            value: "1".into(),
        };

        assert_eq!(edit.apply(b"not json"), None);
        assert_eq!(edit.apply(b"[1,2]"), None);
        assert_eq!(edit.apply(br#"{"a":"text"}"#), None);
    }

    #[test]
    fn a_replacement_is_the_text_whatever_was_there() {
        let edit = BodyEdit::Replace { text: "{}".into() };

        assert_eq!(edit.apply(b"\x00\xff binary").as_deref(), Some(&b"{}"[..]));
        assert!(!edit.reads_body());
    }

    #[test]
    fn a_transformation_that_changes_nothing_is_refused() {
        assert_eq!(
            ModifyRequest::default().validate(),
            Err(RuleActionError::ModifyNothing)
        );
        assert_eq!(
            ModifyResponse::default().validate(),
            Err(RuleActionError::ModifyNothing)
        );
    }

    #[test]
    fn the_framing_headers_and_host_cannot_be_edited() {
        for name in ["Content-Length", "transfer-encoding", "Host", "Connection"] {
            let modify = ModifyRequest {
                headers: vec![HeaderEdit::Remove { name: name.into() }],
                ..ModifyRequest::default()
            };

            assert_eq!(
                modify.validate(),
                Err(RuleActionError::ModifyHeaderProtected),
                "{name}"
            );
        }
    }

    #[test]
    fn a_header_value_cannot_carry_a_line_break() {
        let modify = ModifyRequest {
            headers: vec![HeaderEdit::Set {
                name: "X-A".into(),
                value: "1\r\nX-B: 2".into(),
            }],
            ..ModifyRequest::default()
        };

        assert_eq!(modify.validate(), Err(RuleActionError::ModifyHeaderValue));
    }

    #[test]
    fn an_informational_status_is_not_an_answer() {
        for status in [100, 101, 199, 600] {
            let modify = ModifyResponse {
                status: Some(status),
                ..ModifyResponse::default()
            };

            assert_eq!(modify.validate(), Err(RuleActionError::ModifyStatus));
        }
    }

    #[test]
    fn a_json_edit_must_name_a_field_and_carry_json() {
        let bad_path = BodyEdit::SetJson {
            path: "a..b".into(),
            value: "1".into(),
        };
        let bad_value = BodyEdit::SetJson {
            path: "a".into(),
            value: "not json".into(),
        };

        assert_eq!(bad_path.validate(), Err(RuleActionError::ModifyJsonPath));
        assert_eq!(bad_value.validate(), Err(RuleActionError::ModifyJsonValue));
    }

    #[test]
    fn a_path_edit_carries_a_path_and_no_query() {
        for path in ["relative", "/a?b=1", "/a b", ""] {
            let edit = PathEdit::Set { path: path.into() };

            assert_eq!(edit.validate(), Err(RuleActionError::ModifyPath), "{path}");
        }
    }

    #[test]
    fn a_query_edit_cannot_smuggle_a_second_parameter() {
        for edit in [
            set("a&b", "1"),
            set("a", "1&b=2"),
            set("", "1"),
            set("a", "x y"),
        ] {
            assert_eq!(
                edit.validate(),
                Err(RuleActionError::ModifyQuery),
                "{edit:?}"
            );
        }
    }
}
