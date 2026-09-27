//! What a rule does to a request it matches.
//!
//! ```text
//! Allow          carry it as usual
//! Block          answer it here, with a status, and never contact the origin
//! MapLocal       answer it with the contents of a configured file
//! MapRemote      send it to a different destination
//! Redirect       answer it with a redirect to a configured URL
//!
//! ModifyRequest  change it on its way out        (transformations, which
//! ModifyResponse change its answer on the way back  decide nothing — see
//!                                                    `crate::modify`)
//! ```
//!
//! An action is a *description* of what to do, never the doing of it. A
//! `MapLocal` holds a path and does not open it; a `MapRemote` holds a
//! destination and does not connect to it. The proxy pipeline carries the
//! action out — see `proxy-engine` — which is what keeps rule evaluation
//! free of I/O.
//!
//! # Map Remote, exactly
//!
//! The mapping is a URL, and which parts of the request it replaces is
//! decided when the rule is created, not when it runs:
//!
//! ```text
//! rule target                 GET https://api.example.com/foo?a=1 becomes
//! ───────────────────────     ──────────────────────────────────────────
//! https://staging.example.com  https://staging.example.com/foo?a=1
//! https://staging:8443         https://staging:8443/foo?a=1
//! http://staging.example.com   http://staging.example.com/foo?a=1
//! https://staging.example.com/v2   https://staging.example.com/v2?a=1
//! https://staging.example.com/v2?b=2   https://staging.example.com/v2?b=2
//! ```
//!
//! So: **scheme, host, and port always come from the rule.** The **path** is
//! the request's own unless the rule names one, and the **query** is the
//! request's own unless the rule names one. A rule that names only an
//! authority is the common case — "the same API, somewhere else" — and it
//! preserves everything about the request but where it goes.
//!
//! The mapping is parsed once, when the rule is created, into a
//! [`RemoteTarget`] whose parts are already separated. Nothing concatenates
//! URLs as strings at request time, which is how
//! `https://staging.example.comhttps://api.example.com/foo` happens.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::modify::{ModifyRequest, ModifyResponse};

/// The status a `Block` answers with when the rule does not choose one.
///
/// The request was understood and the proxy is refusing to carry it, which
/// is what `403 Forbidden` says. It is not `404`, which would claim the
/// origin has no such resource, and not `502`, which would claim the origin
/// was asked and failed.
pub const DEFAULT_BLOCK_STATUS: u16 = 403;

/// The most of a mapped local file that is read into one response.
///
/// A generated response is built in memory — see `proxy-engine`'s
/// `map_local` — so this is the bound on what one request can make the proxy
/// hold. Large enough for the fixtures, bundles, and recorded payloads a
/// developer maps in; small enough that pointing a rule at a disk image is
/// refused rather than attempted.
pub const MAX_MAP_LOCAL_BYTES: u64 = 16 * 1024 * 1024;

/// What a rule does with a request it matches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleAction {
    /// Carry the request as the proxy would have anyway.
    ///
    /// Not a no-op: an `Allow` rule at a lower priority number is how one
    /// request is excepted from a `Block` that would otherwise cover it.
    Allow,
    /// Answer the request here, without contacting the origin.
    Block(BlockAction),
    /// Answer the request with the contents of a file on this machine.
    MapLocal(MapLocal),
    /// Send the request somewhere else.
    MapRemote(MapRemote),
    /// Answer the request with a redirect, without contacting the origin.
    Redirect(Redirect),
    /// Change the request before it is sent. Not a decision: see
    /// [`is_transformation`](Self::is_transformation).
    ModifyRequest(ModifyRequest),
    /// Change the response before the client is given it. Not a decision
    /// either.
    ModifyResponse(ModifyResponse),
}

impl RuleAction {
    /// A block with the default status.
    pub fn block() -> Self {
        Self::Block(BlockAction::default())
    }

    /// A block answering with `status`.
    pub fn block_with(status: u16) -> Self {
        Self::Block(BlockAction { status })
    }

    /// A mapping to the file at `path`.
    pub fn map_local(path: impl Into<PathBuf>) -> Self {
        Self::MapLocal(MapLocal::new(path))
    }

    /// A mapping to `url`, or why that is not a destination.
    pub fn map_remote(url: &str) -> Result<Self, RuleActionError> {
        Ok(Self::MapRemote(MapRemote::parse(url)?))
    }

    /// A redirect to `url` with `status`, or why that is not one.
    pub fn redirect(url: &str, status: u16) -> Result<Self, RuleActionError> {
        let redirect = Redirect {
            target: RemoteTarget::parse(url).map_err(|_| RuleActionError::RedirectTarget)?,
            status,
        };
        redirect.validate()?;

        Ok(Self::Redirect(redirect))
    }

    /// What this action is called, for a log line.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block(_) => "block",
            Self::MapLocal(_) => "map_local",
            Self::MapRemote(_) => "map_remote",
            Self::Redirect(_) => "redirect",
            Self::ModifyRequest(_) => "modify_request",
            Self::ModifyResponse(_) => "modify_response",
        }
    }

    /// Whether this action changes a message rather than deciding what
    /// happens to it.
    ///
    /// A transformation that matches is applied and the rules below it are
    /// still consulted; every other action that matches is the decision, and
    /// nothing below it is. See [`crate::engine`].
    pub fn is_transformation(&self) -> bool {
        matches!(self, Self::ModifyRequest(_) | Self::ModifyResponse(_))
    }

    /// Whether this action is one this build can carry out.
    ///
    /// Checked when a rule is created and again when one is read back from
    /// the database, so a row that was edited by hand cannot put an action
    /// into the engine that the pipeline has no answer for.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        match self {
            Self::Allow => Ok(()),
            Self::Block(block) => block.validate(),
            Self::MapLocal(map) => map.validate(),
            Self::MapRemote(map) => map.target.validate(),
            Self::Redirect(redirect) => redirect.validate(),
            Self::ModifyRequest(modify) => modify.validate(),
            Self::ModifyResponse(modify) => modify.validate(),
        }
    }
}

/// The statuses a redirect may answer with.
///
/// The four that mean "go there instead": `301` and `308` permanently, `302`
/// and `307` for now — `307` and `308` keeping the method and the body, `301`
/// and `302` letting a client turn a `POST` into a `GET`, as clients do.
pub const REDIRECT_STATUSES: [u16; 4] = [301, 302, 307, 308];

/// The status a redirect answers with when the rule does not choose one.
///
/// Temporary, because a rule is: `301` is cached by browsers, and a rule
/// someone turned off an hour later would still be followed.
pub const DEFAULT_REDIRECT_STATUS: u16 = 302;

/// Answering a request with a redirect to somewhere else.
///
/// The client is sent to `target` exactly as it is written — no part of the
/// request's own URL is carried over — and follows it or not, as it chooses.
/// Nothing is sent to the origin, and one redirect is one answer: whatever
/// the client asks for next is a request of its own, matched like any other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redirect {
    /// Where the client is sent.
    pub target: RemoteTarget,
    /// The redirect status.
    #[serde(default = "default_redirect_status")]
    pub status: u16,
}

fn default_redirect_status() -> u16 {
    DEFAULT_REDIRECT_STATUS
}

impl Redirect {
    /// The `Location` the client is given.
    pub fn location(&self) -> String {
        self.target.to_string()
    }

    /// Whether this is a redirect the proxy can answer with.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if !REDIRECT_STATUSES.contains(&self.status) {
            return Err(RuleActionError::RedirectStatus);
        }

        self.target
            .validate()
            .map_err(|_| RuleActionError::RedirectTarget)
    }
}

/// Answering a request here rather than carrying it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockAction {
    /// The status the client is answered with.
    #[serde(default = "default_block_status")]
    pub status: u16,
}

fn default_block_status() -> u16 {
    DEFAULT_BLOCK_STATUS
}

impl Default for BlockAction {
    fn default() -> Self {
        Self {
            status: DEFAULT_BLOCK_STATUS,
        }
    }
}

impl BlockAction {
    /// Whether the status is one an HTTP response can carry.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if (100..=599).contains(&self.status) {
            Ok(())
        } else {
            Err(RuleActionError::BlockStatus)
        }
    }
}

/// Answering a request with a file on this machine.
///
/// The path is the rule's, and only the rule's. Nothing from the request
/// reaches it: no part of the URL is appended to it, joined with it, or used
/// to pick a file inside it. That is the whole of Map Local's filesystem
/// exposure — one configured path per rule — and it is why there is no
/// traversal to defend against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapLocal {
    /// The file to answer with.
    pub path: PathBuf,
    /// The content type to answer with, or `None` to work it out from the
    /// file's extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The status to answer with.
    #[serde(default = "default_map_local_status")]
    pub status: u16,
}

fn default_map_local_status() -> u16 {
    200
}

impl MapLocal {
    /// A mapping to the file at `path`, answered as `200`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            content_type: None,
            status: default_map_local_status(),
        }
    }

    /// The same mapping, answered with `content_type`.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// The file this mapping names.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this names a file that could be read, and a usable response.
    ///
    /// Whether the file *is* there is not asked here, and deliberately: a
    /// rule is validated when it is written, the file is checked when the
    /// rule runs, and a rule that stops working because a file was deleted
    /// is a runtime failure rather than an invalid rule.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if self.path.as_os_str().is_empty() {
            return Err(RuleActionError::MapLocalPathEmpty);
        }
        // Relative to what? A proxy has no meaningful working directory, and
        // the one it happens to have started in is not something a rule
        // should silently depend on.
        if !self.path.is_absolute() {
            return Err(RuleActionError::MapLocalPathNotAbsolute);
        }
        if !(100..=599).contains(&self.status) {
            return Err(RuleActionError::MapLocalStatus);
        }
        if let Some(content_type) = &self.content_type {
            if content_type.is_empty() || content_type.bytes().any(|byte| byte < 0x20) {
                return Err(RuleActionError::MapLocalContentType);
            }
        }

        Ok(())
    }
}

/// Sending a request somewhere other than where it was addressed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapRemote {
    /// Where the request goes instead.
    pub target: RemoteTarget,
}

impl MapRemote {
    /// A mapping to `url`, or why that is not a destination.
    pub fn parse(url: &str) -> Result<Self, RuleActionError> {
        Ok(Self {
            target: RemoteTarget::parse(url)?,
        })
    }
}

/// A destination a request can be mapped to.
///
/// Parsed once, when the rule is created, and kept in parts. The pipeline
/// builds the outgoing URL from these parts and the request's own path and
/// query — see [`RemoteTarget::rewrite`] — so no URL is ever assembled by
/// pasting two strings together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteTarget {
    /// `http` or `https`, lowercased.
    pub scheme: RemoteScheme,
    /// The host, without brackets around an IPv6 literal.
    pub host: String,
    /// The port, or `None` for the scheme's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The path the rule names, or `None` to keep the request's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The query the rule names, or `None` to keep the request's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// The schemes a request may be mapped to.
///
/// Two, and closed: a mapping to `file:` or `ftp:` is not a thing this proxy
/// could carry out, and accepting one would be a rule that fails at request
/// time instead of at the moment it was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteScheme {
    /// Plaintext HTTP.
    Http,
    /// HTTP inside a TLS session the proxy opens and verifies.
    Https,
}

impl RemoteScheme {
    /// The port this scheme implies.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }

    /// The scheme as it is written in a URL.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

impl fmt::Display for RemoteScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl RemoteTarget {
    /// Reads a destination out of `url`, or says why it is not one.
    ///
    /// Deliberately strict, and parsed by hand rather than by a URL library:
    /// what is accepted is `scheme://host[:port][/path][?query]`, with no
    /// userinfo and no fragment. A mapping is a destination a developer
    /// typed, and the things a lenient parser would quietly accept —
    /// credentials in the authority, a scheme this proxy cannot speak — are
    /// exactly the ones worth refusing at the moment the rule is written.
    pub fn parse(url: &str) -> Result<Self, RuleActionError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(RuleActionError::RemoteUrlEmpty);
        }
        if url.bytes().any(|byte| byte <= 0x20 || byte == 0x7F) {
            return Err(RuleActionError::RemoteUrlInvalid);
        }

        let (scheme, rest) = url
            .split_once("://")
            .ok_or(RuleActionError::RemoteUrlNoScheme)?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" => RemoteScheme::Http,
            "https" => RemoteScheme::Https,
            _ => return Err(RuleActionError::RemoteSchemeUnsupported),
        };

        // A fragment is never sent to a server, so a mapping carrying one is
        // a mapping whose author expected something that cannot happen.
        if rest.contains('#') {
            return Err(RuleActionError::RemoteUrlFragment);
        }

        let (authority, path_and_query) = match rest.find(['/', '?']) {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };

        // Userinfo would be credentials sitting in a rule and then sent to a
        // server, and it is never what a mapping means.
        if authority.contains('@') {
            return Err(RuleActionError::RemoteUrlUserinfo);
        }

        let (host, port) = split_authority(authority)?;
        let (path, query) = split_path_and_query(path_and_query)?;

        let target = Self {
            scheme,
            host,
            port,
            path,
            query,
        };
        target.validate()?;

        Ok(target)
    }

    /// The port a connection to this destination is opened to.
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.scheme.default_port())
    }

    /// The `Host` header a request to this destination must carry.
    ///
    /// The port is named only when it is not the scheme's own, which is what
    /// a client would have sent had it addressed this destination itself.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        match self.port {
            Some(port) if port != self.scheme.default_port() => format!("{host}:{port}"),
            _ => host,
        }
    }

    /// The path and query a request to `path`/`query` is rewritten to.
    ///
    /// The rule's path and query win where it names them, and the request's
    /// are kept where it does not. Returned as one string in origin form,
    /// which is what goes on the request line.
    pub fn rewrite(&self, path: &str, query: Option<&str>) -> String {
        let path = match &self.path {
            Some(mapped) => mapped.as_str(),
            None => path,
        };
        let query = match &self.query {
            Some(mapped) => Some(mapped.as_str()),
            // A rule that names a path but no query replaces the path and
            // keeps the query: `?debug=1` survives being pointed at staging.
            None => query,
        };

        match query {
            Some(query) => format!("{path}?{query}"),
            None => path.to_owned(),
        }
    }

    /// Whether this is a destination a connection could be opened to.
    pub fn validate(&self) -> Result<(), RuleActionError> {
        if self.host.is_empty() {
            return Err(RuleActionError::RemoteHostEmpty);
        }
        if self
            .host
            .bytes()
            .any(|byte| byte <= 0x20 || byte == 0x7F || b"/?#@\\".contains(&byte))
        {
            return Err(RuleActionError::RemoteHostInvalid);
        }
        if self.port == Some(0) {
            return Err(RuleActionError::RemotePortInvalid);
        }
        if let Some(path) = &self.path {
            if !path.starts_with('/') || path.contains(char::is_whitespace) {
                return Err(RuleActionError::RemotePathInvalid);
            }
        }
        if let Some(query) = &self.query {
            if query.contains(char::is_whitespace) {
                return Err(RuleActionError::RemoteQueryInvalid);
            }
        }

        Ok(())
    }
}

/// The destination as a URL, without the path a request supplies.
impl fmt::Display for RemoteTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host_header())?;
        if let Some(path) = &self.path {
            f.write_str(path)?;
        }
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }

        Ok(())
    }
}

/// Splits `host:port`, keeping an IPv6 literal in one piece.
fn split_authority(authority: &str) -> Result<(String, Option<u16>), RuleActionError> {
    if authority.is_empty() {
        return Err(RuleActionError::RemoteHostEmpty);
    }

    // `[::1]:8443` — the brackets are the URL's way of saying the colons in
    // the middle belong to the address and not to a port.
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or(RuleActionError::RemoteHostInvalid)?;

        return Ok((host.to_owned(), parse_port(after)?));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => Ok((host.to_owned(), parse_port(&format!(":{port}"))?)),
        None => Ok((authority.to_owned(), None)),
    }
}

/// Reads the `:port` that may follow an authority.
fn parse_port(rest: &str) -> Result<Option<u16>, RuleActionError> {
    if rest.is_empty() {
        return Ok(None);
    }

    let port = rest
        .strip_prefix(':')
        .ok_or(RuleActionError::RemoteUrlInvalid)?;

    port.parse::<u16>()
        .map(Some)
        .map_err(|_| RuleActionError::RemotePortInvalid)
}

/// Splits the `/path?query` a mapping may carry.
///
/// A mapping of `https://host` and one of `https://host/` mean the same
/// thing — keep the request's path — because a bare slash is what a URL
/// writes when it names no path at all.
fn split_path_and_query(rest: &str) -> Result<(Option<String>, Option<String>), RuleActionError> {
    if rest.is_empty() {
        return Ok((None, None));
    }

    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query.to_owned())),
        None => (rest, None),
    };

    let path = match path {
        "" | "/" => None,
        path => Some(path.to_owned()),
    };

    Ok((path, query))
}

/// Why an action is not one this build can carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleActionError {
    /// A block's status is not one an HTTP response can carry.
    BlockStatus,
    /// A map-local rule names no file.
    MapLocalPathEmpty,
    /// A map-local path is relative, and there is nothing to resolve it
    /// against.
    MapLocalPathNotAbsolute,
    /// A map-local status is not one an HTTP response can carry.
    MapLocalStatus,
    /// A map-local content type is not one a header can carry.
    MapLocalContentType,
    /// A map-remote URL is empty.
    RemoteUrlEmpty,
    /// A map-remote URL names no scheme.
    RemoteUrlNoScheme,
    /// A map-remote URL names a scheme this proxy cannot speak.
    RemoteSchemeUnsupported,
    /// A map-remote URL carries characters a URL cannot.
    RemoteUrlInvalid,
    /// A map-remote URL carries a fragment, which is never sent to a server.
    RemoteUrlFragment,
    /// A map-remote URL carries userinfo.
    RemoteUrlUserinfo,
    /// A map-remote URL names no host.
    RemoteHostEmpty,
    /// A map-remote host is not one a connection could be opened to.
    RemoteHostInvalid,
    /// A map-remote port is not a port.
    RemotePortInvalid,
    /// A map-remote path is not a path.
    RemotePathInvalid,
    /// A map-remote query is not a query.
    RemoteQueryInvalid,
    /// A redirect target is not an http or https URL.
    RedirectTarget,
    /// A redirect status is not one of the four that redirect.
    RedirectStatus,
    /// A transformation that changes nothing.
    ModifyNothing,
    /// A transformation's method is not a method.
    ModifyMethod,
    /// A transformation's path is not a path.
    ModifyPath,
    /// A query edit names or sets something a query cannot carry.
    ModifyQuery,
    /// A header edit names something that is not a header name.
    ModifyHeaderName,
    /// A header edit names a header the proxy keeps for itself.
    ModifyHeaderProtected,
    /// A header edit's value is not one a header can carry.
    ModifyHeaderValue,
    /// A transformation's response status is not a final status.
    ModifyStatus,
    /// A transformation carries more edits of one kind than it may.
    ModifyTooMany,
    /// A body edit's text is longer than a rule may carry.
    ModifyBodyTooLong,
    /// A JSON body edit names no field.
    ModifyJsonPath,
    /// A JSON body edit's value is not JSON.
    ModifyJsonValue,
}

impl RuleActionError {
    /// What went wrong, in words that name nothing the caller supplied.
    pub fn reason(self) -> &'static str {
        match self {
            Self::BlockStatus => "a block status must be between 100 and 599",
            Self::MapLocalPathEmpty => "a map-local rule must name a file",
            Self::MapLocalPathNotAbsolute => "a map-local path must be absolute",
            Self::MapLocalStatus => "a map-local status must be between 100 and 599",
            Self::MapLocalContentType => "a map-local content type must be a header value",
            Self::RemoteUrlEmpty => "a map-remote rule must name a destination",
            Self::RemoteUrlNoScheme => "a map-remote destination must name http or https",
            Self::RemoteSchemeUnsupported => "a map-remote destination must be http or https",
            Self::RemoteUrlInvalid => "a map-remote destination is not a URL",
            Self::RemoteUrlFragment => "a map-remote destination must not carry a fragment",
            Self::RemoteUrlUserinfo => "a map-remote destination must not carry credentials",
            Self::RemoteHostEmpty => "a map-remote destination must name a host",
            Self::RemoteHostInvalid => "a map-remote host is not a host",
            Self::RemotePortInvalid => "a map-remote port is not a port",
            Self::RemotePathInvalid => "a map-remote path must start with a slash",
            Self::RemoteQueryInvalid => "a map-remote query is not a query",
            Self::RedirectTarget => "a redirect target must be an http or https URL with a host",
            Self::RedirectStatus => "a redirect status must be 301, 302, 307 or 308",
            Self::ModifyNothing => "a modify rule must change something",
            Self::ModifyMethod => "a modify method must be one HTTP token",
            Self::ModifyPath => "a modify path must start with a slash and carry no query",
            Self::ModifyQuery => "a modify query parameter must have a name, and no spaces, & or #",
            Self::ModifyHeaderName => "a modify header name must be one HTTP token",
            Self::ModifyHeaderProtected => {
                "a modify header cannot be Host, Content-Length, Transfer-Encoding \
                 or a hop-by-hop header"
            }
            Self::ModifyHeaderValue => "a modify header value must not contain line breaks",
            Self::ModifyStatus => "a modify status must be between 200 and 599",
            Self::ModifyTooMany => "a modify rule may carry at most 32 edits of each kind",
            Self::ModifyBodyTooLong => "a modify body must be at most 32 KiB; use Map Local",
            Self::ModifyJsonPath => "a modify JSON field is keys joined by dots",
            Self::ModifyJsonValue => "a modify JSON value must be JSON",
        }
    }
}

impl fmt::Display for RuleActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for RuleActionError {}
