//! What a rule matches, and exactly what that means.
//!
//! ```text
//! host    api.example.com     exact, case-insensitive, port never compared
//! path    /users              exact, case-sensitive, query never compared
//!         /assets/            prefix, case-sensitive
//! method  GET                 exact, case-insensitive
//! ```
//!
//! A matcher is three optional criteria that combine with `and`. **A
//! criterion that is not set matches anything**, so a matcher naming only a
//! host matches every request to that host, and a matcher naming nothing at
//! all matches every request. That last case is deliberate rather than
//! guarded against: a rule that matches everything is how "block all of it
//! except these" is written, and the first-match ordering is what makes that
//! work.
//!
//! # Why each comparison is the shape it is
//!
//! **Host** is compared without regard to case because DNS names are, and
//! without the port because a host is a host: a rule for
//! `api.example.com` matches it on 443 and on 8443. A rule that has to
//! distinguish ports is not something this step's matcher expresses.
//!
//! **Path** is compared with regard to case, because a URL path is
//! case-sensitive, and against the path alone — the query string is never
//! part of it. [`PathMatch::Prefix`] is a second, explicitly named matcher
//! rather than a special character inside an exact one: `/assets` meaning
//! "starts with /assets" in one rule and "is exactly /assets" in another is
//! the kind of ambiguity that makes a rule set impossible to reason about.
//!
//! **Method** is compared without regard to case. Every method that travels
//! is uppercase, so a rule written as `get` is a typo and not an intent to
//! match nothing.
//!
//! # No regex, no globs, no expressions
//!
//! Only what is above. A matcher is meant to be read at a glance and to mean
//! one thing; the moment it can express `(a|b)+` it can also express a rule
//! nobody can predict the behaviour of.

use serde::{Deserialize, Serialize};

/// The parts of a request a rule is matched against.
///
/// Built by the proxy from a request it is about to carry, and by a test
/// from three strings. It holds no body, no headers, and no connection: a
/// matcher cannot see them, so nothing here carries them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestContext<'a> {
    /// The destination host, without a port and without IPv6 brackets.
    pub host: &'a str,
    /// The request path, without the query string.
    pub path: &'a str,
    /// The request method, as it was sent.
    pub method: &'a str,
}

impl<'a> RequestContext<'a> {
    /// The request for `method` to `path` on `host`.
    pub fn new(host: &'a str, path: &'a str, method: &'a str) -> Self {
        Self { host, path, method }
    }
}

/// Which requests a rule applies to.
///
/// Every field is optional and they combine with `and`; an omitted field
/// matches anything. See the module documentation for the exact comparison
/// each one makes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMatcher {
    /// The host the request is for, or every host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostMatch>,
    /// The path the request asks for, or every path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathMatch>,
    /// The method the request used, or every method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<MethodMatch>,
}

impl RuleMatcher {
    /// A matcher that matches every request.
    pub fn any() -> Self {
        Self::default()
    }

    /// The same matcher, narrowed to `host`.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(HostMatch::exact(host));
        self
    }

    /// The same matcher, narrowed to requests for exactly `path`.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(PathMatch::Exact(path.into()));
        self
    }

    /// The same matcher, narrowed to requests whose path starts with `prefix`.
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path = Some(PathMatch::Prefix(prefix.into()));
        self
    }

    /// The same matcher, narrowed to `method`.
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(MethodMatch::exact(method));
        self
    }

    /// Whether `request` is one this matcher describes.
    ///
    /// Written as three refusals rather than one expression: a criterion
    /// that is set and does not match is the whole of why a rule is skipped,
    /// and a criterion that is not set is not consulted.
    pub fn matches(&self, request: &RequestContext<'_>) -> bool {
        if let Some(host) = &self.host {
            if !host.matches(request.host) {
                return false;
            }
        }

        if let Some(path) = &self.path {
            if !path.matches(request.path) {
                return false;
            }
        }

        if let Some(method) = &self.method {
            if !method.matches(request.method) {
                return false;
            }
        }

        true
    }

    /// Whether this matcher narrows nothing, and so matches every request.
    pub fn is_any(&self) -> bool {
        self.host.is_none() && self.path.is_none() && self.method.is_none()
    }

    /// Whether every criterion set is one this build can act on.
    ///
    /// An empty host, path, or method is not a criterion — it is a criterion
    /// that was left half-written, and matching it literally would mean a
    /// rule that quietly matches nothing.
    pub fn is_valid(&self) -> bool {
        if let Some(host) = &self.host {
            if !host.is_valid() {
                return false;
            }
        }

        if let Some(path) = &self.path {
            if !path.is_valid() {
                return false;
            }
        }

        if let Some(method) = &self.method {
            if !method.is_valid() {
                return false;
            }
        }

        true
    }
}

/// The host a rule is for.
///
/// One shape today — exact — and it is an enum so that a second one can be
/// added without every rule already persisted having to be rewritten.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum HostMatch {
    /// This host exactly, compared without regard to case.
    Exact(String),
}

impl HostMatch {
    /// The host `host` exactly.
    pub fn exact(host: impl Into<String>) -> Self {
        Self::Exact(host.into())
    }

    /// Whether `host` is the one this names.
    pub fn matches(&self, host: &str) -> bool {
        match self {
            // ASCII case folding, which is what a DNS name is compared with.
            Self::Exact(wanted) => wanted.eq_ignore_ascii_case(host),
        }
    }

    /// The host as written.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(host) => host,
        }
    }

    /// Whether this names a host at all.
    pub fn is_valid(&self) -> bool {
        let host = self.as_str();

        !host.is_empty() && !host.contains(char::is_whitespace) && !host.contains('/')
    }
}

/// The path a rule is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PathMatch {
    /// This path exactly, compared with regard to case.
    Exact(String),
    /// Every path that starts with this one.
    ///
    /// `/assets` matches `/assets`, `/assets/logo.png`, and `/assetsX`: it is
    /// a string prefix and not a path-segment prefix. A rule that means the
    /// directory writes the trailing slash.
    Prefix(String),
}

impl PathMatch {
    /// Whether `path` is one this names.
    pub fn matches(&self, path: &str) -> bool {
        match self {
            // A path is case-sensitive, so this comparison is too.
            Self::Exact(wanted) => wanted == path,
            Self::Prefix(prefix) => path.starts_with(prefix.as_str()),
        }
    }

    /// The path as written.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(path) | Self::Prefix(path) => path,
        }
    }

    /// Whether this names a path at all.
    ///
    /// A path begins with `/`: anything else is a path that was meant to be
    /// a URL, and matching it literally would match nothing for ever.
    pub fn is_valid(&self) -> bool {
        let path = self.as_str();

        path.starts_with('/') && !path.contains(char::is_whitespace)
    }
}

/// The method a rule is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MethodMatch {
    /// This method exactly, compared without regard to case.
    Exact(String),
}

impl MethodMatch {
    /// The method `method` exactly.
    pub fn exact(method: impl Into<String>) -> Self {
        Self::Exact(method.into())
    }

    /// Whether `method` is the one this names.
    pub fn matches(&self, method: &str) -> bool {
        match self {
            Self::Exact(wanted) => wanted.eq_ignore_ascii_case(method),
        }
    }

    /// The method as written.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(method) => method,
        }
    }

    /// Whether this names a method at all.
    ///
    /// A method is one HTTP token: no spaces, no separators, not empty.
    pub fn is_valid(&self) -> bool {
        let method = self.as_str();

        !method.is_empty()
            && method
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    }
}
