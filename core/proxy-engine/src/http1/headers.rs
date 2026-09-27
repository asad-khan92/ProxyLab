//! Header hygiene at the proxy boundary.
//!
//! Some headers describe a single TCP hop rather than the message itself, and
//! a proxy must not pass them on: they belong to the connection the proxy is
//! terminating, not to the request or response travelling through it. The
//! framing headers are stripped for the same reason — each side of the proxy
//! frames its own bodies.
//!
//! The one header this module adds is the proxy's own `Connection: close`,
//! which describes the hop it really does own: the connection to the client.

use hyper::header::{HeaderMap, HeaderValue, CONNECTION, HOST, UPGRADE};

/// Headers that describe one hop and never cross the proxy.
///
/// `connection`, `keep-alive`, `te`, `trailer`, `transfer-encoding` and
/// `upgrade` are the hop-by-hop headers of RFC 9110; `proxy-connection` is the
/// widespread non-standard spelling of `connection`; `proxy-authenticate` and
/// `proxy-authorization` address the proxy itself.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Connection options that name a header state rather than a header.
const CONNECTION_OPTIONS: &[&str] = &["close", "keep-alive", "upgrade"];

/// Removes every header that must not cross the proxy.
///
/// A sender can nominate further hop-by-hop headers by listing them in
/// `Connection`, so those are collected before `Connection` itself is removed.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let nominated: Vec<String> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty() && !CONNECTION_OPTIONS.contains(&name.as_str()))
        .collect();

    for name in HOP_BY_HOP {
        headers.remove(*name);
    }

    for name in &nominated {
        headers.remove(name.as_str());
    }
}

/// Replaces `Host` with the host of the absolute-form request target.
///
/// RFC 9112 §3.2.2 makes the request target authoritative for a proxy request:
/// whatever `Host` the client sent is ignored in favour of the URL it asked
/// for.
pub fn set_host(headers: &mut HeaderMap, host: &HeaderValue) {
    headers.insert(HOST, host.clone());
}

/// Declares a response the last one on its connection.
///
/// The proxy answers one request per client connection and then closes it:
/// reusing a client connection is only worth anything with pooled upstream
/// connections behind it, and those are not built yet. Saying so in the
/// response, rather than leaving it to the server's keep-alive setting,
/// keeps the choice with the message — a `CONNECT` that opens a tunnel is
/// not finished with its connection and so does not carry this.
pub fn set_close(headers: &mut HeaderMap) {
    headers.insert(CONNECTION, HeaderValue::from_static("close"));
}

/// Asks the other end to leave HTTP for WebSocket.
///
/// The counterpart to [`strip_hop_by_hop`], and deliberately a separate step.
/// `Connection` and `Upgrade` are hop-by-hop headers, so they are stripped
/// from every message that crosses the proxy — including this one. Putting
/// them back in the proxy's own words, rather than passing the client's
/// through, is what keeps the two facts separate: the client asked to
/// upgrade, and the proxy is asking the origin to upgrade. A `Connection`
/// header listing half a dozen other tokens does not cross, and the request
/// the origin sees asks for exactly one thing.
///
/// The same two headers are what a `101` carries back, and this sets them
/// there too: a response that hands its socket over says so in both.
pub fn set_websocket_upgrade(headers: &mut HeaderMap) {
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        headers
    }

    fn names(headers: &HeaderMap) -> Vec<String> {
        let mut names: Vec<String> = headers
            .keys()
            .map(|name| name.as_str().to_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn end_to_end_headers_are_left_alone() {
        let mut map = headers(&[
            ("accept", "application/json"),
            ("authorization", "Bearer token"),
            ("user-agent", "curl/8.0.0"),
            ("content-length", "12"),
        ]);

        strip_hop_by_hop(&mut map);

        assert_eq!(
            names(&map),
            ["accept", "authorization", "content-length", "user-agent"]
        );
    }

    #[test]
    fn hop_by_hop_headers_are_removed() {
        let mut map = headers(&[
            ("connection", "close"),
            ("keep-alive", "timeout=5"),
            ("proxy-connection", "keep-alive"),
            ("proxy-authorization", "Basic abc"),
            ("te", "trailers"),
            ("trailer", "expires"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("accept", "*/*"),
        ]);

        strip_hop_by_hop(&mut map);

        assert_eq!(names(&map), ["accept"]);
    }

    #[test]
    fn headers_nominated_by_connection_are_removed_too() {
        let mut map = headers(&[
            ("connection", "x-hop-one, X-Hop-Two"),
            ("x-hop-one", "1"),
            ("x-hop-two", "2"),
            ("x-keep", "3"),
        ]);

        strip_hop_by_hop(&mut map);

        assert_eq!(names(&map), ["x-keep"]);
    }

    #[test]
    fn connection_options_do_not_remove_same_named_headers() {
        let mut map = headers(&[("connection", "close"), ("x-keep", "1")]);

        strip_hop_by_hop(&mut map);

        assert_eq!(names(&map), ["x-keep"]);
    }

    #[test]
    fn every_value_of_a_repeated_header_is_removed() {
        let mut map = headers(&[
            ("connection", "x-hop"),
            ("x-hop", "1"),
            ("x-hop", "2"),
            ("x-keep", "3"),
        ]);

        strip_hop_by_hop(&mut map);

        assert_eq!(names(&map), ["x-keep"]);
    }

    #[test]
    fn stripping_an_empty_header_map_is_harmless() {
        let mut map = HeaderMap::new();

        strip_hop_by_hop(&mut map);

        assert!(map.is_empty());
    }

    #[test]
    fn the_host_header_is_replaced_with_the_target_host() {
        let mut map = headers(&[("host", "proxy.invalid")]);

        set_host(&mut map, &HeaderValue::from_static("example.com:8080"));

        assert_eq!(map[HOST], "example.com:8080");
        assert_eq!(map.get_all(HOST).iter().count(), 1);
    }

    #[test]
    fn the_host_header_is_added_when_the_client_sent_none() {
        let mut map = HeaderMap::new();

        set_host(&mut map, &HeaderValue::from_static("example.com"));

        assert_eq!(map[HOST], "example.com");
    }

    #[test]
    fn closing_declares_the_response_the_last_on_the_connection() {
        let mut map = HeaderMap::new();

        set_close(&mut map);

        assert_eq!(map[CONNECTION], "close");
    }

    #[test]
    fn asking_for_an_upgrade_names_the_protocol_and_the_connection_option() {
        let mut map = HeaderMap::new();

        set_websocket_upgrade(&mut map);

        assert_eq!(map[CONNECTION], "Upgrade");
        assert_eq!(map[UPGRADE], "websocket");
    }

    #[test]
    fn asking_for_an_upgrade_replaces_whatever_the_client_listed() {
        // A client may list several connection options; the origin is asked
        // for one thing and told nothing else.
        let mut map = headers(&[
            ("connection", "keep-alive, Upgrade, x-hop"),
            ("upgrade", "websocket, h2c"),
        ]);

        strip_hop_by_hop(&mut map);
        set_websocket_upgrade(&mut map);

        assert_eq!(map[CONNECTION], "Upgrade");
        assert_eq!(map.get_all(CONNECTION).iter().count(), 1);
        assert_eq!(map[UPGRADE], "websocket");
        assert_eq!(map.get_all(UPGRADE).iter().count(), 1);
    }

    #[test]
    fn closing_replaces_a_connection_header_that_survived_stripping() {
        let mut map = headers(&[("connection", "keep-alive")]);

        strip_hop_by_hop(&mut map);
        set_close(&mut map);

        assert_eq!(map[CONNECTION], "close");
        assert_eq!(map.get_all(CONNECTION).iter().count(), 1);
    }
}
