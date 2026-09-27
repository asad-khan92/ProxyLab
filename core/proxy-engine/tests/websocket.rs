//! WebSocket through the proxy: the handshake, and the bytes after it.
//!
//! What is asserted here is that ProxyLab carries a WebSocket without being
//! in the way of it. The handshake reaches the origin with the headers that
//! make it a handshake, the origin's own `101` reaches the client with the
//! accept value it computed, and what the two ends then send each other
//! arrives unchanged and in both directions at once.
//!
//! The proxy is deliberately not asked to understand WebSocket framing, so
//! neither are these tests: after the `101` the connection carries byte
//! strings, because that is exactly what ProxyLab sees.

mod support;

use std::sync::Arc;

use rules::{NewRule, Rule, RuleAction, RuleMatcher, RuleSet};
use support::pooling::{KeepAliveOrigin, OriginPlan};
use support::websocket::{
    self, read_exactly, read_handshake, round_trip, upgrade_request, TlsWsOrigin, WsOrigin, ACCEPT,
    AGREED, KEY, REFUSED,
};
use support::{next_exchange, HttpsClient, Proxy, TEST_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;

/// A rule called `name` doing `action` to what `matcher` describes.
fn rule(name: &str, matcher: RuleMatcher, action: RuleAction) -> Rule {
    Rule::create(NewRule::new(name, matcher, action)).expect("a valid rule")
}

/// A rule set holding one rule.
fn one(rule: Rule) -> RuleSet {
    RuleSet::new([rule])
}

/// Opens a WebSocket through `proxy` to `url`, and hands back the socket it
/// is now carried on.
async fn open(
    proxy: std::net::SocketAddr,
    url: &str,
    host: &str,
) -> (websocket::Handshake, TcpStream) {
    let mut stream = TcpStream::connect(proxy)
        .await
        .expect("connect to the proxy");
    stream
        .write_all(&upgrade_request(url, host))
        .await
        .expect("send the handshake");
    stream.flush().await.expect("flush");

    let handshake = read_handshake(&mut stream).await;

    (handshake, stream)
}

// MARK: - The handshake

#[tokio::test]
async fn a_valid_upgrade_reaches_the_origin_as_an_upgrade() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, _stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    assert_eq!(handshake.status, 101);

    // The headers that make it a handshake survived a proxy that strips
    // `Connection` and `Upgrade` from everything else.
    let request = origin.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/socket");
    assert_eq!(request.header("upgrade"), Some("websocket"));
    assert_eq!(request.header("connection"), Some("Upgrade"));
    // The nonce is the client's own and is never regenerated: the client
    // checks the origin's answer against it.
    assert_eq!(request.header("sec-websocket-key"), Some(KEY));
    assert_eq!(request.header("sec-websocket-version"), Some("13"));
    // An end-to-end header is carried like any other.
    assert_eq!(request.header("origin"), Some("http://test.invalid"));

    proxy.stop().await;
}

#[tokio::test]
async fn the_origins_own_answer_reaches_the_client() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, _stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    assert_eq!(handshake.status, 101);
    // Computed by the origin from the client's key. A proxy that rewrote this
    // would be a proxy that broke the handshake it was carrying.
    assert_eq!(handshake.header("sec-websocket-accept"), Some(ACCEPT));
    assert_eq!(handshake.header("upgrade"), Some("websocket"));
    assert_eq!(
        handshake
            .header("connection")
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("upgrade")
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_that_is_not_an_upgrade_is_carried_as_an_ordinary_request() {
    // `Connection: keep-alive` is not `Connection: Upgrade`. A proxy that
    // confused the two would hand an ordinary request's connection to a
    // tunnel and never answer it.
    let origin = support::Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    )
    .await;
    let proxy = Proxy::start().await;

    let response = support::request(
        proxy.addr(),
        "GET",
        &format!("{}/api", origin.base_url()),
        &[("Connection", "keep-alive")],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "ok");

    proxy.stop().await;
}

#[tokio::test]
async fn an_upgrade_the_origin_refuses_is_an_ordinary_response() {
    let origin = WsOrigin::start(REFUSED, |_| async {}).await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    assert_eq!(handshake.status, 404);

    // The body of the refusal is forwarded like any other response body, and
    // nothing was upgraded.
    let mut body = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut body))
        .await
        .expect("the proxy closes the connection")
        .expect("read the body");
    assert!(
        String::from_utf8_lossy(&body).contains("no socket"),
        "got: {}",
        String::from_utf8_lossy(&body)
    );

    proxy.stop().await;
}

// MARK: - The bytes after it

#[tokio::test]
async fn bytes_travel_from_the_client_to_the_origin_and_back() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    // Bytes chosen to be nothing like HTTP, and to include a zero byte and a
    // high byte: a proxy that treated the tunnel as text would corrupt them.
    let message = [0x81, 0x05, b'h', b'e', 0x00, 0xff, b'o'];
    assert_eq!(round_trip(&mut stream, &message).await, message);

    // And again, so it is a connection rather than one exchange.
    assert_eq!(round_trip(&mut stream, b"second").await, b"second");

    proxy.stop().await;
}

#[tokio::test]
async fn both_directions_are_carried_at_once() {
    // The origin greets first and then echoes, so the two directions overlap:
    // the client reads a message it did not ask for while its own message is
    // still on the way up.
    let origin = WsOrigin::start(AGREED, |stream| async move {
        let (mut from_client, mut to_client) = tokio::io::split(stream);
        let _ = to_client.write_all(b"hello from the origin").await;
        let _ = to_client.flush().await;
        let _ = tokio::io::copy(&mut from_client, &mut to_client).await;
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    // Server to client, unprompted.
    assert_eq!(
        read_exactly(&mut stream, 21).await,
        b"hello from the origin"
    );

    // Client to server, on the same connection, afterwards.
    assert_eq!(round_trip(&mut stream, b"and back").await, b"and back");

    proxy.stop().await;
}

#[tokio::test]
async fn a_message_each_way_crosses_without_waiting_for_the_other() {
    // Neither side answers until both have spoken, so a proxy that carried
    // one direction at a time would deadlock here rather than fail an
    // assertion.
    let ready = Arc::new(Notify::new());
    let origin_ready = Arc::clone(&ready);

    let origin = WsOrigin::start(AGREED, move |stream| {
        let ready = Arc::clone(&origin_ready);
        async move {
            let (mut from_client, mut to_client) = tokio::io::split(stream);

            // Wait for the client's message before sending anything.
            let mut up = vec![0_u8; 9];
            let _ = tokio::io::AsyncReadExt::read_exact(&mut from_client, &mut up).await;
            assert_eq!(&up, b"client up");

            ready.notify_one();
            let _ = to_client.write_all(b"origin down").await;
            let _ = to_client.flush().await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    stream.write_all(b"client up").await.expect("write");
    stream.flush().await.expect("flush");

    timeout(TEST_TIMEOUT, ready.notified())
        .await
        .expect("the origin received the client's message");
    assert_eq!(read_exactly(&mut stream, 11).await, b"origin down");

    proxy.stop().await;
}

// MARK: - How it ends

#[tokio::test]
async fn an_origin_that_closes_ends_the_clients_side_too() {
    let origin = WsOrigin::start(AGREED, |mut stream| async move {
        let _ = stream.write_all(b"bye").await;
        let _ = stream.flush().await;
        let _ = stream.shutdown().await;
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("the client's side ends when the origin's does")
        .expect("read to the end");

    assert_eq!(rest, b"bye");

    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_disappears_does_not_take_the_proxy_with_it() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    // Dropped without a close frame and without a shutdown: a process that
    // went away.
    drop(stream);

    // The proxy is still answering, on a connection of its own.
    let ordinary = support::Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    )
    .await;
    let response = support::request(
        proxy.addr(),
        "GET",
        &format!("{}/api", ordinary.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);

    proxy.stop().await;
}

#[tokio::test]
async fn an_origin_that_cannot_be_reached_is_a_bad_gateway() {
    let unreachable = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        addr
    };
    let proxy = Proxy::start().await;

    let response = support::request(
        proxy.addr(),
        "GET",
        &format!("http://{unreachable}/socket"),
        &[
            ("Connection", "Upgrade"),
            ("Upgrade", "websocket"),
            ("Sec-WebSocket-Key", KEY),
            ("Sec-WebSocket-Version", "13"),
        ],
    )
    .await;

    assert_eq!(response.status, 502);

    proxy.stop().await;
}

// MARK: - Capture

#[tokio::test]
async fn the_handshake_is_captured_as_one_exchange() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    let exchange = next_exchange(&mut events).await;

    assert_eq!(exchange.request.method, "GET");
    assert_eq!(exchange.request.url, url);
    let response = exchange.response.as_ref().expect("the handshake answered");
    assert_eq!(response.status, 101);
    // What makes the stored exchange a WebSocket handshake, with nothing
    // added to the model to say so.
    assert_eq!(response.headers.text("upgrade"), Some("websocket"));
    assert_eq!(response.headers.text("sec-websocket-accept"), Some(ACCEPT));

    // Frames that cross afterwards are not exchanges: one WebSocket is one
    // row, however long it stays open.
    assert_eq!(round_trip(&mut stream, b"one").await, b"one");
    assert_eq!(round_trip(&mut stream, b"two").await, b"two");
    support::no_further_exchange(&mut events).await;

    proxy.stop().await;
}

// MARK: - Rules

#[tokio::test]
async fn a_blocked_websocket_never_reaches_the_origin() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    proxy.set_rules(one(rule(
        "block the socket",
        RuleMatcher::any().with_path("/socket"),
        RuleAction::block(),
    )));
    let url = format!("{}/socket", origin.base_url());

    let (handshake, _stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    assert_eq!(handshake.status, 403);
    assert_eq!(origin.handshakes(), 0, "the origin was contacted");

    proxy.stop().await;
}

#[tokio::test]
async fn a_websocket_handshake_is_not_transformed() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    proxy.set_rules(RuleSet::new([
        rule(
            "modify requests",
            RuleMatcher::any(),
            RuleAction::ModifyRequest(rules::ModifyRequest {
                headers: vec![rules::HeaderEdit::Remove {
                    name: "Sec-WebSocket-Key".into(),
                }],
                ..rules::ModifyRequest::default()
            }),
        ),
        rule(
            "modify responses",
            RuleMatcher::any(),
            RuleAction::ModifyResponse(rules::ModifyResponse {
                status: Some(404),
                ..rules::ModifyResponse::default()
            }),
        ),
    ]));
    let url = format!("{}/socket", origin.base_url());

    let (handshake, _stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    // Either rule applied to a handshake would have broken it: the key is
    // what the origin answers from, and the 101 is the upgrade.
    assert_eq!(handshake.status, 101);
    assert_eq!(handshake.header("sec-websocket-accept"), Some(ACCEPT));
    assert_eq!(origin.only_request().header("sec-websocket-key"), Some(KEY));

    proxy.stop().await;
}

// MARK: - Pooling

#[tokio::test]
async fn a_websocket_connection_never_enters_the_http_pool() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);
    assert_eq!(round_trip(&mut stream, b"live").await, b"live");

    // The connection carrying it is not idle, and is not the pool's.
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "a WebSocket connection was pooled"
    );

    // And it is still not pooled once it closes: a socket that carried a
    // WebSocket is not an HTTP connection any more.
    drop(stream);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(proxy.idle_upstream_connections(), 0);

    proxy.stop().await;
}

// MARK: - TLS

#[tokio::test]
async fn a_secure_websocket_is_carried_through_the_interception() {
    let origin = TlsWsOrigin::echoing("localhost").await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let client = HttpsClient::trusting(proxy.trust());

    let (connect, session) = client
        .connect(proxy.addr(), &origin.authority(), origin.host())
        .await;
    assert_eq!(connect.status, 200);
    let mut session = session.expect("the client trusts the proxy's certificate");

    session
        .write_all(&upgrade_request("/socket", &origin.authority()))
        .await
        .expect("send the handshake");
    session.flush().await.expect("flush");

    let handshake = read_handshake(&mut session).await;

    assert_eq!(handshake.status, 101);
    assert_eq!(handshake.header("sec-websocket-accept"), Some(ACCEPT));

    // The upgrade reached the origin through the second TLS session.
    let request = origin.only_request();
    assert_eq!(request.target, "/socket");
    assert_eq!(request.header("sec-websocket-key"), Some(KEY));

    // And the bytes after it cross both sessions unchanged.
    assert_eq!(
        round_trip(&mut session, b"through tls").await,
        b"through tls"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn an_upgrade_missing_what_makes_it_one_is_an_ordinary_request() {
    // Every header but the nonce. This is a request, not a handshake, and a
    // proxy that upgraded it would hand its connection to a tunnel the client
    // is not speaking into.
    let origin = support::Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nplain",
    )
    .await;
    let proxy = Proxy::start().await;

    let response = support::request(
        proxy.addr(),
        "GET",
        &format!("{}/socket", origin.base_url()),
        &[
            ("Connection", "Upgrade"),
            ("Upgrade", "websocket"),
            ("Sec-WebSocket-Version", "13"),
        ],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "plain");
    // Carried as an ordinary request, so the upgrade headers did not cross.
    let request = origin.only_request();
    assert_eq!(request.header("upgrade"), None);
    assert_eq!(request.header("connection"), None);

    proxy.stop().await;
}

#[tokio::test]
async fn an_origin_that_vanishes_mid_stream_ends_the_tunnel() {
    // The origin drops the connection without shutting it down: a process
    // that died, or a network that went away.
    let origin = WsOrigin::start(AGREED, |mut stream| async move {
        let _ = stream.write_all(b"before").await;
        let _ = stream.flush().await;
        drop(stream);
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert_eq!(handshake.status, 101);

    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("the client's side does not hang when the origin's disappears")
        .expect("read to the end");
    assert_eq!(rest, b"before");

    proxy.stop().await;
}

#[tokio::test]
async fn a_mapped_websocket_upgrades_at_the_destination_the_rule_chose() {
    let mapped = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    proxy.set_rules(one(rule(
        "to the socket server",
        RuleMatcher::any().with_host("ws.example.invalid"),
        RuleAction::MapRemote(
            rules::MapRemote::parse(&format!("http://{}", mapped.addr()))
                .expect("a valid destination"),
        ),
    )));

    let (handshake, mut stream) = open(
        proxy.addr(),
        "http://ws.example.invalid/socket",
        "ws.example.invalid",
    )
    .await;

    assert_eq!(handshake.status, 101);
    assert_eq!(handshake.header("sec-websocket-accept"), Some(ACCEPT));

    // The mapped server received the handshake, as a handshake.
    let request = mapped.only_request();
    assert_eq!(request.target, "/socket");
    assert_eq!(request.header("upgrade"), Some("websocket"));
    // And it is told the host it actually is.
    assert_eq!(
        request.header("host"),
        Some(mapped.addr().to_string().as_str())
    );

    assert_eq!(round_trip(&mut stream, b"mapped").await, b"mapped");

    proxy.stop().await;
}

#[tokio::test]
async fn a_websocket_does_not_take_a_pooled_connection() {
    // An ordinary request first, so there is an idle connection to this
    // origin sitting in the pool. A WebSocket that took it would leave the
    // pool holding a socket that is no longer an HTTP connection.
    let origin = WsOrigin::start(AGREED, |stream| async move {
        let (mut from_client, mut to_client) = tokio::io::split(stream);
        let _ = tokio::io::copy(&mut from_client, &mut to_client).await;
    })
    .await;
    // An origin that keeps its connections open, because a connection the
    // origin closed would never have been pooled in the first place and the
    // question here is what the WebSocket does with one that was.
    let ordinary = KeepAliveOrigin::plaintext(OriginPlan::default()).await;
    let proxy = Proxy::start().await;

    let response = support::request(
        proxy.addr(),
        "GET",
        &format!("{}/api", ordinary.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        proxy.idle_upstream_connections(),
        1,
        "the ordinary request should have left a pooled connection"
    );

    let url = format!("{}/socket", origin.base_url());
    let (handshake, mut stream) = open(proxy.addr(), &url, &origin.addr().to_string()).await;

    assert_eq!(handshake.status, 101);
    assert_eq!(round_trip(&mut stream, b"own socket").await, b"own socket");
    // The pooled connection is untouched: the WebSocket opened its own.
    assert_eq!(proxy.idle_upstream_connections(), 1);

    proxy.stop().await;
}
