//! Telling a WebSocket upgrade apart from an ordinary request.
//!
//! ```text
//! GET /socket HTTP/1.1              ──▶ an upgrade
//! Connection: Upgrade
//! Upgrade: websocket
//! Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==
//! Sec-WebSocket-Version: 13
//!
//! GET /api HTTP/1.1                 ──▶ not one
//! Connection: keep-alive
//! ```
//!
//! `Connection: keep-alive` is not `Connection: Upgrade`, and an `Upgrade`
//! header naming something else — `h2c`, or a protocol this proxy has never
//! heard of — is not a WebSocket either. Both are ordinary requests and are
//! carried as such, which is the point of checking rather than guessing: a
//! request misread as an upgrade would be a request whose connection is
//! handed to a tunnel and never answered.
//!
//! What is checked is RFC 6455 §4.1's own list, and nothing beyond it. The
//! `Sec-WebSocket-Key` is not validated or recomputed here: the proxy does not
//! answer the handshake, the origin does, and the client checks the origin's
//! answer. Forwarding the key unchanged is what makes that work — see
//! [`super::forward`].

use hyper::header::{HeaderMap, CONNECTION, UPGRADE};
use hyper::{Method, Request, Version};

/// The protocol name a WebSocket upgrade asks for.
pub const WEBSOCKET: &str = "websocket";

/// The only version of the protocol RFC 6455 defines.
const SUPPORTED_VERSION: &str = "13";

/// The nonce the client sends and the origin answers.
const KEY: &str = "sec-websocket-key";

/// The version of the WebSocket protocol the client is asking for.
const VERSION: &str = "sec-websocket-version";

/// Whether `request` is a WebSocket upgrade this proxy should carry.
///
/// Everything RFC 6455 requires of the request, and all of it, because each
/// part rules out a different mistake:
///
/// ```text
/// GET                     a method that cannot carry an upgrade
/// HTTP/1.1                HTTP/1.0 has no upgrade, HTTP/2 has no handshake
/// Connection: Upgrade     the request means to leave HTTP
/// Upgrade: websocket      and to become this protocol rather than another
/// Sec-WebSocket-Key       the nonce the origin's answer is checked against
/// Sec-WebSocket-Version   the version of the protocol, which must be 13
/// ```
pub fn is_upgrade<B>(request: &Request<B>) -> bool {
    request.method() == Method::GET
        && request.version() == Version::HTTP_11
        && asks_to_upgrade(request.headers())
}

/// Whether these headers ask to leave HTTP for WebSocket.
fn asks_to_upgrade(headers: &HeaderMap) -> bool {
    lists(headers, CONNECTION.as_str(), "upgrade")
        && lists(headers, UPGRADE.as_str(), WEBSOCKET)
        && headers
            .get(KEY)
            .and_then(|key| key.to_str().ok())
            .is_some_and(|key| !key.trim().is_empty())
        && lists(headers, VERSION, SUPPORTED_VERSION)
}

/// Whether `name`'s value is a comma-separated list containing `token`.
///
/// Both the name and the token are matched without regard to case, because
/// HTTP says these are case-insensitive and clients spell them every way
/// there is: `Upgrade`, `upgrade`, and `keep-alive, Upgrade` are all the same
/// request.
fn lists(headers: &HeaderMap, name: &str, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|listed| listed.trim().eq_ignore_ascii_case(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;

    /// A well-formed upgrade request, which each test then spoils in one way.
    fn upgrade() -> hyper::http::request::Builder {
        Request::builder()
            .method(Method::GET)
            .uri("http://example.com/socket")
            .version(Version::HTTP_11)
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-version", "13")
    }

    fn built(builder: hyper::http::request::Builder) -> Request<Empty<Bytes>> {
        builder.body(Empty::new()).expect("valid request")
    }

    #[test]
    fn a_well_formed_upgrade_is_recognised() {
        assert!(is_upgrade(&built(upgrade())));
    }

    #[test]
    fn the_tokens_are_matched_however_they_are_spelled() {
        let request = built(
            Request::builder()
                .method(Method::GET)
                .uri("/socket")
                .header("connection", "keep-alive, UPGRADE")
                .header("upgrade", "WebSocket")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .header("sec-websocket-version", "13"),
        );

        assert!(is_upgrade(&request));
    }

    #[test]
    fn keep_alive_alone_is_not_an_upgrade() {
        let request = built(
            Request::builder()
                .method(Method::GET)
                .uri("/api")
                .header("connection", "keep-alive"),
        );

        assert!(!is_upgrade(&request));
    }

    #[test]
    fn an_upgrade_to_something_else_is_not_a_websocket() {
        let request = built(
            Request::builder()
                .method(Method::GET)
                .uri("/socket")
                .header("connection", "Upgrade")
                .header("upgrade", "h2c")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .header("sec-websocket-version", "13"),
        );

        assert!(!is_upgrade(&request));
    }

    #[test]
    fn every_required_header_is_required() {
        for missing in [
            "connection",
            "upgrade",
            "sec-websocket-key",
            "sec-websocket-version",
        ] {
            let mut request = built(upgrade());
            request.headers_mut().remove(missing);

            assert!(!is_upgrade(&request), "{missing} was not required");
        }
    }

    #[test]
    fn an_empty_key_is_not_a_key() {
        let mut request = built(upgrade());
        request
            .headers_mut()
            .insert("sec-websocket-key", "   ".parse().expect("valid value"));

        assert!(!is_upgrade(&request));
    }

    #[test]
    fn a_version_this_proxy_does_not_carry_is_not_carried() {
        let mut request = built(upgrade());
        request
            .headers_mut()
            .insert("sec-websocket-version", "8".parse().expect("valid value"));

        assert!(!is_upgrade(&request));
    }

    #[test]
    fn only_get_can_ask_for_an_upgrade() {
        assert!(!is_upgrade(&built(upgrade().method(Method::POST))));
        assert!(!is_upgrade(&built(upgrade().method(Method::CONNECT))));
    }

    #[test]
    fn an_upgrade_is_an_http_1_1_mechanism() {
        // HTTP/1.0 has no upgrade, and HTTP/2 has no handshake of this shape:
        // WebSocket over HTTP/2 is a different mechanism, and not one this
        // proxy carries.
        assert!(!is_upgrade(&built(upgrade().version(Version::HTTP_10))));
        assert!(!is_upgrade(&built(upgrade().version(Version::HTTP_2))));
    }
}
