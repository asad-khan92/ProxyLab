//! SSE events captured individually, with the existing streaming path intact.

mod support;

use std::sync::Arc;

use proxy_engine::capture::Capture;
use proxy_storage::{CaptureConfig, EventPublisher, StreamDirection, StreamKind};
use support::{next_exchange, next_stream_message, Proxy, RawServer, TEST_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::timeout;

fn chunk(text: &str) -> Vec<u8> {
    format!("{:x}\r\n{text}\r\n", text.len()).into_bytes()
}

const EVENT_STREAM_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\n\
    Content-Type: text/event-stream\r\n\
    Cache-Control: no-cache\r\n\
    Transfer-Encoding: chunked\r\n\
    \r\n";

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
    .expect("the head arrives")
    .expect("read the head");

    (String::from_utf8(head).expect("text head"), stream)
}

async fn read_until(stream: &mut TcpStream, what: &str) -> String {
    timeout(TEST_TIMEOUT, async {
        let mut seen = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await?;
            seen.push(byte[0]);
            if seen
                .windows(what.len())
                .any(|window| window == what.as_bytes())
            {
                return std::io::Result::Ok(String::from_utf8_lossy(&seen).into_owned());
            }
        }
    })
    .await
    .expect("the bytes arrive")
    .expect("read")
}

#[tokio::test]
async fn sse_events_are_captured_individually_as_they_stream() {
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(EVENT_STREAM_HEAD).await;
        let _ = stream
            .write_all(&chunk(
                "event: update\nid: 42\nretry: 3000\ndata: hello\n\n",
            ))
            .await;
        let _ = stream.write_all(&chunk("data: two\n\n")).await;
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let _ = stream.flush().await;
    })
    .await;

    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.status(), Some(200));

    assert!(read_until(&mut stream, "data: hello")
        .await
        .contains("data: hello"));

    let first = next_stream_message(&mut streams).await;
    assert_eq!(first.exchange_id, exchange.id);
    assert_eq!(first.sequence, 0);
    assert_eq!(first.direction, StreamDirection::ServerToClient);
    assert_eq!(first.kind, StreamKind::Sse);
    assert_eq!(first.event_name.as_deref(), Some("update"));
    assert_eq!(first.event_id.as_deref(), Some("42"));
    assert_eq!(first.retry_ms, Some(3_000));
    assert_eq!(first.payload_size, 5);

    assert!(read_until(&mut stream, "data: two")
        .await
        .contains("data: two"));
    let second = next_stream_message(&mut streams).await;
    assert_eq!(second.sequence, 1);
    assert!(second.event_name.is_none());
    assert_eq!(second.payload_size, 3);
    assert!(second.captured_at >= first.captured_at);

    proxy.stop().await;
}

#[tokio::test]
async fn a_long_sse_event_is_truncated_and_the_stream_keeps_flowing() {
    let origin = RawServer::start(|mut stream| async move {
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
        let _ = stream.write_all(EVENT_STREAM_HEAD).await;
        let _ = stream
            .write_all(&chunk(&format!("data: {}\n\n", "x".repeat(64))))
            .await;
        let _ = stream.write_all(&chunk("data: end\n\n")).await;
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let _ = stream.flush().await;
    })
    .await;

    let capture = Capture::new(CaptureConfig::new(8), EventPublisher::new());
    let mut streams = capture.subscribe_streams();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    let rest = timeout(TEST_TIMEOUT, async {
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await?;
        std::io::Result::Ok(rest)
    })
    .await
    .expect("the stream finishes")
    .expect("read");
    let text = String::from_utf8_lossy(&rest);
    assert!(text.contains("data: end"), "forwarding stopped: {text}");

    let first = next_stream_message(&mut streams).await;
    assert!(first.payload_truncated);
    assert_eq!(first.payload_captured_len, 8);
    assert!(first.payload_size > 8);

    let second = next_stream_message(&mut streams).await;
    assert_eq!(second.payload_size, 3);
    assert!(!second.payload_truncated);

    proxy.stop().await;
}

#[tokio::test]
async fn a_running_stream_is_announced_once_and_not_per_event() {
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

    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("http://{}/events", origin.addr());

    let (_, mut stream) = open_stream(proxy.addr(), &url, &origin.addr().to_string()).await;
    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.status(), Some(200));

    assert!(read_until(&mut stream, "data: one")
        .await
        .contains("data: one"));
    let event = next_stream_message(&mut streams).await;
    assert_eq!(event.exchange_id, exchange.id);
    assert_eq!(event.kind, StreamKind::Sse);

    support::no_further_exchange(&mut events).await;

    release.notify_one();
    support::no_further_exchange(&mut events).await;

    proxy.stop().await;
}
