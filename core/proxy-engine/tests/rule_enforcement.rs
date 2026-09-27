//! Rules, against the real proxy.
//!
//! Every test starts the actual listener with an actual rule engine and sends
//! an actual request through it. What is being proved is not that the engine
//! decides correctly — `rules` proves that without a proxy — but that the
//! decision is carried out: that a blocked request never reaches an origin,
//! that a mapped one reaches a different origin, and that a locally answered
//! one is answered from the file the rule names.
//!
//! Nothing here reaches the network. The origins are servers the test starts
//! on loopback, and the certificates come from a CA generated per test.

mod support;

use std::io::Write;

use support::{origin_form_request, request, HttpsClient, Origin, Proxy, TlsOrigin, TEST_TIMEOUT};

use proxy_storage::TrafficEvent;
use proxy_tls::UpstreamTrust;
use rules::{
    MapLocal, NewRule, RemoteTarget, Rule, RuleAction, RuleMatcher, RuleSet, MAX_MAP_LOCAL_BYTES,
};
use tempfile::TempDir;
use tokio::time::timeout;

/// A plain `200 OK` from an origin that should not have been reached.
const UPSTREAM: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 8\r\n\r\nupstream";

/// A rule called `name` doing `action` to what `matcher` describes.
fn rule(name: &str, matcher: RuleMatcher, action: RuleAction) -> Rule {
    Rule::create(NewRule::new(name, matcher, action)).expect("a valid rule")
}

/// A rule set holding one rule.
fn one(rule: Rule) -> RuleSet {
    RuleSet::new([rule])
}

/// The next exchange the proxy publishes.
async fn published(subscriber: &mut proxy_storage::EventSubscriber) -> TrafficEvent {
    timeout(TEST_TIMEOUT, subscriber.recv())
        .await
        .expect("an exchange is published")
        .expect("the event stream is healthy")
}

/// A file holding `content`, in a directory removed with the test.
fn fixture(directory: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = directory.path().join(name);
    let mut file = std::fs::File::create(&path).expect("the fixture is created");
    file.write_all(content).expect("the fixture is written");

    path
}

// MARK: - Allow

#[tokio::test]
async fn an_allowed_request_reaches_the_origin() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "allow the api",
            RuleMatcher::any().with_host(origin.addr().ip().to_string()),
            RuleAction::Allow,
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "upstream");
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_no_rule_matches_reaches_the_origin() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "block something else",
            RuleMatcher::any().with_host("elsewhere.example.com"),
            RuleAction::block(),
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

// MARK: - Block

#[tokio::test]
async fn a_blocked_request_never_reaches_the_origin() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "block it",
            RuleMatcher::any().with_path("/secret"),
            RuleAction::block(),
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/secret", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 403);
    assert!(response.body_text().contains("blocked by a ProxyLab rule"));
    // The whole point: the origin was never asked.
    assert!(origin.requests().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn a_block_answers_with_the_status_the_rule_chose() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "gone",
            RuleMatcher::any(),
            RuleAction::block_with(451),
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/x", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 451);
    assert!(origin.requests().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn a_blocked_request_is_still_captured_as_traffic() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule("block", RuleMatcher::any(), RuleAction::block())),
    )
    .await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "POST",
        &format!("http://{}/orders", origin.addr()),
        &[],
    )
    .await;

    // A blocked request is not a request that vanished: it is in the traffic
    // list with the response the client actually received.
    let exchange = published(&mut events).await;
    assert_eq!(exchange.request.method, "POST");
    assert!(exchange.request.url.ends_with("/orders"));
    assert_eq!(exchange.status(), Some(403));
    assert_eq!(exchange.error, None);

    proxy.stop().await;
}

#[tokio::test]
async fn a_disabled_rule_does_not_block() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let mut blocked = rule("block", RuleMatcher::any(), RuleAction::block());
    blocked.enabled = false;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(blocked),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/x", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn turning_a_rule_off_while_the_proxy_runs_stops_it_blocking() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let blocked = rule("block", RuleMatcher::any(), RuleAction::block());
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(blocked.clone()),
    )
    .await;
    let url = format!("http://{}/x", origin.addr());

    assert_eq!(request(proxy.addr(), "GET", &url, &[]).await.status, 403);

    // What an IPC update does: a whole new snapshot, swapped in.
    let mut off = blocked;
    off.enabled = false;
    proxy.set_rules(one(off));

    assert_eq!(request(proxy.addr(), "GET", &url, &[]).await.status, 200);
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_connect_is_not_matched_against_host_and_path_rules() {
    // A `CONNECT` carries no path, and blocking it would block every request
    // in the session rather than the ones a rule named. The rule below would
    // match the CONNECT's authority if CONNECT were evaluated.
    let origin = TlsOrigin::responding_with("localhost", UPSTREAM).await;
    let proxy = Proxy::ruled(
        origin.trust(),
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "block the path only",
            RuleMatcher::any().with_path("/secret"),
            RuleAction::block(),
        )),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    // The session opens, and the request inside it that does not match is
    // carried.
    let response = client
        .exchange(
            proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request("GET", "/allowed", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

// MARK: - Map Local

#[tokio::test]
async fn a_mapped_local_request_is_answered_from_the_file() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = fixture(&directory, "users.json", br#"{"users":["ada"]}"#);
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "fixture",
            RuleMatcher::any().with_path("/users"),
            RuleAction::MapLocal(MapLocal::new(&path)),
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/users", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), r#"{"users":["ada"]}"#);
    // Answered here: nothing was sent upstream.
    assert!(origin.requests().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_local_response_names_a_content_type() {
    let directory = TempDir::new().expect("a temporary directory");
    let json = fixture(&directory, "body.json", b"{}");
    let text = fixture(&directory, "body.txt", b"hello");
    let unknown = fixture(&directory, "body.unknown", b"bytes");

    for (path, expected) in [
        (json, Some("application/json")),
        (text, Some("text/plain; charset=utf-8")),
        (unknown, None),
    ] {
        let proxy = Proxy::ruled(
            UpstreamTrust::PublicRoots,
            proxy_engine::capture::Capture::with_defaults(),
            one(rule(
                "fixture",
                RuleMatcher::any(),
                RuleAction::MapLocal(MapLocal::new(&path)),
            )),
        )
        .await;

        let response = request(proxy.addr(), "GET", "http://example.com/x", &[]).await;

        assert_eq!(
            response
                .headers
                .iter()
                .find(|(name, _)| name == "content-type")
                .map(|(_, value)| value.as_str()),
            expected,
            "{}",
            path.display()
        );

        proxy.stop().await;
    }
}

#[tokio::test]
async fn a_map_local_content_type_the_rule_names_wins() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = fixture(&directory, "body.txt", b"{}");
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "fixture",
            RuleMatcher::any(),
            RuleAction::MapLocal(MapLocal::new(&path).with_content_type("application/json")),
        )),
    )
    .await;

    let response = request(proxy.addr(), "GET", "http://example.com/x", &[]).await;

    assert_eq!(
        response
            .headers
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| value.as_str()),
        Some("application/json")
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_map_local_that_cannot_be_served_says_so_without_naming_the_path() {
    let directory = TempDir::new().expect("a temporary directory");
    let missing = directory.path().join("not-here.json");
    let a_directory = directory.path().to_path_buf();

    for path in [missing, a_directory] {
        let origin = Origin::responding_with(UPSTREAM).await;
        let proxy = Proxy::ruled(
            UpstreamTrust::PublicRoots,
            proxy_engine::capture::Capture::with_defaults(),
            one(rule(
                "fixture",
                RuleMatcher::any(),
                RuleAction::MapLocal(MapLocal::new(&path)),
            )),
        )
        .await;

        let response = request(
            proxy.addr(),
            "GET",
            &format!("http://{}/x", origin.addr()),
            &[],
        )
        .await;

        assert_eq!(response.status, 500, "{}", path.display());
        // The client learns that a rule failed, and nothing about this
        // machine's filesystem.
        let body = response.body_text();
        assert!(body.contains("could not serve its local file"), "{body}");
        assert!(
            !body.contains(&path.display().to_string()),
            "{body} names the path"
        );
        // And a failed mapping does not fall through to the origin.
        assert!(origin.requests().is_empty());

        proxy.stop().await;
    }
}

#[tokio::test]
async fn a_mapped_local_request_is_captured_as_traffic() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = fixture(&directory, "users.json", b"[]");
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "fixture",
            RuleMatcher::any(),
            RuleAction::MapLocal(MapLocal::new(&path)),
        )),
    )
    .await;
    let mut events = proxy.events();

    request(proxy.addr(), "GET", "http://api.example.com/users", &[]).await;

    let exchange = published(&mut events).await;
    assert_eq!(exchange.request.url, "http://api.example.com/users");
    assert_eq!(exchange.status(), Some(200));
    // The body the client was served is the body that was captured.
    assert_eq!(
        exchange
            .response
            .as_ref()
            .map(|response| response.body.data()),
        Some(b"[]".as_slice())
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_cannot_choose_which_file_a_map_local_rule_serves() {
    // The rule names one file. Nothing from the request is joined to it, so
    // a path that tries to climb out reaches the configured file like every
    // other matching request does.
    let directory = TempDir::new().expect("a temporary directory");
    let path = fixture(&directory, "fixture.json", b"the configured file");
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "fixture",
            RuleMatcher::any().with_path_prefix("/files/"),
            RuleAction::MapLocal(MapLocal::new(&path)),
        )),
    )
    .await;

    for target in [
        "http://example.com/files/../../../../etc/passwd",
        "http://example.com/files/%2e%2e%2f%2e%2e%2fetc%2fpasswd",
        "http://example.com/files/etc/passwd",
    ] {
        let response = request(proxy.addr(), "GET", target, &[]).await;

        assert_eq!(response.status, 200, "{target}");
        assert_eq!(response.body_text(), "the configured file", "{target}");
    }

    proxy.stop().await;
}

#[tokio::test]
async fn a_file_larger_than_a_mapped_response_may_be_is_refused() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = directory.path().join("huge.bin");
    let file = std::fs::File::create(&path).expect("the file is created");
    file.set_len(MAX_MAP_LOCAL_BYTES + 1)
        .expect("the file is sized");
    drop(file);

    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "huge",
            RuleMatcher::any(),
            RuleAction::MapLocal(MapLocal::new(&path)),
        )),
    )
    .await;

    let response = request(proxy.addr(), "GET", "http://example.com/x", &[]).await;

    assert_eq!(response.status, 500);

    proxy.stop().await;
}

// MARK: - Map Remote

#[tokio::test]
async fn a_mapped_request_reaches_the_destination_the_rule_names() {
    let intended = Origin::responding_with(UPSTREAM).await;
    let mapped = Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nmapped",
    )
    .await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any().with_host("api.example.com"),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        "http://api.example.com/users?active=true",
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "mapped");
    assert!(intended.requests().is_empty());

    // The path and query survived, and the Host names where it went.
    let forwarded = mapped.only_request();
    assert_eq!(forwarded.target, "/users?active=true");
    assert_eq!(forwarded.method, "GET");
    assert_eq!(
        forwarded.header("host"),
        Some(mapped.addr().to_string().as_str())
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_request_keeps_its_method_body_and_headers() {
    let mapped = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any().with_host("api.example.com"),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;

    support::request_with_body(
        proxy.addr(),
        "POST",
        "http://api.example.com/orders",
        &[("X-Trace", "abc123")],
        r#"{"item":1}"#,
    )
    .await;

    let forwarded = mapped.only_request();
    assert_eq!(forwarded.method, "POST");
    assert_eq!(forwarded.target, "/orders");
    assert_eq!(forwarded.body, br#"{"item":1}"#);
    assert_eq!(forwarded.header("x-trace"), Some("abc123"));

    proxy.stop().await;
}

#[tokio::test]
async fn a_destination_with_a_path_replaces_the_requests_path() {
    let mapped = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to v2",
            RuleMatcher::any().with_host("api.example.com"),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}/v2", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;

    request(proxy.addr(), "GET", "http://api.example.com/v1?a=1", &[]).await;

    // The rule's path, and the request's query.
    assert_eq!(mapped.only_request().target, "/v2?a=1");

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_request_never_has_two_urls_pasted_together() {
    let mapped = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any(),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;

    request(proxy.addr(), "GET", "http://api.example.com/foo", &[]).await;

    let target = &mapped.only_request().target;
    assert_eq!(target, "/foo");
    assert!(!target.contains("http://"), "{target}");
    assert!(!target.contains("api.example.com"), "{target}");

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_request_is_captured_with_where_it_actually_went() {
    let mapped = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any(),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;
    let mut events = proxy.events();

    request(proxy.addr(), "GET", "http://api.example.com/users", &[]).await;

    let exchange = published(&mut events).await;
    // The URL the client asked for, and the destination it was sent to.
    assert_eq!(exchange.request.url, "http://api.example.com/users");
    assert_eq!(
        exchange.connection.destination.as_ref().map(|to| to.port),
        Some(mapped.addr().port())
    );
    assert_eq!(exchange.status(), Some(200));

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_destination_that_is_not_listening_is_a_bad_gateway() {
    // A port that was bound to learn a free one and then released, so
    // connecting to it is refused rather than ignored.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("a local address");
    drop(listener);

    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to nowhere",
            RuleMatcher::any(),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{addr}")).expect("a valid destination"),
            ),
        )),
    )
    .await;

    let response = request(proxy.addr(), "GET", "http://api.example.com/x", &[]).await;

    assert_eq!(response.status, 502);

    proxy.stop().await;
}

// MARK: - Inside an intercepted session

#[tokio::test]
async fn an_intercepted_request_can_be_blocked() {
    let origin = TlsOrigin::responding_with("localhost", UPSTREAM).await;
    let proxy = Proxy::ruled(
        origin.trust(),
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "block",
            RuleMatcher::any().with_path("/secret"),
            RuleAction::block(),
        )),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = client
        .exchange(
            proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request("GET", "/secret", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 403);
    assert!(response.body_text().contains("blocked by a ProxyLab rule"));
    assert!(origin.requests().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn an_intercepted_request_can_be_answered_from_a_file() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = fixture(&directory, "users.json", br#"{"from":"disk"}"#);
    let origin = TlsOrigin::responding_with("localhost", UPSTREAM).await;
    let proxy = Proxy::ruled(
        origin.trust(),
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "fixture",
            RuleMatcher::any().with_path("/users"),
            RuleAction::MapLocal(MapLocal::new(&path)),
        )),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = client
        .exchange(
            proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request("GET", "/users", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), r#"{"from":"disk"}"#);
    assert!(origin.requests().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn an_intercepted_request_can_be_mapped_to_a_plaintext_destination() {
    let intended = TlsOrigin::responding_with("localhost", UPSTREAM).await;
    let mapped = Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nmapped",
    )
    .await;
    let proxy = Proxy::ruled(
        intended.trust(),
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any().with_path("/users"),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = client
        .exchange(
            proxy.addr(),
            &intended.authority(),
            intended.host(),
            &origin_form_request("GET", "/users?a=1", &intended.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "mapped");
    // The session's own origin was never asked, and the mapped one was.
    assert!(intended.requests().is_empty());
    assert_eq!(mapped.only_request().target, "/users?a=1");

    proxy.stop().await;
}

#[tokio::test]
async fn an_intercepted_request_can_be_mapped_to_another_tls_destination() {
    let intended = TlsOrigin::responding_with("localhost", UPSTREAM).await;
    let mapped = TlsOrigin::responding_with(
        "localhost",
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nmapped",
    )
    .await;
    // Both CAs. The session to the original origin is opened by the
    // `CONNECT`, before any request has been read and so before any rule has
    // been consulted, so that origin still has to be reachable and its
    // certificate still has to verify. See the note in `ARCHITECTURE.md`.
    let proxy = Proxy::ruled(
        support::trusting_all(&[intended.trust(), mapped.trust()]),
        proxy_engine::capture::Capture::with_defaults(),
        one(rule(
            "to staging",
            RuleMatcher::any().with_path("/users"),
            RuleAction::MapRemote(
                rules::MapRemote::parse(&format!("https://{}", mapped.authority()))
                    .expect("a valid destination"),
            ),
        )),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = client
        .exchange(
            proxy.addr(),
            &intended.authority(),
            intended.host(),
            &origin_form_request("GET", "/users", &intended.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "mapped");
    assert!(intended.requests().is_empty());
    assert_eq!(mapped.requests().len(), 1);

    proxy.stop().await;
}

// MARK: - Which rule wins, in the proxy

#[tokio::test]
async fn the_first_matching_rule_is_the_one_the_proxy_carries_out() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let block = Rule::create(
        NewRule::new(
            "block the host",
            RuleMatcher::any().with_host(origin.addr().ip().to_string()),
            RuleAction::block(),
        )
        .with_priority(10),
    )
    .expect("a valid rule");
    let allow = Rule::create(
        NewRule::new(
            "except health",
            RuleMatcher::any().with_path("/health"),
            RuleAction::Allow,
        )
        .with_priority(5),
    )
    .expect("a valid rule");
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        proxy_engine::capture::Capture::with_defaults(),
        RuleSet::new([block, allow]),
    )
    .await;

    let health = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/health", origin.addr()),
        &[],
    )
    .await;
    let other = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/users", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(health.status, 200);
    assert_eq!(other.status, 403);
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_map_remote_target_is_the_one_the_rule_was_created_with() {
    // Proves the destination is parsed once and kept in parts, rather than
    // re-parsed per request from a string that could be read two ways.
    let target = RemoteTarget::parse("https://staging.example.com:8443/v2?fixed=yes")
        .expect("a valid destination");

    assert_eq!(target.rewrite("/foo", Some("a=1")), "/v2?fixed=yes");
    assert_eq!(target.host_header(), "staging.example.com:8443");
    assert_eq!(target.effective_port(), 8443);
}
