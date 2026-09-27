//! Request targets in the form it is safe to write to a log.
//!
//! A URL may carry credentials in its authority:
//!
//! ```text
//! http://alice:hunter2@example.com/orders
//! ```
//!
//! A client should not put them there, and most no longer do, but a proxy
//! reports what it is asked for and a request target is whatever the client
//! sent. A log is read over shoulders, pasted into issues, and shipped off
//! the machine, so what goes in one is:
//!
//! ```text
//! http://[redacted]@example.com/orders
//! ```
//!
//! Redacted, not removed: the line still says the request carried
//! credentials, which is the thing a developer chasing a `401` needs to see.
//!
//! # Capture is not logging
//!
//! Nothing here touches what is captured. The captured URL is built
//! separately, in [`crate::capture::message`], and the capture keeps the
//! complete debugging picture while the log keeps the sanitised one.

use hyper::Uri;

/// What stands in for the userinfo of a redacted URL.
const REDACTED_USERINFO: &str = "[redacted]";

/// `uri` as a log line should carry it.
///
/// Only the userinfo is replaced; the scheme, host, port, path, and query are
/// what the client asked for, because that is what a log is for. A target
/// with no userinfo is written as it was sent.
///
/// The userinfo is found through the parsed authority rather than by looking
/// for an `@`, so an `@` in a path or a query — which is allowed, and common
/// in an email address — is not mistaken for credentials.
pub fn loggable(uri: &Uri) -> String {
    let Some(authority) = uri.authority() else {
        return uri.to_string();
    };

    if !authority.as_str().contains('@') {
        return uri.to_string();
    }

    let mut safe = String::new();
    if let Some(scheme) = uri.scheme_str() {
        safe.push_str(scheme);
        safe.push_str("://");
    }
    safe.push_str(REDACTED_USERINFO);
    safe.push('@');
    // `host` keeps the brackets of an IPv6 literal, which belong to the URL.
    safe.push_str(authority.host());
    if let Some(port) = authority.port_u16() {
        safe.push(':');
        safe.push_str(&port.to_string());
    }
    if let Some(path) = uri.path_and_query() {
        safe.push_str(path.as_str());
    }

    safe
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(target: &str) -> Uri {
        target.parse().expect("valid uri")
    }

    fn redacted(target: &str) -> String {
        loggable(&uri(target))
    }

    #[test]
    fn credentials_in_a_request_target_do_not_reach_the_log() {
        let line = redacted("http://alice:hunter2@example.com/orders");

        assert!(!line.contains("hunter2"), "{line}");
        assert!(!line.contains("alice"), "{line}");
        assert_eq!(line, "http://[redacted]@example.com/orders");
    }

    #[test]
    fn a_username_with_no_password_is_redacted_too() {
        assert_eq!(
            redacted("http://alice@example.com/orders"),
            "http://[redacted]@example.com/orders"
        );
    }

    #[test]
    fn a_password_that_looks_like_the_rest_of_a_url_is_still_redacted() {
        // Everything awkward a password can contain: an `@`, a `/`, a `?`,
        // and a `:`. None of it may survive into the line.
        let line = redacted("http://alice:p%40ss%2Fword%3Fx@example.com/orders?q=1");

        assert_eq!(line, "http://[redacted]@example.com/orders?q=1");
        assert!(!line.contains("p%40ss"), "{line}");
    }

    #[test]
    fn the_rest_of_the_url_is_kept_exactly() {
        assert_eq!(
            redacted("http://alice:hunter2@example.com:8080/a/b?c=d&e=f"),
            "http://[redacted]@example.com:8080/a/b?c=d&e=f"
        );
    }

    #[test]
    fn a_redacted_target_says_that_there_were_credentials() {
        assert!(redacted("http://alice:hunter2@example.com/").contains("[redacted]"));
    }

    #[test]
    fn a_target_with_no_credentials_is_written_as_it_was_sent() {
        for target in [
            "http://example.com/orders",
            "http://example.com:8080/a?b=c",
            "https://example.com/",
            "/index.html",
            "*",
        ] {
            assert_eq!(redacted(target), target, "{target} was rewritten");
        }
    }

    #[test]
    fn an_at_sign_outside_the_authority_is_not_credentials() {
        for target in [
            "http://example.com/mail/alice@example.com",
            "http://example.com/search?q=alice@example.com",
        ] {
            assert_eq!(redacted(target), target, "{target} was rewritten");
        }
    }

    #[test]
    fn an_ipv6_host_keeps_its_brackets() {
        assert_eq!(
            redacted("http://alice:hunter2@[::1]:8080/x"),
            "http://[redacted]@[::1]:8080/x"
        );
    }

    #[test]
    fn an_authority_form_target_is_redacted_without_gaining_a_scheme() {
        // What a `CONNECT` carries. There is no scheme and no path, and
        // adding either would misreport what the client asked for.
        assert_eq!(
            redacted("alice:hunter2@example.com:443"),
            "[redacted]@example.com:443"
        );
        assert_eq!(redacted("example.com:443"), "example.com:443");
    }
}
