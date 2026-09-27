//! End-to-end tests for the upstream connection pool.
//!
//! Everything here goes through the real listener, over a real socket, to a
//! real in-process origin server. What is asserted is what the *origin* saw:
//! how many connections were opened to it, which requests came down which
//! one, and how many it was answering at once. That is the only honest way to
//! test a pool — a proxy that claims to reuse connections and opens a fresh
//! one per request looks identical from the client's side.
//!
//! ```text
//!   client ──▶ ProxyLab ──▶ KeepAliveOrigin
//!                              │
//!                              └── counts connections, requests, concurrency
//! ```
//!
//! Concurrency is proved with a barrier inside the origin rather than with a
//! stopwatch: see [`support::pooling`]. Nothing here sleeps to decide whether
//! a test passed.

mod support;

use support::http2::{Http2Client, Http2Origin, Http2Reply};
use support::pooling::{KeepAliveOrigin, OriginPlan};
use support::{
    next_exchange, origin_form_request, request, send_raw, HttpsClient, Origin, Proxy, RawResponse,
    TEST_TIMEOUT,
};

use proxy_engine::capture::Capture;
use proxy_engine::upstream::PoolLimits;
use proxy_tls::UpstreamTrust;
use rules::{
    BlockAction, MapLocal, MapRemote, NewRule, RemoteScheme, RemoteTarget, Rule, RuleAction,
    RuleMatcher, RuleSet,
};
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration};

/// A `GET` of `path` at `origin`, through `proxy`.
async fn get(proxy: &Proxy, origin: &KeepAliveOrigin, path: &str) -> RawResponse {
    timeout(
        TEST_TIMEOUT,
        request(
            proxy.addr(),
            "GET",
            &format!("{}{path}", origin.base_url()),
            &[],
        ),
    )
    .await
    .expect("the proxy answers within the timeout")
}

/// Which physical origin connection answered a response.
fn served_by(response: &RawResponse) -> &str {
    response
        .header("x-origin-connection")
        .expect("the origin names the connection it answered on")
}

/// A rule set holding exactly one rule.
fn only(name: &str, matcher: RuleMatcher, action: RuleAction) -> RuleSet {
    RuleSet::new([Rule::create(NewRule::new(name, matcher, action)).expect("a valid rule")])
}

// ---------------------------------------------------------------------------
// Reuse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sequential_requests_to_one_origin_reuse_one_connection() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    for path in ["/one", "/two", "/three"] {
        let response = get(&proxy, &origin, path).await;

        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), path);
        assert_eq!(served_by(&response), "0", "a second connection was opened");
    }

    assert_eq!(origin.connections(), 1, "the connection was not reused");
    assert_eq!(origin.requests_on(0), ["/one", "/two", "/three"]);

    proxy.stop().await;
}

#[tokio::test]
async fn a_reused_connection_is_put_back_after_every_request() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    get(&proxy, &origin, "/first").await;
    assert_eq!(proxy.idle_upstream_connections(), 1);

    get(&proxy, &origin, "/second").await;
    assert_eq!(proxy.idle_upstream_connections(), 1);
    assert_eq!(origin.connections(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn different_origins_never_share_a_connection() {
    let first = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let second = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    get(&proxy, &first, "/a").await;
    get(&proxy, &second, "/b").await;
    get(&proxy, &first, "/c").await;

    assert_eq!(first.requests(), ["/a", "/c"]);
    assert_eq!(second.requests(), ["/b"]);
    assert_eq!(first.connections(), 1);
    assert_eq!(second.connections(), 1);
    assert_eq!(
        proxy.idle_upstream_connections(),
        2,
        "the two origins should hold a connection each"
    );

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_requests_to_one_origin_each_get_a_connection() {
    const REQUESTS: usize = 4;

    // The origin answers nothing until four requests are waiting at once, so
    // this test cannot pass unless the proxy really did run them side by side.
    let origin = KeepAliveOrigin::plaintext(OriginPlan::gated(REQUESTS)).await;
    let proxy = Proxy::start().await;
    let addr = proxy.addr();
    let base = origin.base_url();

    let mut sending = Vec::new();
    for index in 0..REQUESTS {
        let url = format!("{base}/concurrent-{index}");
        sending.push(tokio::spawn(async move {
            timeout(TEST_TIMEOUT, request(addr, "GET", &url, &[]))
                .await
                .expect("the proxy answers within the timeout")
        }));
    }

    let mut connections = Vec::new();
    for sent in sending {
        let response = sent.await.expect("the request task does not panic");

        assert_eq!(response.status, 200);
        connections.push(served_by(&response).to_owned());
    }

    connections.sort();
    connections.dedup();
    assert_eq!(
        connections.len(),
        REQUESTS,
        "requests shared a connection: {connections:?}"
    );
    assert_eq!(origin.connections(), REQUESTS);
    assert_eq!(origin.peak_concurrent(), REQUESTS);

    proxy.stop().await;
}

#[tokio::test]
async fn one_connection_never_carries_two_exchanges_at_once() {
    const REQUESTS: usize = 6;

    let origin = KeepAliveOrigin::plaintext(OriginPlan::gated(REQUESTS)).await;
    let proxy = Proxy::start().await;
    let addr = proxy.addr();
    let base = origin.base_url();

    let sending: Vec<_> = (0..REQUESTS)
        .map(|index| {
            let url = format!("{base}/overlap-{index}");
            tokio::spawn(async move {
                timeout(TEST_TIMEOUT, request(addr, "GET", &url, &[]))
                    .await
                    .expect("the proxy answers within the timeout")
            })
        })
        .collect();

    for sent in sending {
        assert_eq!(
            sent.await.expect("the request task does not panic").status,
            200
        );
    }

    // Every request was in flight at the same time, so no connection can have
    // carried more than one of them.
    for id in 0..origin.connections() {
        assert_eq!(
            origin.requests_on(id).len(),
            1,
            "connection {id} carried more than one concurrent exchange"
        );
    }

    proxy.stop().await;
}

#[tokio::test]
async fn connections_opened_for_concurrent_requests_are_reused_afterwards() {
    const REQUESTS: usize = 3;

    let origin = KeepAliveOrigin::plaintext(OriginPlan::gated(REQUESTS)).await;
    let proxy = Proxy::start().await;
    let addr = proxy.addr();
    let base = origin.base_url();

    let sending: Vec<_> = (0..REQUESTS)
        .map(|index| {
            let url = format!("{base}/burst-{index}");
            tokio::spawn(async move {
                timeout(TEST_TIMEOUT, request(addr, "GET", &url, &[]))
                    .await
                    .expect("the proxy answers within the timeout")
            })
        })
        .collect();
    for sent in sending {
        sent.await.expect("the request task does not panic");
    }

    assert_eq!(origin.connections(), REQUESTS);
    assert_eq!(proxy.idle_upstream_connections(), REQUESTS);

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_pool_keeps_no_more_than_its_limit() {
    const REQUESTS: usize = 5;
    const KEPT: usize = 2;

    let origin = KeepAliveOrigin::plaintext(OriginPlan::gated(REQUESTS)).await;
    let proxy = Proxy::pooling(
        UpstreamTrust::PublicRoots,
        Capture::with_defaults(),
        RuleSet::empty(),
        PoolLimits {
            max_idle_per_key: KEPT,
            ..PoolLimits::default()
        },
    )
    .await;
    let addr = proxy.addr();
    let base = origin.base_url();

    let sending: Vec<_> = (0..REQUESTS)
        .map(|index| {
            let url = format!("{base}/bounded-{index}");
            tokio::spawn(async move {
                timeout(TEST_TIMEOUT, request(addr, "GET", &url, &[]))
                    .await
                    .expect("the proxy answers within the timeout")
            })
        })
        .collect();
    for sent in sending {
        assert_eq!(
            sent.await.expect("the request task does not panic").status,
            200
        );
    }

    assert_eq!(origin.connections(), REQUESTS, "every request was carried");
    assert_eq!(
        proxy.idle_upstream_connections(),
        KEPT,
        "the pool kept more than it was allowed"
    );

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Connections that must not be kept
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_connection_the_origin_closed_is_not_pooled() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::closing()).await;
    let proxy = Proxy::start().await;

    let response = get(&proxy, &origin, "/closes").await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "/closes");
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "a connection that said close was kept"
    );

    // And the next request opens a connection of its own rather than finding
    // the closed one.
    get(&proxy, &origin, "/again").await;
    assert_eq!(origin.connections(), 2);

    proxy.stop().await;
}

#[tokio::test]
async fn a_stale_pooled_connection_is_replaced_and_the_request_still_arrives() {
    // The origin answers and then closes without saying so, which is what a
    // keep-alive connection reaped at the far end looks like: the proxy pools
    // it in good faith and finds out when it writes.
    let origin = KeepAliveOrigin::plaintext(OriginPlan::vanishing()).await;
    let proxy = Proxy::start().await;

    assert_eq!(get(&proxy, &origin, "/first").await.status, 200);

    // Long enough for the close to reach the proxy in the usual case, and
    // harmless when it has not: either way the second request must succeed.
    sleep(Duration::from_millis(50)).await;

    let second = get(&proxy, &origin, "/second").await;
    assert_eq!(second.status, 200);
    assert_eq!(second.body_text(), "/second");
    assert_eq!(origin.connections(), 2);
    assert_eq!(origin.requests(), ["/first", "/second"]);

    proxy.stop().await;
}

#[tokio::test]
async fn a_malformed_upstream_response_is_a_bad_gateway_and_pools_nothing() {
    let origin = Origin::responding_with(b"this is not an HTTP response at all\r\n\r\n").await;
    let proxy = Proxy::start().await;

    let response = timeout(
        TEST_TIMEOUT,
        request(
            proxy.addr(),
            "GET",
            &format!("{}/x", origin.base_url()),
            &[],
        ),
    )
    .await
    .expect("the proxy answers within the timeout");

    assert_eq!(response.status, 502);
    assert_eq!(proxy.idle_upstream_connections(), 0);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_the_origin_may_already_have_acted_on_is_not_sent_again() {
    // The origin reads the whole request and then closes without answering.
    // Whether it acted on the request cannot be known from here, so the
    // exchange fails rather than being retried — a second `GET` would be a
    // second side effect for a request the developer made once.
    let origin = Origin::responding_with(b"").await;
    let proxy = Proxy::start().await;

    let response = timeout(
        TEST_TIMEOUT,
        request(
            proxy.addr(),
            "GET",
            &format!("{}/x", origin.base_url()),
            &[],
        ),
    )
    .await
    .expect("the proxy answers within the timeout");

    assert_eq!(response.status, 502);
    assert_eq!(
        origin.requests().len(),
        1,
        "a request the origin may have seen was sent a second time"
    );
    assert_eq!(proxy.idle_upstream_connections(), 0);

    proxy.stop().await;
}

#[tokio::test]
async fn a_response_body_that_breaks_off_pools_nothing() {
    // A hundred bytes promised and five delivered, then the socket closes.
    let origin =
        Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort").await;
    let proxy = Proxy::start().await;

    let response = timeout(
        TEST_TIMEOUT,
        request(
            proxy.addr(),
            "GET",
            &format!("{}/x", origin.base_url()),
            &[],
        ),
    )
    .await
    .expect("the proxy answers within the timeout");

    assert_eq!(response.status, 200, "the head was fine; the body was not");
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "a connection whose body broke off was kept"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_response_that_is_never_read_does_not_leave_a_connection_in_the_pool() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    // The head is written and the client goes away before the body is read.
    let response = send_raw(
        proxy.addr(),
        format!(
            "GET {}/abandoned HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            origin.base_url(),
            origin.addr()
        )
        .as_bytes(),
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.connections(), 1);

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_blocked_request_acquires_no_upstream_connection() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        Capture::with_defaults(),
        only(
            "block",
            RuleMatcher::any().with_host(origin.addr().ip().to_string()),
            RuleAction::Block(BlockAction { status: 403 }),
        ),
    )
    .await;

    let response = get(&proxy, &origin, "/blocked").await;

    assert_eq!(response.status, 403);
    assert_eq!(
        origin.connections(),
        0,
        "a blocked request reached the origin"
    );
    assert_eq!(proxy.idle_upstream_connections(), 0);

    proxy.stop().await;
}

#[tokio::test]
async fn a_map_local_request_acquires_no_upstream_connection() {
    let directory = TempDir::new().expect("a temporary directory");
    let fixture = directory.path().join("body.json");
    std::fs::write(&fixture, br#"{"from":"disk"}"#).expect("write the fixture");

    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        Capture::with_defaults(),
        only(
            "local",
            RuleMatcher::any().with_host(origin.addr().ip().to_string()),
            RuleAction::MapLocal(MapLocal::new(&fixture).with_content_type("application/json")),
        ),
    )
    .await;

    let response = get(&proxy, &origin, "/local").await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), r#"{"from":"disk"}"#);
    assert_eq!(origin.connections(), 0);
    assert_eq!(proxy.idle_upstream_connections(), 0);

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_request_pools_under_the_destination_it_actually_went_to() {
    let named = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let actual = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::ruled(
        UpstreamTrust::PublicRoots,
        Capture::with_defaults(),
        only(
            "remote",
            RuleMatcher::any()
                .with_host(named.addr().ip().to_string())
                .with_path("/mapped"),
            RuleAction::MapRemote(MapRemote {
                target: remote_target(&actual),
            }),
        ),
    )
    .await;

    // Mapped: it must reach the other origin and no connection to the named
    // one may be opened for it.
    let mapped = get(&proxy, &named, "/mapped").await;
    assert_eq!(mapped.status, 200);
    assert_eq!(named.connections(), 0);
    assert_eq!(actual.connections(), 1);

    // Unmapped, to the host the first request named: it must not be given the
    // connection that was opened for the mapped destination.
    let direct = get(&proxy, &named, "/direct").await;
    assert_eq!(direct.status, 200);
    assert_eq!(direct.body_text(), "/direct");
    assert_eq!(named.connections(), 1);
    assert_eq!(actual.requests().len(), 1);
    assert_eq!(
        proxy.idle_upstream_connections(),
        2,
        "the two destinations should hold a connection each"
    );

    proxy.stop().await;
}

/// A Map Remote target naming `origin`.
fn remote_target(origin: &KeepAliveOrigin) -> RemoteTarget {
    RemoteTarget {
        scheme: RemoteScheme::Http,
        host: origin.addr().ip().to_string(),
        port: Some(origin.addr().port()),
        path: None,
        query: None,
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_request_on_a_reused_connection_is_its_own_exchange() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    for path in ["/first", "/second", "/third"] {
        get(&proxy, &origin, path).await;
    }

    let mut ids = Vec::new();
    let mut urls = Vec::new();
    for _ in 0..3 {
        let exchange = next_exchange(&mut events).await;

        ids.push(exchange.id);
        urls.push(exchange.request.url.clone());
        assert_eq!(
            exchange.response.as_ref().map(|response| response.status),
            Some(200)
        );
        assert!(exchange.error.is_none());
        assert!(exchange.timing.completed_micros.is_some());
    }

    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "reuse merged two exchanges into one");
    assert_eq!(origin.connections(), 1, "the connection was not reused");

    for (url, path) in urls.iter().zip(["/first", "/second", "/third"]) {
        assert!(
            url.ends_with(path),
            "{url} is not the request that was made"
        );
    }

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// HTTPS, and the case Step 16 exists for
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_intercepted_https_request_still_works_and_pools_its_connection() {
    let origin = KeepAliveOrigin::tls("localhost", OriginPlan::default()).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = timeout(
        TEST_TIMEOUT,
        client.exchange(
            proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request("GET", "/secure", origin.host(), &[]),
        ),
    )
    .await
    .expect("the proxy answers within the timeout");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "/secure");
    assert_eq!(origin.connections(), 1);
    assert_eq!(
        proxy.idle_upstream_connections(),
        1,
        "the intercepted session's upstream connection was not kept"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn concurrent_http_2_streams_to_an_http_1_1_origin_do_not_serialise() {
    const STREAMS: usize = 4;

    // The origin speaks HTTP/1.1 only, and answers nothing until four
    // requests are waiting at once. Before Step 16 the four streams took
    // turns on one upstream connection and this could not have passed.
    let origin = KeepAliveOrigin::tls("localhost", OriginPlan::gated(STREAMS)).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let client = Http2Client::trusting(proxy.trust());

    let connection = client
        .connect(proxy.addr(), &origin.authority(), origin.host())
        .await;
    let paths: Vec<String> = (0..STREAMS)
        .map(|index| format!("/stream-{index}"))
        .collect();
    let borrowed: Vec<&str> = paths.iter().map(String::as_str).collect();

    let answers = timeout(TEST_TIMEOUT, connection.send_concurrently(&borrowed))
        .await
        .expect("every stream is answered within the timeout");

    assert_eq!(answers.len(), STREAMS);
    for path in &paths {
        let answer = answers.get(path).expect("the stream was answered");

        assert_eq!(answer.status, 200);
        assert_eq!(answer.body_text(), *path);
    }

    assert_eq!(
        origin.connections(),
        STREAMS,
        "the streams did not get connections of their own"
    );
    assert_eq!(origin.peak_concurrent(), STREAMS);

    proxy.stop().await;
}

#[tokio::test]
async fn concurrent_http_2_streams_to_an_http_2_origin_still_multiplex() {
    const STREAMS: usize = 4;

    let origin = Http2Origin::responding_with("localhost", Http2Reply::ok("multiplexed")).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let client = Http2Client::trusting(proxy.trust());

    let connection = client
        .connect(proxy.addr(), &origin.authority(), origin.host())
        .await;
    let paths: Vec<String> = (0..STREAMS).map(|index| format!("/mux-{index}")).collect();
    let borrowed: Vec<&str> = paths.iter().map(String::as_str).collect();

    let answers = timeout(TEST_TIMEOUT, connection.send_concurrently(&borrowed))
        .await
        .expect("every stream is answered within the timeout");

    assert_eq!(answers.len(), STREAMS);
    for answer in answers.values() {
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body_text(), "multiplexed");
    }

    assert_eq!(
        origin.connections(),
        1,
        "HTTP/2 multiplexing was given up for one connection per stream"
    );
    assert_eq!(
        origin.requests_on(0),
        STREAMS,
        "the streams did not share it"
    );
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "an HTTP/2 connection has no business in the HTTP/1.1 pool"
    );
    assert_eq!(proxy.upstream().http2_connections(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn two_client_connections_share_one_http_2_connection_to_an_origin() {
    let origin = Http2Origin::responding_with("localhost", Http2Reply::ok("shared")).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let client = Http2Client::trusting(proxy.trust());

    for _ in 0..2 {
        let connection = client
            .connect(proxy.addr(), &origin.authority(), origin.host())
            .await;
        let answer = connection.send("GET", "/shared", &[], Vec::new()).await;

        assert_eq!(answer.status, 200);
    }

    // The proxy holds exactly one HTTP/2 connection to this origin, and both
    // requests went down it. The second `CONNECT` did open a socket and
    // verify the origin's certificate — that is what it promises the client
    // before answering — but no second HTTP/2 conversation was started on it.
    assert_eq!(proxy.upstream().http2_connections(), 1);
    assert_eq!(origin.requests().len(), 2);
    assert_eq!(
        origin.requests_on(0),
        2,
        "the second session did not reuse the first connection"
    );

    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_the_proxy_closes_its_idle_connections() {
    let origin = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    get(&proxy, &origin, "/kept").await;
    assert_eq!(proxy.idle_upstream_connections(), 1);
    assert_eq!(origin.closed_connections(), 0);

    proxy.stop().await;

    timeout(TEST_TIMEOUT, origin.wait_for_connections_to_close())
        .await
        .expect("the idle connection is let go of");
    assert_eq!(origin.closed_connections(), 1);
}
