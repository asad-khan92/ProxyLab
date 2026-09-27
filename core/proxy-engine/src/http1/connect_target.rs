//! Where a `CONNECT` request wants a tunnel to.
//!
//! A `CONNECT` request carries an authority-form request target, which RFC
//! 9112 §3.2.3 defines as a host and a port and nothing else:
//!
//! ```text
//! CONNECT example.com:443 HTTP/1.1
//! ```
//!
//! This module is the gate in front of the tunnel: it turns that target into
//! the host and port to dial, and refuses everything that is not authority
//! form. Nothing downstream has to second-guess the target it is given.

use std::fmt;

use hyper::{StatusCode, Uri};

/// The destination of a `CONNECT` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectTarget {
    /// Host to dial, without the brackets of an IPv6 literal.
    host: String,
    port: u16,
}

impl ConnectTarget {
    /// Reads the destination out of an authority-form request target.
    ///
    /// Both the host and the port must be present and the port must be a port
    /// a TCP connection can be made to, so a target that only looks close
    /// enough is refused rather than guessed at.
    pub fn parse(uri: &Uri) -> Result<Self, ConnectTargetError> {
        // Authority form carries a host and a port, so a scheme or a path
        // means the client sent some other form of request target: an
        // absolute URL, an origin-form path, or the asterisk form.
        if uri.scheme_str().is_some() || uri.path_and_query().is_some() {
            return Err(ConnectTargetError::NotAuthorityForm);
        }

        let authority = uri
            .authority()
            .map(|authority| authority.as_str())
            .ok_or(ConnectTargetError::NotAuthorityForm)?;

        // Userinfo is part of a URL authority but not of authority form, and
        // a tunnel has nowhere to send credentials, so it is refused instead
        // of being quietly dropped.
        if authority.contains('@') {
            return Err(ConnectTargetError::NotAuthorityForm);
        }

        let (host, port) = split_host_port(authority)?;

        // An IPv6 literal is bracketed in a request target but must not be
        // when it is used as a socket address.
        let host = host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(host);
        if host.is_empty() {
            return Err(ConnectTargetError::MissingHost);
        }

        if port.is_empty() {
            return Err(ConnectTargetError::MissingPort);
        }

        // `u16::from_str` accepts a leading `+`, which is not a port, and
        // port 0 asks the operating system for a local port rather than
        // naming a remote one, so neither is a destination.
        let port: u16 = port
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| port.parse().ok())
            .flatten()
            .filter(|port| *port != 0)
            .ok_or(ConnectTargetError::InvalidPort)?;

        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }

    /// A destination whose host and port were already checked elsewhere.
    ///
    /// For a transparently captured flow, whose host is the one the client
    /// named in its TLS hello rather than one written in a request target:
    /// it has been through [`proxy_tls::Host`] already, and the port came
    /// from a target this type parsed.
    pub(crate) fn named(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
        }
    }

    /// Host to open the upstream TCP connection to, without IPv6 brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Port to open the upstream TCP connection to.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// The target as it was asked for, which is what logs should name.
impl fmt::Display for ConnectTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Splits an authority into its host and port halves.
///
/// The colon that separates them is the last one, except in an IPv6 literal,
/// where the colons inside the brackets belong to the address.
fn split_host_port(authority: &str) -> Result<(&str, &str), ConnectTargetError> {
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or(ConnectTargetError::NotAuthorityForm)?;
        let (host, rest) = authority.split_at(close + 1);

        return match rest.strip_prefix(':') {
            Some(port) => Ok((host, port)),
            // Anything else after the brackets is not a port.
            None if rest.is_empty() => Err(ConnectTargetError::MissingPort),
            None => Err(ConnectTargetError::NotAuthorityForm),
        };
    }

    authority
        .rsplit_once(':')
        .ok_or(ConnectTargetError::MissingPort)
}

/// Why a `CONNECT` target cannot be tunnelled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectTargetError {
    /// The target was not authority form, so it names no tunnel destination.
    NotAuthorityForm,
    /// The target named no host.
    MissingHost,
    /// The target named no port.
    MissingPort,
    /// The port is not a port a TCP connection can be made to.
    InvalidPort,
}

impl ConnectTargetError {
    /// Status to answer the client with.
    ///
    /// Every failure here is a request the client got wrong, which is what
    /// `400 Bad Request` reports.
    pub fn status(self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }

    /// Explanation safe to send to the client.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotAuthorityForm => "CONNECT requires a host:port request target",
            Self::MissingHost => "the CONNECT target names no host",
            Self::MissingPort => "the CONNECT target names no port",
            Self::InvalidPort => "the CONNECT target port is not a valid port number",
        }
    }
}

impl fmt::Display for ConnectTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(target: &str) -> ConnectTarget {
        let uri: Uri = target.parse().expect("valid uri");
        ConnectTarget::parse(&uri).expect("tunnellable target")
    }

    fn error(target: &str) -> ConnectTargetError {
        let uri: Uri = target.parse().expect("valid uri");
        ConnectTarget::parse(&uri).expect_err("target cannot be tunnelled to")
    }

    #[test]
    fn reads_the_host_and_port_of_an_authority_form_target() {
        let target = target("example.com:443");

        assert_eq!(target.host(), "example.com");
        assert_eq!(target.port(), 443);
    }

    #[test]
    fn an_ip_literal_is_a_host_like_any_other() {
        let target = target("127.0.0.1:8443");

        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 8443);
    }

    #[test]
    fn an_ipv6_literal_loses_its_brackets_only_for_connecting() {
        let target = target("[::1]:443");

        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 443);
        assert_eq!(target.to_string(), "[::1]:443");
    }

    #[test]
    fn the_highest_port_is_still_a_port() {
        assert_eq!(target("example.com:65535").port(), 65535);
    }

    #[test]
    fn the_target_names_itself_the_way_the_client_asked_for_it() {
        assert_eq!(target("example.com:443").to_string(), "example.com:443");
    }

    #[test]
    fn a_target_with_no_port_is_refused() {
        assert_eq!(error("example.com"), ConnectTargetError::MissingPort);
        assert_eq!(error("[::1]"), ConnectTargetError::MissingPort);
    }

    #[test]
    fn a_target_with_an_empty_port_is_refused() {
        assert_eq!(error("example.com:"), ConnectTargetError::MissingPort);
        assert_eq!(error("[::1]:"), ConnectTargetError::MissingPort);
    }

    #[test]
    fn a_target_with_no_host_is_refused() {
        assert_eq!(error(":443"), ConnectTargetError::MissingHost);
    }

    #[test]
    fn a_port_that_is_not_a_number_is_refused() {
        assert_eq!(error("example.com:https"), ConnectTargetError::InvalidPort);
        assert_eq!(error("example.com:44a"), ConnectTargetError::InvalidPort);
        assert_eq!(error("example.com:+443"), ConnectTargetError::InvalidPort);
    }

    #[test]
    fn a_port_outside_the_tcp_range_is_refused() {
        assert_eq!(error("example.com:65536"), ConnectTargetError::InvalidPort);
        assert_eq!(error("example.com:99999"), ConnectTargetError::InvalidPort);
        assert_eq!(error("example.com:0"), ConnectTargetError::InvalidPort);
    }

    #[test]
    fn an_origin_form_target_is_not_authority_form() {
        assert_eq!(error("/index.html"), ConnectTargetError::NotAuthorityForm);
    }

    #[test]
    fn an_asterisk_form_target_is_not_authority_form() {
        assert_eq!(error("*"), ConnectTargetError::NotAuthorityForm);
    }

    #[test]
    fn an_absolute_url_is_not_authority_form() {
        assert_eq!(
            error("http://example.com/"),
            ConnectTargetError::NotAuthorityForm
        );
        assert_eq!(
            error("https://example.com:443"),
            ConnectTargetError::NotAuthorityForm
        );
    }

    #[test]
    fn userinfo_in_the_target_is_refused_rather_than_dropped() {
        assert_eq!(
            error("user:secret@example.com:443"),
            ConnectTargetError::NotAuthorityForm
        );
    }

    #[test]
    fn every_malformed_target_is_a_bad_request() {
        for error in [
            ConnectTargetError::NotAuthorityForm,
            ConnectTargetError::MissingHost,
            ConnectTargetError::MissingPort,
            ConnectTargetError::InvalidPort,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn every_rejection_explains_itself_without_leaking_internals() {
        for error in [
            ConnectTargetError::NotAuthorityForm,
            ConnectTargetError::MissingHost,
            ConnectTargetError::MissingPort,
            ConnectTargetError::InvalidPort,
        ] {
            assert!(!error.reason().is_empty());
            assert_eq!(error.to_string(), error.reason());
        }
    }
}
