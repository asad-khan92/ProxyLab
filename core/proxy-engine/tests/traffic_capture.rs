//! End-to-end tests for traffic capture on the plaintext forwarding path.
//!
//! Every test runs the real proxy and an in-process origin server on
//! OS-assigned ports, sends a real request through the proxy, and reads the
//! exchange the proxy published. Nothing here reaches the network, DNS, or
//! any external service.
//!
//! Two things are checked side by side throughout: what the client received,
//! and what the capture recorded. They are not the same question — a capture
//! limit changes the second and must never change the first — so where both
//! matter, no test asserts one without the other.

mod support;

use support::{
    next_exchange, next_exchanges, no_further_exchange, request, request_with_body, send_raw,
    Origin, Proxy,
};

use std::collections::BTreeSet;

use proxy_engine::capture::Capture;
use proxy_storage::{CaptureConfig, EventPublisher, Scheme, TrafficError, TrafficErrorKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

/// A capture that keeps at most `limit` bytes of each body.
fn capturing(limit: usize) -> Capture {
    Capture::new(CaptureConfig::new(limit), EventPublisher::new())
}

/// A response with `body` as its payload, framed by `Content-Length`.
fn response_with(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);

    response
}

/// An address nothing is listening on: a port is bound to learn a free one
/// and then released, so connecting to it is refused rather than ignored.
async fn unused_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    drop(listener);

    addr.to_string()
}

#[tokio::test]
async fn a_forwarded_request_publishes_a_completed_exchange() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.status, 200);

    let exchange = next_exchange(&mut events).await;

    assert!(exchange.succeeded(), "{}", exchange.summary());
    assert_eq!(exchange.request.method, "GET");
    assert_eq!(exchange.status(), Some(200));
    assert_eq!(exchange.error, None);
    // One request, one exchange: a subscriber is not asked to piece a
    // request together out of several events.
    no_further_exchange(&mut events).await;

    proxy.stop().await;
}

#[tokio::test]
async fn the_captured_url_is_the_absolute_url_that_was_asked_for() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();
    let url = format!("{}/api/users?active=true", origin.base_url());

    request(proxy.addr(), "GET", &url, &[]).await;

    let exchange = next_exchange(&mut events).await;

    assert_eq!(exchange.request.url, url);
    assert_eq!(exchange.connection.scheme, Scheme::Http);
    assert_eq!(
        exchange
            .connection
            .destination
            .as_ref()
            .map(|destination| destination.port),
        Some(origin.addr().port())
    );

    proxy.stop().await;
}

#[tokio::test]
async fn the_captured_url_does_not_repeat_credentials_from_the_request_target() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "GET",
        &format!("http://alice:hunter2@{}/secret", origin.addr()),
        &[],
    )
    .await;

    let exchange = next_exchange(&mut events).await;

    assert!(
        !exchange.request.url.contains("hunter2"),
        "captured a password: {}",
        exchange.request.url
    );
    assert!(exchange.request.url.ends_with("/secret"));

    proxy.stop().await;
}

#[tokio::test]
async fn request_headers_are_captured_as_the_client_sent_them() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[
            ("Accept", "application/json"),
            ("X-Request-Id", "abc-123"),
            ("Authorization", "Bearer sk-secret"),
        ],
    )
    .await;

    let headers = &next_exchange(&mut events).await.request.headers;

    assert_eq!(headers.text("accept"), Some("application/json"));
    assert_eq!(headers.text("x-request-id"), Some("abc-123"));
    assert_eq!(
        headers.text("host"),
        Some(origin.addr().to_string().as_str())
    );
    // Credentials are captured, because the debugging UI needs them. What
    // must not happen is their reaching a log, which is a separate test.
    assert_eq!(headers.text("authorization"), Some("Bearer sk-secret"));

    proxy.stop().await;
}

#[tokio::test]
async fn response_headers_and_status_are_captured_as_the_origin_sent_them() {
    let origin = Origin::responding_with(
        b"HTTP/1.1 201 Created\r\nLocation: /users/1\r\nSet-Cookie: session=abc\r\nContent-Length: 2\r\n\r\nok",
    )
    .await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "POST",
        &format!("{}/users", origin.base_url()),
        &[],
    )
    .await;

    let exchange = next_exchange(&mut events).await;
    let response = exchange.response.as_ref().expect("a response was captured");

    assert_eq!(response.status, 201);
    assert_eq!(response.reason.as_deref(), Some("Created"));
    assert_eq!(response.headers.text("location"), Some("/users/1"));
    assert_eq!(response.headers.text("set-cookie"), Some("session=abc"));
    assert_eq!(response.headers.text("content-length"), Some("2"));

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_body_is_captured_and_still_reaches_the_origin() {
    let origin =
        Origin::responding_with(b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\n\r\ncreated").await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request_with_body(
        proxy.addr(),
        "POST",
        &format!("{}/users", origin.base_url()),
        &[("Content-Type", "application/json")],
        r#"{"name":"ada"}"#,
    )
    .await;
    assert_eq!(response.status, 201);

    // The origin received it, and the capture kept a copy of the same bytes.
    assert_eq!(origin.only_request().body_text(), r#"{"name":"ada"}"#);

    let exchange = next_exchange(&mut events).await;
    let body = &exchange.request.body;
    assert_eq!(body.as_text(), Some(r#"{"name":"ada"}"#));
    assert_eq!(body.size(), 14);
    assert!(!body.is_truncated());

    proxy.stop().await;
}

#[tokio::test]
async fn a_response_body_is_captured_and_still_reaches_the_client() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.body_text(), "hello");

    let exchange = next_exchange(&mut events).await;
    let body = &exchange.response.as_ref().expect("a response").body;

    assert_eq!(body.as_text(), Some("hello"));
    assert_eq!(body.size(), 5);
    assert!(!body.is_truncated());

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_with_no_body_captures_an_empty_body() {
    let origin = Origin::responding_with(b"HTTP/1.1 204 No Content\r\n\r\n").await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "GET",
        &format!("{}/nothing", origin.base_url()),
        &[],
    )
    .await;

    let exchange = next_exchange(&mut events).await;

    assert!(exchange.request.body.is_empty());
    assert!(!exchange.request.body.is_truncated());
    assert!(exchange
        .response
        .as_ref()
        .expect("a response")
        .body
        .is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn timing_is_populated_for_a_completed_exchange() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    let timing = next_exchange(&mut events).await.timing;

    // No exact durations are asserted: what matters is that every mark was
    // taken, that they are in order, and that they came from a clock that
    // only moves forwards.
    assert!(timing.started_at.seconds > 1_600_000_000);
    let connect = timing
        .upstream_connect_micros
        .expect("the upstream connection was timed");
    let response = timing
        .upstream_response_micros
        .expect("the origin's answer was timed");
    let completed = timing.completed_micros.expect("the exchange was timed");

    assert!(connect <= response, "{connect} > {response}");
    assert!(response <= completed, "{response} > {completed}");
    assert!(timing.duration_ms().is_some());

    proxy.stop().await;
}

#[tokio::test]
async fn a_body_under_the_limit_is_captured_whole() {
    let body = vec![b'x'; 512];
    let expected = body.clone();
    let origin = Origin::start(move |_| response_with(&expected)).await;
    let proxy = Proxy::with_capture(capturing(1024)).await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/big", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.body(), body);

    let exchange = next_exchange(&mut events).await;
    let captured = &exchange.response.as_ref().expect("a response").body;

    assert_eq!(captured.data(), body);
    assert_eq!(captured.size(), 512);
    assert!(!captured.is_truncated());

    proxy.stop().await;
}

#[tokio::test]
async fn a_body_over_the_limit_is_forwarded_in_full_and_captured_in_part() {
    // Far larger than the limit, and larger than a single read, so the body
    // really does arrive in pieces the way a download does.
    let body = vec![b'x'; 200_000];
    let expected = body.clone();
    let origin = Origin::start(move |_| response_with(&expected)).await;
    let proxy = Proxy::with_capture(capturing(1_024)).await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/big", origin.base_url()),
        &[],
    )
    .await;

    // The client got all of it. This is the assertion the whole feature
    // hangs on: a capture limit is a limit on what is remembered, never on
    // what is proxied.
    assert_eq!(response.body().len(), 200_000);
    assert_eq!(response.body(), body);

    let exchange = next_exchange(&mut events).await;
    let captured = &exchange.response.as_ref().expect("a response").body;

    assert_eq!(captured.captured_len(), 1_024);
    assert_eq!(captured.size(), 200_000);
    assert!(captured.is_truncated());
    assert_eq!(captured.data(), vec![b'x'; 1_024]);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_body_over_the_limit_is_forwarded_in_full_and_captured_in_part() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::with_capture(capturing(8)).await;
    let mut events = proxy.events();
    let sent = "abcdefghijklmnopqrstuvwxyz";

    request_with_body(
        proxy.addr(),
        "POST",
        &format!("{}/upload", origin.base_url()),
        &[],
        sent,
    )
    .await;

    assert_eq!(origin.only_request().body_text(), sent);

    let exchange = next_exchange(&mut events).await;
    let captured = &exchange.request.body;

    assert_eq!(captured.as_text(), Some("abcdefgh"));
    assert_eq!(captured.size(), 26);
    assert!(captured.is_truncated());

    proxy.stop().await;
}

#[tokio::test]
async fn a_capture_limit_of_zero_keeps_the_headers_and_none_of_the_body() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::with_capture(capturing(0)).await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.body_text(), "hello");

    let exchange = next_exchange(&mut events).await;
    let captured = &exchange.response.as_ref().expect("a response").body;

    assert_eq!(captured.captured_len(), 0);
    assert_eq!(captured.size(), 5);
    assert!(captured.is_truncated());
    assert_eq!(
        exchange
            .response
            .as_ref()
            .expect("a response")
            .headers
            .text("content-type"),
        Some("text/plain")
    );

    proxy.stop().await;
}

#[tokio::test]
async fn binary_bodies_survive_the_proxy_and_the_capture_byte_for_byte() {
    let body: Vec<u8> = (0..=255).collect();
    let expected = body.clone();
    let origin = Origin::start(move |_| response_with(&expected)).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/binary", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.body(), body);

    let exchange = next_exchange(&mut events).await;
    let captured = &exchange.response.as_ref().expect("a response").body;

    assert_eq!(captured.data(), body);
    // Nothing pretends a byte string is text.
    assert_eq!(captured.as_text(), None);

    proxy.stop().await;
}

#[tokio::test]
async fn utf8_bodies_survive_the_proxy_and_the_capture_exactly() {
    let text = "héllo — wörld ✓";
    let origin = Origin::start(move |_| response_with(text.as_bytes())).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/text", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.body(), text.as_bytes());

    let exchange = next_exchange(&mut events).await;

    assert_eq!(
        exchange
            .response
            .as_ref()
            .expect("a response")
            .body
            .as_text(),
        Some(text)
    );

    proxy.stop().await;
}

#[tokio::test]
async fn every_subscriber_receives_the_same_exchange() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut first = proxy.events();
    let mut second = proxy.events();
    let mut third = proxy.events();

    request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    let one = next_exchange(&mut first).await;
    let two = next_exchange(&mut second).await;
    let three = next_exchange(&mut third).await;

    assert_eq!(one.id, two.id);
    assert_eq!(two.id, three.id);
    assert_eq!(one.request.url, three.request.url);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_requests_publish_independent_exchanges() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let requested: Vec<String> = (0..8)
        .map(|index| format!("{}/path/{index}", origin.base_url()))
        .collect();

    let mut clients = Vec::new();
    for url in &requested {
        let addr = proxy.addr();
        let url = url.clone();
        clients.push(tokio::spawn(async move {
            request(addr, "GET", &url, &[]).await
        }));
    }

    for client in clients {
        let response = client.await.expect("client task does not panic");
        assert_eq!(response.status, 200);
    }

    let exchanges = next_exchanges(&mut events, 8).await;

    let ids: BTreeSet<proxy_storage::ExchangeId> =
        exchanges.iter().map(|exchange| exchange.id).collect();
    assert_eq!(ids.len(), 8, "exchanges shared an id");

    let urls: BTreeSet<String> = exchanges
        .iter()
        .map(|exchange| exchange.request.url.clone())
        .collect();
    assert_eq!(urls, requested.into_iter().collect::<BTreeSet<String>>());

    for exchange in &exchanges {
        assert!(exchange.succeeded(), "{}", exchange.summary());
    }

    proxy.stop().await;
}

#[tokio::test]
async fn a_subscriber_that_never_reads_does_not_stop_the_proxy() {
    let origin = Origin::responding_with(HELLO).await;
    // A channel that holds four events, and a subscriber that reads none of
    // them: every request after the fourth has nowhere to be queued.
    let capture = Capture::new(CaptureConfig::default(), EventPublisher::with_capacity(4));
    let proxy = Proxy::with_capture(capture).await;
    let _never_reads = proxy.events();

    for index in 0..20 {
        let response = request(
            proxy.addr(),
            "GET",
            &format!("{}/path/{index}", origin.base_url()),
            &[],
        )
        .await;

        assert_eq!(response.status, 200, "request {index} was not served");
        assert_eq!(response.body_text(), "hello");
    }

    // And a subscriber attaching now still receives what follows, so the
    // stream was not broken by the one that fell behind.
    let mut fresh = proxy.events();
    request(
        proxy.addr(),
        "GET",
        &format!("{}/after", origin.base_url()),
        &[],
    )
    .await;
    assert!(next_exchange(&mut fresh)
        .await
        .request
        .url
        .ends_with("/after"));

    proxy.stop().await;
}

#[tokio::test]
async fn an_unreachable_origin_publishes_an_exchange_with_no_response() {
    let addr = unused_addr().await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = request(proxy.addr(), "GET", &format!("http://{addr}/x"), &[]).await;
    assert_eq!(response.status, 502);

    let exchange = next_exchange(&mut events).await;

    // No status is invented for a transport failure.
    assert_eq!(exchange.response, None);
    assert_eq!(exchange.status(), None);
    let error = exchange.error.as_ref().expect("a failure was captured");
    assert_eq!(error.kind(), TrafficErrorKind::UpstreamConnect);
    assert_eq!(
        error.summary(),
        "could not connect to the destination server"
    );
    // Nothing of the underlying operating system error is repeated.
    assert!(!error.summary().contains("os error"), "{error}");

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_the_proxy_refuses_publishes_a_rejected_exchange() {
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    let response = send_raw(
        proxy.addr(),
        b"GET /index.html HTTP/1.1\r\nHost: example.test\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 400);

    let exchange = next_exchange(&mut events).await;

    assert_eq!(exchange.request.url, "/index.html");
    assert_eq!(exchange.response, None);
    assert_eq!(
        exchange.error.as_ref().map(TrafficError::kind),
        Some(TrafficErrorKind::RequestRejected)
    );
    assert_eq!(exchange.connection.destination, None);

    proxy.stop().await;
}

#[tokio::test]
async fn an_origin_that_stops_mid_body_publishes_a_failed_exchange() {
    // A response that promises a hundred bytes and sends five, then closes.
    let origin =
        Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort").await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    // The client's own read may end in an error or a short body; either way
    // the proxy must not panic, and must say what happened.
    let _ = send_raw(
        proxy.addr(),
        format!(
            "GET {}/short HTTP/1.1\r\nHost: {}\r\n\r\n",
            origin.base_url(),
            origin.addr()
        )
        .as_bytes(),
    )
    .await;

    let exchange = next_exchange(&mut events).await;

    assert!(!exchange.succeeded(), "{}", exchange.summary());
    // The head did arrive, so it is captured; the body did not finish, so
    // the exchange says so rather than claiming a clean 200.
    assert_eq!(exchange.status(), Some(200));
    assert!(exchange.error.is_some(), "{}", exchange.summary());

    // The proxy is still serving.
    let healthy = Origin::responding_with(HELLO).await;
    assert_eq!(
        request(
            proxy.addr(),
            "GET",
            &format!("{}/hello", healthy.base_url()),
            &[]
        )
        .await
        .status,
        200
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_stops_reading_mid_response_publishes_what_it_received() {
    // Far more than a socket buffer holds, so the proxy is still writing when
    // the client goes away rather than having already finished.
    let body = vec![b'x'; 2_000_000];
    let origin = Origin::start(move |_| response_with(&body)).await;
    let proxy = Proxy::with_capture(capturing(1_024)).await;
    let mut events = proxy.events();

    {
        let mut client = TcpStream::connect(proxy.addr())
            .await
            .expect("connect to the proxy");
        client
            .write_all(
                format!(
                    "GET {}/big HTTP/1.1\r\nHost: {}\r\n\r\n",
                    origin.base_url(),
                    origin.addr()
                )
                .as_bytes(),
            )
            .await
            .expect("the request is sent");
        client.flush().await.expect("the request is flushed");

        // Read enough to know the answer started, then go away mid-body.
        let mut head = [0_u8; 64];
        let read = client.read(&mut head).await.expect("the answer starts");
        assert!(read > 0, "the proxy sent nothing at all");
    }

    // A request that vanished would be the worst answer a debugging proxy
    // could give, so the exchange is published with what did happen.
    let exchange = next_exchange(&mut events).await;

    assert_eq!(exchange.status(), Some(200));
    assert!(!exchange.succeeded(), "{}", exchange.summary());
    assert_eq!(
        exchange.error.as_ref().map(TrafficError::kind),
        Some(TrafficErrorKind::ClientDisconnected)
    );

    // And the proxy is still serving.
    let healthy = Origin::responding_with(HELLO).await;
    assert_eq!(
        request(
            proxy.addr(),
            "GET",
            &format!("{}/hello", healthy.base_url()),
            &[]
        )
        .await
        .status,
        200
    );

    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_hangs_up_before_reading_anything_does_not_stop_the_proxy() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    // Sent and gone. hyper gives the connection up before the request is
    // dispatched, so there is no exchange to publish — and, more to the
    // point, nothing panics and no task is left stuck.
    {
        let mut client = TcpStream::connect(proxy.addr())
            .await
            .expect("connect to the proxy");
        client
            .write_all(
                format!(
                    "GET {}/hello HTTP/1.1\r\nHost: {}\r\n\r\n",
                    origin.base_url(),
                    origin.addr()
                )
                .as_bytes(),
            )
            .await
            .expect("the request is sent");
        client.flush().await.expect("the request is flushed");
    }

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");

    proxy.stop().await;
}

#[tokio::test]
async fn a_captured_exchange_keeps_secrets_out_of_its_diagnostic_forms() {
    let origin = Origin::responding_with(
        b"HTTP/1.1 200 OK\r\nSet-Cookie: session=s3cr3t-cookie\r\nContent-Length: 24\r\n\r\n{\"token\":\"s3cr3t-token\"}",
    )
    .await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();

    request_with_body(
        proxy.addr(),
        "POST",
        &format!("{}/login", origin.base_url()),
        &[
            ("Authorization", "Bearer s3cr3t-bearer"),
            ("Cookie", "sid=s3cr3t-sid"),
            ("X-Api-Key", "s3cr3t-key"),
        ],
        r#"{"password":"s3cr3t-password"}"#,
    )
    .await;

    let exchange = next_exchange(&mut events).await;

    // The capture holds them, because that is what the debugging UI is for.
    let request_headers = &exchange.request.headers;
    assert_eq!(
        request_headers.text("authorization"),
        Some("Bearer s3cr3t-bearer")
    );
    assert_eq!(request_headers.text("cookie"), Some("sid=s3cr3t-sid"));
    assert_eq!(request_headers.text("x-api-key"), Some("s3cr3t-key"));
    assert_eq!(
        exchange.request.body.as_text(),
        Some(r#"{"password":"s3cr3t-password"}"#)
    );
    let response = exchange.response.as_ref().expect("a response");
    assert_eq!(
        response.headers.text("set-cookie"),
        Some("session=s3cr3t-cookie")
    );

    // And none of them can reach a log, whichever diagnostic form is used.
    let secrets = [
        "s3cr3t-bearer",
        "s3cr3t-sid",
        "s3cr3t-key",
        "s3cr3t-password",
        "s3cr3t-cookie",
        "s3cr3t-token",
    ];
    for rendered in [format!("{exchange:?}"), exchange.summary()] {
        for secret in secrets {
            assert!(
                !rendered.contains(secret),
                "a diagnostic rendering leaked {secret}: {rendered}"
            );
        }
    }

    proxy.stop().await;
}
