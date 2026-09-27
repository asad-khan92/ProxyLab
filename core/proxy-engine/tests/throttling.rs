//! Throttling, against the real proxy.
//!
//! Timing tests are the ones most likely to be flaky, so these are written to
//! be lopsided: every ceiling is slow enough that the delay it causes is
//! seconds rather than milliseconds, and every assertion is a floor —
//! "took at least this long" — rather than a window. A loaded machine makes
//! a throttled transfer slower, never faster, so a floor cannot be made to
//! fail by the machine being busy.
//!
//! What matters as much as the shaping is what it does not break, which is
//! why half of these send traffic through a proxy that is shaping nothing and
//! prove it arrives unchanged, and why the streaming and WebSocket tests are
//! here at all: a response that never ends and a connection that is not HTTP
//! are exactly what a naive implementation would buffer and stall.
//!
//! Nothing here reaches the network.

mod support;

use std::time::Duration;

use proxy_ipc::ThrottleSettings;
use support::websocket::{read_handshake, round_trip, upgrade_request, WsOrigin};
use support::{request, Origin, Proxy, RawServer, TEST_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Instant};

/// Reads from `stream` until `what` has been seen, or fails the test.
async fn read_until(stream: &mut TcpStream, what: &str) -> String {
    timeout(TEST_TIMEOUT, async {
        let mut seen = Vec::new();
        let mut buffer = vec![0u8; 1024];

        loop {
            let read = stream.read(&mut buffer).await.expect("read the stream");
            if read == 0 {
                break;
            }
            seen.extend_from_slice(&buffer[..read]);

            if String::from_utf8_lossy(&seen).contains(what) {
                break;
            }
        }

        String::from_utf8_lossy(&seen).into_owned()
    })
    .await
    .unwrap_or_else(|_| panic!("the stream never carried {what:?}"))
}

/// A body big enough that a slow ceiling takes a measurable time to deliver.
const BODY_BYTES: usize = 16 * 1024;

/// A ceiling that turns [`BODY_BYTES`] into roughly two seconds.
const SLOW: u64 = 8 * 1024;

/// A `200 OK` carrying [`BODY_BYTES`] bytes.
fn large_response() -> &'static [u8] {
    static RESPONSE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

    RESPONSE
        .get_or_init(|| {
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {BODY_BYTES}\r\n\r\n"
            )
            .into_bytes();
            response.extend(std::iter::repeat_n(b'x', BODY_BYTES));

            response
        })
        .as_slice()
}

/// Settings that shape nothing but are switched on.
fn shaping(latency_ms: u64, download: u64, upload: u64) -> ThrottleSettings {
    ThrottleSettings {
        enabled: true,
        latency_ms,
        download_bytes_per_second: download,
        upload_bytes_per_second: upload,
    }
}

// MARK: - Latency

#[tokio::test(flavor = "multi_thread")]
async fn added_latency_delays_the_exchange() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(600, 0, 0));

    let started = Instant::now();
    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;
    let took = started.elapsed();

    assert_eq!(response.status, 200);
    assert!(took >= Duration::from_millis(550), "{took:?}");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn latency_that_is_switched_off_delays_nothing() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(ThrottleSettings {
        enabled: false,
        ..shaping(5_000, 0, 0)
    });

    let started = Instant::now();
    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert!(started.elapsed() < Duration::from_secs(2));

    proxy.stop().await;
}

// MARK: - Bandwidth

#[tokio::test(flavor = "multi_thread")]
async fn a_download_ceiling_slows_a_response_body_without_changing_it() {
    let origin = Origin::responding_with(large_response()).await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(0, SLOW, 0));

    let started = Instant::now();
    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/download", origin.addr()),
        &[],
    )
    .await;
    let took = started.elapsed();

    assert_eq!(response.status, 200);
    let body = response.body();
    assert_eq!(body.len(), BODY_BYTES);
    assert!(body.iter().all(|byte| *byte == b'x'));
    // Two seconds' worth at this ceiling; the floor is generous.
    assert!(took >= Duration::from_millis(1_200), "{took:?}");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_ceiling_delivers_the_same_body_at_full_speed() {
    let origin = Origin::responding_with(large_response()).await;
    let proxy = Proxy::start().await;

    let started = Instant::now();
    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/download", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body().len(), BODY_BYTES);
    assert!(started.elapsed() < Duration::from_secs(2));

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_ceiling_slows_a_request_body_without_changing_it() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(0, 0, SLOW));

    let body = "u".repeat(BODY_BYTES);
    let started = Instant::now();
    let response = support::request_with_body(
        proxy.addr(),
        "POST",
        &format!("http://{}/upload", origin.addr()),
        &[],
        &body,
    )
    .await;
    let took = started.elapsed();

    assert_eq!(response.status, 200);
    assert_eq!(origin.only_request().body_text().len(), BODY_BYTES);
    assert!(took >= Duration::from_millis(1_200), "{took:?}");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_ceiling_leaves_the_upload_direction_alone() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(0, SLOW, 0));

    let body = "u".repeat(BODY_BYTES);
    let started = Instant::now();
    let response = support::request_with_body(
        proxy.addr(),
        "POST",
        &format!("http://{}/upload", origin.addr()),
        &[],
        &body,
    )
    .await;

    assert_eq!(response.status, 200);
    // The two-byte response is all that is paced, which is no time at all.
    assert!(started.elapsed() < Duration::from_secs(2));

    proxy.stop().await;
}

// MARK: - What must not break

#[tokio::test(flavor = "multi_thread")]
async fn a_server_sent_event_stream_still_arrives_as_it_is_written() {
    // An SSE response never ends, so a proxy that buffered one in order to
    // pace it would hang here rather than fail an assertion.
    let origin = RawServer::start(|mut stream| async move {
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await;

        for part in [
            &b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
            &b"9\r\ndata: 1\n\n\r\n"[..],
            &b"9\r\ndata: 2\n\n\r\n"[..],
        ] {
            if stream.write_all(part).await.is_err() {
                return;
            }
            let _ = stream.flush().await;
        }

        // Held open, as an event stream is.
        std::future::pending::<()>().await;
    })
    .await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(0, SLOW, 0));

    let mut stream = TcpStream::connect(proxy.addr())
        .await
        .expect("connect to the proxy");
    stream
        .write_all(
            format!(
                "GET http://{0}/events HTTP/1.1\r\nHost: {0}\r\n\r\n",
                origin.addr()
            )
            .as_bytes(),
        )
        .await
        .expect("write the request");

    let seen = read_until(&mut stream, "data: 2").await;

    assert!(seen.contains("data: 1"), "{seen}");
    assert!(seen.contains("data: 2"), "{seen}");

    // The stream is still open and still throttled, which is the shape an
    // event stream has. Stopping must not wait for it — see the same
    // arrangement in `streaming.rs`.
    proxy
        .stop_within(proxy_engine::listener::SHUTDOWN_GRACE + Duration::from_secs(5))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_websocket_still_carries_frames_in_both_directions() {
    let origin = WsOrigin::echoing().await;
    let proxy = Proxy::start().await;
    proxy.controls().throttle().set(shaping(0, SLOW, SLOW));

    let mut stream = TcpStream::connect(proxy.addr())
        .await
        .expect("connect to the proxy");
    stream
        .write_all(&upgrade_request(
            &format!("{}/socket", origin.base_url()),
            &origin.addr().to_string(),
        ))
        .await
        .expect("send the handshake");
    stream.flush().await.expect("flush");

    let handshake = read_handshake(&mut stream).await;
    assert_eq!(handshake.status, 101);

    assert_eq!(round_trip(&mut stream, b"hello").await, b"hello");
    assert_eq!(round_trip(&mut stream, b"again").await, b"again");

    proxy.stop().await;
}

// MARK: - The settings themselves

#[tokio::test(flavor = "multi_thread")]
async fn settings_take_effect_from_the_next_exchange() {
    let origin = Origin::responding_with(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").await;
    let proxy = Proxy::start().await;

    let url = format!("http://{}/hello", origin.addr());
    assert_eq!(request(proxy.addr(), "GET", &url, &[]).await.status, 200);

    proxy.controls().throttle().set(shaping(400, 0, 0));
    let started = Instant::now();
    assert_eq!(request(proxy.addr(), "GET", &url, &[]).await.status, 200);
    assert!(started.elapsed() >= Duration::from_millis(350));

    proxy.controls().throttle().set(ThrottleSettings::none());
    let started = Instant::now();
    assert_eq!(request(proxy.addr(), "GET", &url, &[]).await.status, 200);
    assert!(started.elapsed() < Duration::from_secs(2));

    proxy.stop().await;
}
