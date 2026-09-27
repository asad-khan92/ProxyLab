//! Where a proxy request is going.
//!
//! A proxy request carries an absolute-form request target:
//!
//! ```text
//! GET http://example.com/api/users?active=true HTTP/1.1
//! ```
//!
//! This module turns that target into the three things forwarding needs: the
//! host and port to connect to, the `Host` header the origin must be given,
//! and the origin-form target to put on the request line.

use std::fmt;

use hyper::header::HeaderValue;
use hyper::http::uri::PathAndQuery;
use hyper::{StatusCode, Uri};

/// Port used when an `http` URL does not name one.
const DEFAULT_HTTP_PORT: u16 = 80;

/// The destination of a proxy request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    host: String,
    port: u16,
    host_header: HeaderValue,
    origin_form: Uri,
}

impl Target {
    /// Reads the destination out of an absolute-form request target.
    ///
    /// Only `http` is accepted. `https` needs `CONNECT` and TLS interception,
    /// which are later phases, so it is reported as unsupported rather than
    /// attempted over a plain socket.
    pub fn parse(uri: &Uri) -> Result<Self, TargetError> {
        let scheme = uri.scheme_str().ok_or(TargetError::NotAbsoluteForm)?;
        if !scheme.eq_ignore_ascii_case("http") {
            return Err(TargetError::UnsupportedScheme);
        }

        let authority = uri.authority().ok_or(TargetError::NotAbsoluteForm)?;
        let host = authority.host();
        if host.is_empty() {
            return Err(TargetError::MissingHost);
        }

        // `Authority::host` keeps the brackets around an IPv6 literal, which
        // belong in the `Host` header but not in a socket address.
        let connect_host = host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(host);

        // Userinfo is part of the authority but must never reach the origin in
        // a `Host` header, so the header is rebuilt from host and port. The
        // port is repeated only when the client spelled it out, so the origin
        // sees the same `Host` the client asked for.
        let host_header = match authority.port_u16() {
            Some(port) => HeaderValue::from_str(&format!("{host}:{port}")),
            None => HeaderValue::from_str(host),
        }
        .map_err(|_| TargetError::UnusableHost)?;

        let path_and_query = uri
            .path_and_query()
            .filter(|path_and_query| !path_and_query.as_str().is_empty())
            .cloned()
            .unwrap_or_else(|| PathAndQuery::from_static("/"));

        Ok(Self {
            host: connect_host.to_owned(),
            port: authority.port_u16().unwrap_or(DEFAULT_HTTP_PORT),
            host_header,
            origin_form: Uri::from(path_and_query),
        })
    }

    /// Host to open the upstream TCP connection to, without IPv6 brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Port to open the upstream TCP connection to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Value the origin's `Host` header must carry.
    pub fn host_header(&self) -> &HeaderValue {
        &self.host_header
    }

    /// The request target as the origin expects it: path and query only.
    pub fn origin_form(&self) -> &Uri {
        &self.origin_form
    }
}

/// Why a request target cannot be forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetError {
    /// The target was not an absolute URL, so there is nowhere to forward it.
    NotAbsoluteForm,
    /// The URL scheme is not one this proxy handles.
    UnsupportedScheme,
    /// The URL named no host.
    MissingHost,
    /// The host cannot be expressed as a `Host` header value.
    UnusableHost,
}

impl TargetError {
    /// Status to answer the client with.
    pub fn status(self) -> StatusCode {
        match self {
            Self::NotAbsoluteForm | Self::MissingHost | Self::UnusableHost => {
                StatusCode::BAD_REQUEST
            }
            Self::UnsupportedScheme => StatusCode::NOT_IMPLEMENTED,
        }
    }

    /// Explanation safe to send to the client.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotAbsoluteForm => "proxy requests must use an absolute URL",
            Self::UnsupportedScheme => "only http:// URLs are supported",
            Self::MissingHost => "the request URL names no host",
            Self::UnusableHost => "the request URL host cannot be sent to the origin",
        }
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(uri: &str) -> Target {
        let uri: Uri = uri.parse().expect("valid uri");
        Target::parse(&uri).expect("forwardable target")
    }

    fn error(uri: &str) -> TargetError {
        let uri: Uri = uri.parse().expect("valid uri");
        Target::parse(&uri).expect_err("target cannot be forwarded")
    }

    #[test]
    fn reads_host_and_default_port_from_an_absolute_url() {
        let target = target("http://example.com/index.html");

        assert_eq!(target.host(), "example.com");
        assert_eq!(target.port(), 80);
    }

    #[test]
    fn an_explicit_port_wins_over_the_default() {
        let target = target("http://127.0.0.1:8080/hello");

        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 8080);
    }

    #[test]
    fn the_request_line_keeps_the_path_and_query() {
        let target = target("http://example.com/api/users?active=true");

        assert_eq!(target.origin_form(), "/api/users?active=true");
    }

    #[test]
    fn a_url_with_no_path_asks_the_origin_for_the_root() {
        assert_eq!(target("http://example.com").origin_form(), "/");
    }

    #[test]
    fn the_host_header_repeats_a_port_the_client_spelled_out() {
        assert_eq!(
            target("http://example.com:8080/x").host_header(),
            "example.com:8080"
        );
    }

    #[test]
    fn the_host_header_omits_a_port_the_client_left_out() {
        assert_eq!(target("http://example.com/x").host_header(), "example.com");
    }

    #[test]
    fn userinfo_is_kept_out_of_the_host_header() {
        let target = target("http://user:secret@example.com/x");

        assert_eq!(target.host(), "example.com");
        assert_eq!(target.host_header(), "example.com");
    }

    #[test]
    fn an_ipv6_literal_loses_its_brackets_only_for_connecting() {
        let target = target("http://[::1]:8080/x");

        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 8080);
        assert_eq!(target.host_header(), "[::1]:8080");
    }

    #[test]
    fn an_origin_form_target_is_not_forwardable() {
        assert_eq!(error("/index.html"), TargetError::NotAbsoluteForm);
        assert_eq!(error("/index.html").status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn https_is_reported_as_unsupported_rather_than_sent_in_the_clear() {
        assert_eq!(
            error("https://example.com/x"),
            TargetError::UnsupportedScheme
        );
        assert_eq!(
            error("https://example.com/x").status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }

    #[test]
    fn other_schemes_are_unsupported_too() {
        assert_eq!(error("ftp://example.com/x"), TargetError::UnsupportedScheme);
    }

    #[test]
    fn an_authority_form_target_is_not_forwardable() {
        assert_eq!(error("example.com:443"), TargetError::NotAbsoluteForm);
    }

    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        assert_eq!(target("HTTP://example.com/x").host(), "example.com");
    }

    #[test]
    fn every_rejection_explains_itself_without_leaking_internals() {
        for error in [
            TargetError::NotAbsoluteForm,
            TargetError::UnsupportedScheme,
            TargetError::MissingHost,
            TargetError::UnusableHost,
        ] {
            assert!(!error.reason().is_empty());
            assert_eq!(error.to_string(), error.reason());
        }
    }
}
