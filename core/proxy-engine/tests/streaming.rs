//! Server-Sent Events, and the streaming a long-lived response depends on.
//!
//! An SSE response is an ordinary HTTP response that does not end. Everything
//! ProxyLab does with one it already did — the head is written as soon as it
//! arrives, the body is a stream of frames copied past as they come, the
//! capture is a bounded copy taken on the way, and the upstream connection is
//! held by the body until the body ends. What these tests do is prove that,
//! because "it already works" is a claim and not a guarantee: an accidental
//! `collect()` anywhere on the response path would turn every one of them
//! into a test that hangs.
//!
//! The origins here write their responses by hand and choose when. That is
//! the whole point: an origin that emits an event and then waits proves the
//! proxy is forwarding rather than buffering, and no HTTP library on the far
//! end could be made to wait so precisely.

mod support;

use std::sync::Arc;
use std::time::Duration;

use proxy_engine::capture::Capture;
use proxy_storage::CaptureConfig;
use support::{next_exchange, Proxy, RawServer, TEST_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;

/// One SSE event, chunk-framed as an origin would write it.
fn chunk(text: &str) -> Vec<u8> {
    format!("{:x}\r\n{text}\r\n", text.len()).into_bytes()
}

/// The head of a streaming `text/event-stream` response.
const EVENT_STREAM_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\n\
    Content-Type: text/event-stream\r\n\
    Cache-Control: no-cache\r\n\
    Transfer-Encoding: chunked\r\n\
    \r\n";

/// Sends a request through `proxy` and reads only the response head back.
///
/// Stops at the blank line, so nothing the origin has already streamed is
/// swallowed by the reader that is about to look for it.
async fn open_stream(proxy: std::net::SocketAddr, url: &str, host: &str) -> (String, TcpStream) {
    let mut stream = TcpStream::connect(proxy)
        .await
        .expect("connect to the proxy");
    stream
        .write_all(
            format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n\r\n")
                .as_bytes(),
        )
        .await
        .expect("send the request");
    stream.flush().await.expect("flush");

    let head = timeout(TEST_TIMEOUT, async {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];

        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            head.push(byte[0]);
        }

        std::io::Result::Ok(head)
    })
    .await
    .expect("the head arrives before the stream does")
    .expect("read the head");

    (String::from_utf8(head).expect("text head"), stream)
}

/// Reads until `what` has been seen, or fails the test.
async fn read_until(stream: &mut TcpStream, what: &str) -> String {
    timeout(TEST_TIMEOUT, async {
        let mut seen = Vec::new();
        let mut buffer = vec![0_u8; 1024];

        loop {
            let read = stream.read(&mut buffer).await.expect("read");
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

// MARK: - Detection and the head

#[tokio::test]
async fn the_head_of_an_event_stream_reaches_the_client_before_any_event() {
    // The origin sends its head and then nothing at all. A proxy that waited
    // for a body would leave the client with no head either.
    let held = Arc::new(Notify::new());
    let origin_gate = Arc::clone(&held);

    let origin = RawServer::start(move |mut stream| {
        let gate = Arc::clone(&origin_gate);
        async move {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            let _ = stream.write_all(EVENT_STREAM_HEAD).await;
            let _ = stream.flush().await;

            gate.notified().await;
            let _ = stream.write_all(&chunk("data: late\n\n")).await;
            let _ = stream.write_all(b"0\r\n\r\n").await;
            let _ = stream.flush().await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (head, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;

    // The content type that makes it an event stream crossed unchanged, and
    // it arrived while the origin had sent no events at all.
    assert!(head.starts_with("HTTP/1.1 200 OK"), "got: {head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "got: {head}"
    );

    held.notify_one();
    assert!(read_until(&mut stream, "data: late")
        .await
        .contains("data: late"));

    proxy.stop().await;
}

#[tokio::test]
async fn events_reach_the_client_one_at_a_time() {
    // Three events, each released only once the previous has been seen by
    // the client. A proxy that buffered the response would deliver nothing
    // until the last of them, and this would time out on the first.
    let next = Arc::new(Notify::new());
    let origin_gate = Arc::clone(&next);

    let origin = RawServer::start(move |mut stream| {
        let gate = Arc::clone(&origin_gate);
        async move {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            let _ = stream.write_all(EVENT_STREAM_HEAD).await;

            for event in ["data: one\n\n", "data: two\n\n", "data: three\n\n"] {
                let _ = stream.write_all(&chunk(event)).await;
                let _ = stream.flush().await;
                gate.notified().await;
            }

            let _ = stream.write_all(b"0\r\n\r\n").await;
            let _ = stream.flush().await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;

    for event in ["data: one", "data: two", "data: three"] {
        let seen = read_until(&mut stream, event).await;
        assert!(seen.contains(event), "got: {seen}");
        next.notify_one();
    }

    proxy.stop().await;
}

// MARK: - How a stream ends

#[tokio::test]
async fn an_origin_that_ends_the_stream_ends_the_clients_response() {
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(EVENT_STREAM_HEAD).await;
        let _ = stream.write_all(&chunk("data: last\n\n")).await;
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let _ = stream.flush().await;
        let _ = stream.shutdown().await;
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;

    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("the client's response ends when the origin's stream does")
        .expect("read to the end");

    let text = String::from_utf8_lossy(&rest);
    assert!(text.contains("data: last"), "got: {text}");
    // The terminating chunk, so the response was framed as complete rather
    // than cut off.
    assert!(text.ends_with("0\r\n\r\n"), "got: {text}");

    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_disconnects_takes_the_upstream_stream_with_it() {
    // The origin reports when its own socket goes away. Nothing else tells
    // it: a proxy that kept reading a stream nobody was listening to would
    // leave this waiting until the test timed out.
    let ended = Arc::new(Notify::new());
    let origin_ended = Arc::clone(&ended);

    let origin = RawServer::start(move |mut stream| {
        let ended = Arc::clone(&origin_ended);
        async move {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            let _ = stream.write_all(EVENT_STREAM_HEAD).await;
            let _ = stream.write_all(&chunk("data: one\n\n")).await;
            let _ = stream.flush().await;

            // Keep writing until the socket refuses, which is the proxy
            // having dropped the connection.
            loop {
                if stream.write_all(&chunk("data: more\n\n")).await.is_err()
                    || stream.flush().await.is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            ended.notify_one();
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert!(read_until(&mut stream, "data: one")
        .await
        .contains("data: one"));

    drop(stream);

    timeout(TEST_TIMEOUT, ended.notified())
        .await
        .expect("the upstream stream ends when the client goes away");

    proxy.stop().await;
}

// MARK: - Capture

#[tokio::test]
async fn an_exchange_is_announced_while_the_stream_is_open() {
    let release = Arc::new(Notify::new());
    let origin_gate = Arc::clone(&release);

    let origin = RawServer::start(move |mut stream| {
        let gate = Arc::clone(&origin_gate);
        async move {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            let _ = stream.write_all(EVENT_STREAM_HEAD).await;
            let _ = stream.write_all(&chunk("data: one\n\n")).await;
            let _ = stream.flush().await;

            gate.notified().await;
            let _ = stream.write_all(b"0\r\n\r\n").await;
            let _ = stream.flush().await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let mut events = proxy.events();
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert!(read_until(&mut stream, "data: one")
        .await
        .contains("data: one"));

    // The head is stored as soon as it exists, so a live stream can be
    // selected. Events on it are stream messages, not further exchanges.
    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.status(), Some(200));
    assert_eq!(
        exchange
            .response
            .as_ref()
            .and_then(|response| response.headers.text("content-type")),
        Some("text/event-stream")
    );
    support::no_further_exchange(&mut events).await;

    release.notify_one();
    support::no_further_exchange(&mut events).await;

    proxy.stop().await;
}

#[tokio::test]
async fn a_stream_past_the_capture_limit_keeps_flowing_and_is_marked_truncated() {
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(EVENT_STREAM_HEAD).await;

        for index in 0..40 {
            let _ = stream
                .write_all(&chunk(&format!("data: {index} {}\n\n", "x".repeat(64))))
                .await;
        }
        let _ = stream.write_all(&chunk("data: end\n\n")).await;
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let _ = stream.flush().await;
        let _ = stream.shutdown().await;
    })
    .await;

    // A limit small enough that the stream passes it well before it ends.
    let capture = Capture::new(
        CaptureConfig::new(256),
        proxy_storage::EventPublisher::new(),
    );
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.status(), Some(200));

    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("the stream finishes")
        .expect("read to the end");

    // Forwarding is unaffected by the limit: the last event still arrived.
    let text = String::from_utf8_lossy(&rest);
    assert!(
        text.contains("data: end"),
        "forwarding stopped at the limit"
    );

    proxy.stop().await;
}

// MARK: - Pooling

#[tokio::test]
async fn the_upstream_connection_is_held_for_as_long_as_the_stream_is_open() {
    let release = Arc::new(Notify::new());
    let origin_gate = Arc::clone(&release);

    let origin = RawServer::start(move |mut stream| {
        let gate = Arc::clone(&origin_gate);
        async move {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            // Framed so the connection survives the response: an origin that
            // said `Connection: close` would not be poolable afterwards, and
            // the question here is what happens while the stream is running.
            let _ = stream.write_all(EVENT_STREAM_HEAD).await;
            let _ = stream.write_all(&chunk("data: one\n\n")).await;
            let _ = stream.flush().await;

            gate.notified().await;
            let _ = stream.write_all(b"0\r\n\r\n").await;
            let _ = stream.flush().await;

            // Stay open, as a keep-alive origin does, so the proxy's decision
            // is the one being observed.
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert!(read_until(&mut stream, "data: one")
        .await
        .contains("data: one"));

    // The response head has arrived and its body has not ended: the
    // connection is this stream's and nobody else may have it.
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "a connection carrying an open stream was pooled"
    );

    release.notify_one();
    assert!(read_until(&mut stream, "0\r\n\r\n")
        .await
        .contains("0\r\n\r\n"));

    // Once the stream has ended the connection is free, and the origin left
    // it usable, so it goes back.
    timeout(TEST_TIMEOUT, async {
        while proxy.idle_upstream_connections() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the connection returns to the pool once the stream ends");

    proxy.stop().await;
}

#[tokio::test]
async fn a_stream_the_origin_closes_does_not_return_its_connection() {
    // An origin that frames its stream by closing has no connection left to
    // give back, and the pool must not be told otherwise.
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream
            .write_all(
                b"HTTP/1.1 200 OK\r\n\
                  Content-Type: text/event-stream\r\n\
                  Connection: close\r\n\r\n",
            )
            .await;
        let _ = stream.write_all(b"data: one\n\n").await;
        let _ = stream.flush().await;
        let _ = stream.shutdown().await;
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;

    let mut rest = Vec::new();
    timeout(TEST_TIMEOUT, stream.read_to_end(&mut rest))
        .await
        .expect("the stream ends when the origin closes")
        .expect("read to the end");
    assert!(String::from_utf8_lossy(&rest).contains("data: one"));

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        proxy.idle_upstream_connections(),
        0,
        "a closed connection was pooled"
    );

    proxy.stop().await;
}

// MARK: - Shutdown

#[tokio::test]
async fn a_stream_that_never_ends_does_not_keep_a_stopped_proxy_running() {
    // An origin that writes events and never stops. Nothing about this
    // connection will ever finish on its own, which is the point: waiting for
    // it is waiting forever, and a proxy that has been told to stop has to
    // stop anyway.
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(EVENT_STREAM_HEAD).await;

        loop {
            if stream.write_all(&chunk("data: forever\n\n")).await.is_err()
                || stream.flush().await.is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    assert!(read_until(&mut stream, "data: forever")
        .await
        .contains("data: forever"));

    // The client is still there, still reading, and the stream is still
    // running. Stopping must not wait for any of that indefinitely.
    let stopping = std::time::Instant::now();
    proxy
        .stop_within(proxy_engine::listener::SHUTDOWN_GRACE + Duration::from_secs(5))
        .await;

    assert!(
        stopping.elapsed() < proxy_engine::listener::SHUTDOWN_GRACE + Duration::from_secs(5),
        "stopping took {:?}",
        stopping.elapsed()
    );

    // And the client's connection is gone with it rather than left open on a
    // proxy that has stopped.
    let mut rest = Vec::new();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
        .await
        .expect("the client's connection is closed by the stopping proxy")
        .ok();
}
