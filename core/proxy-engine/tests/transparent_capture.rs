//! End-to-end tests for the transparent capture bridge.
//!
//! What the Network Extension's provider does is stood in for by the test: it
//! connects to the bridge, sends the `CONNECT` the provider sends — naming
//! the *address* the application connected to, never a name — and then
//! speaks as the application would. Everything past that is real: the
//! listener, the interception, the rules, the controls, the capture, and TLS
//! and HTTP origins on OS-assigned loopback ports.
//!
//! ```text
//! test client ──CONNECT 127.0.0.1:port──▶ bridge ──▶ origin (as "localhost")
//!             ──TLS, SNI "localhost"───▶
//! ```
//!
//! Nothing here reaches the network, and nothing is a Network Extension: the
//! provider is exercised by hand on a Mac where one is installed, because
//! Apple's framework will not run one without a signed, approved extension.

mod support;

use std::time::Instant;

use proxy_ipc::ThrottleSettings;
use proxy_storage::TrafficErrorKind;
use rules::{NewRule, Rule, RuleAction, RuleMatcher, RuleSet};
use support::http2::{Http2Client, Http2Origin, Http2Reply};
use support::{
    connect_tunnel, next_exchange, no_further_exchange, open_tunnel, origin_form_request, send_raw,
    HttpsClient, Origin, Proxy, TlsOrigin,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};

/// Long enough to absorb a loaded machine, short enough that a bridge that
/// never answers fails the test instead of hanging it.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\nhello, https";

const PLAIN: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\nhello, plain";

fn one(name: &str, matcher: RuleMatcher, action: RuleAction) -> RuleSet {
    RuleSet::new([Rule::create(NewRule::new(name, matcher, action)).expect("a valid rule")])
}

/// A `CONNECT` as the provider writes it, naming the process the flow came
/// from.
fn connect_from(target: &str, pid: u32) -> Vec<u8> {
    format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxyLab-Source-Pid: {pid}\r\n\r\n")
        .into_bytes()
}

// MARK: - HTTPS

#[tokio::test]
async fn a_flow_to_an_address_is_intercepted_as_the_host_its_hello_names() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    let mut events = bridge.events();
    let client = HttpsClient::trusting(bridge.trust());

    // The address, as a flow names it; the name, as the application's TLS
    // hello names it.
    let response = client
        .exchange(
            bridge.addr(),
            &origin.addr().to_string(),
            "localhost",
            &origin_form_request("GET", "/hello", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello, https");
    assert_eq!(origin.only_request().target, "/hello");

    let exchange = next_exchange(&mut events).await;
    assert_eq!(
        exchange.request.url,
        format!("https://localhost:{}/hello", origin.addr().port())
    );
    assert_eq!(
        exchange.response.as_ref().map(|response| response.status),
        Some(200)
    );
    // One certificate, for the name the client checked — not the address.
    assert_eq!(bridge.certified_hosts(), 1);

    bridge.stop().await;
}

#[tokio::test]
async fn a_hello_that_names_no_host_is_answered_and_verified_as_the_address() {
    let origin = TlsOrigin::responding_with("127.0.0.1", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    let client = HttpsClient::trusting(bridge.trust());

    let response = client
        .exchange(
            bridge.addr(),
            &origin.authority(),
            "127.0.0.1",
            &origin_form_request("GET", "/by-address", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.only_request().target, "/by-address");

    bridge.stop().await;
}

#[tokio::test]
async fn an_origin_certificate_for_another_name_is_still_refused() {
    // The client names "localhost"; the origin at that address presents a
    // certificate for "127.0.0.1". Reading the name out of the hello is not
    // a reason to accept a certificate that does not match it.
    let origin = TlsOrigin::responding_with("127.0.0.1", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    let mut events = bridge.events();
    let client = HttpsClient::trusting(bridge.trust());

    let (connect, session) = client
        .connect(bridge.addr(), &origin.addr().to_string(), "localhost")
        .await;
    assert_eq!(connect.status, 200);

    // The client's handshake with the bridge completes; the bridge's with the
    // origin does not, so nothing is ever read back.
    if let Ok(mut session) = session {
        let _ = session
            .write_all(&origin_form_request("GET", "/", "localhost", &[]))
            .await;
        let mut rest = Vec::new();
        let read = timeout(TEST_TIMEOUT, session.read_to_end(&mut rest)).await;
        assert!(rest.is_empty() || read.is_err(), "the origin was reached");
    }

    let exchange = next_exchange(&mut events).await;
    assert_eq!(
        exchange.error.as_ref().map(|error| error.kind()),
        Some(TrafficErrorKind::UpstreamTls)
    );
    assert!(origin.requests().is_empty());

    bridge.stop().await;
}

#[tokio::test]
async fn a_flow_that_speaks_http_2_is_carried_as_http_2() {
    let origin = Http2Origin::responding_with("localhost", Http2Reply::ok("over h2")).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    let client = Http2Client::trusting(bridge.trust());

    let connection = client
        .connect(bridge.addr(), &origin.addr().to_string(), "localhost")
        .await;
    let response = connection.send("GET", "/stream", &[], Vec::new()).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"over h2");
    assert_eq!(origin.only_request().path, "/stream");

    bridge.stop().await;
}

// MARK: - Plain HTTP

#[tokio::test]
async fn plain_http_in_a_flow_is_forwarded_as_the_host_it_names() {
    let origin = Origin::responding_with(PLAIN).await;
    let bridge = Proxy::bridge(support_trust(), RuleSet::empty()).await;
    let mut events = bridge.events();

    let mut tunnel = connect_tunnel(bridge.addr(), &origin.addr().to_string()).await;
    assert_eq!(tunnel.response.status, 200);

    let answer = timeout(TEST_TIMEOUT, async {
        tunnel
            .stream
            .write_all(&origin_form_request(
                "GET",
                "/plain?x=1",
                &origin.addr().to_string(),
                &[],
            ))
            .await?;
        let mut answer = Vec::new();
        tunnel.stream.read_to_end(&mut answer).await?;

        std::io::Result::Ok(answer)
    })
    .await
    .expect("the bridge answers")
    .expect("the exchange");

    assert!(answer.starts_with(b"HTTP/1.1 200 "), "{answer:?}");
    assert!(answer.ends_with(b"hello, plain"));

    let forwarded = origin.only_request();
    assert_eq!(forwarded.method, "GET");
    assert_eq!(forwarded.target, "/plain?x=1");

    let exchange = next_exchange(&mut events).await;
    assert_eq!(
        exchange.request.url,
        format!("http://{}/plain?x=1", origin.addr())
    );

    bridge.stop().await;
}

// MARK: - What the proxy does to traffic

#[tokio::test]
async fn a_rule_applies_to_a_captured_flow_as_it_does_to_a_proxied_request() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let bridge = Proxy::bridge(
        origin.trust(),
        one(
            "block localhost",
            RuleMatcher::any().with_host("localhost"),
            RuleAction::block(),
        ),
    )
    .await;
    let client = HttpsClient::trusting(bridge.trust());

    let response = client
        .exchange(
            bridge.addr(),
            &origin.addr().to_string(),
            "localhost",
            &origin_form_request("GET", "/", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 403);
    assert!(
        origin.requests().is_empty(),
        "a blocked flow reached the origin"
    );

    bridge.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn throttling_applies_to_a_captured_flow() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    bridge.controls().throttle().set(ThrottleSettings {
        enabled: true,
        latency_ms: 500,
        download_bytes_per_second: 0,
        upload_bytes_per_second: 0,
    });
    let client = HttpsClient::trusting(bridge.trust());

    let started = Instant::now();
    let response = client
        .exchange(
            bridge.addr(),
            &origin.addr().to_string(),
            "localhost",
            &origin_form_request("GET", "/slow", &origin.authority(), &[]),
        )
        .await;
    let took = started.elapsed();

    assert_eq!(response.status, 200);
    assert!(took >= Duration::from_millis(450), "{took:?}");

    bridge.stop().await;
}

// MARK: - Loops

#[tokio::test]
async fn a_flow_from_the_proxys_own_process_is_handed_back_untouched() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;
    let mut events = bridge.events();

    // The bridge runs in this test process, so this is the proxy's own pid.
    let response = send_raw(
        bridge.addr(),
        &connect_from(&origin.addr().to_string(), std::process::id()),
    )
    .await;

    assert_eq!(response.status, 508);
    assert!(origin.requests().is_empty());
    // Not traffic: it is the proxy's own connection, and recording it would
    // record every exchange twice.
    no_further_exchange(&mut events).await;

    bridge.stop().await;
}

#[tokio::test]
async fn a_flow_from_any_other_process_is_carried() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let bridge = Proxy::bridge(origin.trust(), RuleSet::empty()).await;

    let tunnel = open_tunnel(
        bridge.addr(),
        &connect_from(&origin.addr().to_string(), std::process::id() + 1),
    )
    .await;

    assert_eq!(tunnel.response.status, 200);

    bridge.stop().await;
}

#[tokio::test]
async fn a_flow_to_the_bridge_itself_is_refused_and_recorded() {
    let bridge = Proxy::bridge(support_trust(), RuleSet::empty()).await;
    let mut events = bridge.events();

    let response = send_raw(
        bridge.addr(),
        &support::connect_request(&bridge.addr().to_string()),
    )
    .await;

    assert_eq!(response.status, 508);
    let exchange = next_exchange(&mut events).await;
    assert_eq!(
        exchange.error.as_ref().map(|error| error.kind()),
        Some(TrafficErrorKind::RequestRejected)
    );

    bridge.stop().await;
}

#[tokio::test]
async fn anything_but_a_connect_is_not_a_flow() {
    let origin = Origin::responding_with(PLAIN).await;
    let bridge = Proxy::bridge(support_trust(), RuleSet::empty()).await;

    let response = send_raw(
        bridge.addr(),
        format!(
            "GET http://{}/ HTTP/1.1\r\nHost: {}\r\n\r\n",
            origin.addr(),
            origin.addr()
        )
        .as_bytes(),
    )
    .await;

    assert_eq!(response.status, 405);
    assert!(origin.requests().is_empty());

    bridge.stop().await;
}

#[tokio::test]
async fn an_unreachable_destination_is_refused_before_the_flow_is_accepted() {
    let bridge = Proxy::bridge(support_trust(), RuleSet::empty()).await;

    // A port bound to learn it is free and then released.
    let unused = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        listener.local_addr().expect("address")
    };
    let response = send_raw(
        bridge.addr(),
        &support::connect_request(&unused.to_string()),
    )
    .await;

    assert_eq!(response.status, 502);

    bridge.stop().await;
}

/// Trust in nothing a test origin presents, for the tests that never reach a
/// TLS origin.
fn support_trust() -> proxy_tls::UpstreamTrust {
    proxy_tls::UpstreamTrust::PublicRoots
}
