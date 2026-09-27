//! WebSocket messages: captured individually, in order, and editable as text.

mod support;

use std::sync::Arc;

use proxy_engine::capture::Capture;
use proxy_ipc::{
    BreakpointControl, BreakpointDirection, NewBreakpoint, PauseDecision, PauseNotice, PauseStream,
    WebSocketEdit,
};
use proxy_storage::{StreamDirection, StreamKind};
use rules::RuleMatcher;
use support::websocket::{read_exactly, read_handshake, upgrade_request, WsOrigin, AGREED};
use support::{next_exchange, next_stream_message, Proxy, TEST_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

fn text_frame(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 126, "these tests send short frames");
    let mut frame = vec![0x81, payload.len() as u8];
    frame.extend_from_slice(payload);
    frame
}

fn binary_frame(payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 126, "these tests send short frames");
    let mut frame = vec![0x82, payload.len() as u8];
    frame.extend_from_slice(payload);
    frame
}

/// The payload of one short text or binary frame, unmasked.
fn frame_payload(frame: &[u8]) -> Vec<u8> {
    assert!(frame.len() >= 2, "a frame is at least two bytes: {frame:?}");
    let masked = frame[1] & 0x80 != 0;
    let len = (frame[1] & 0x7f) as usize;
    let mut offset = 2;
    let mask = if masked {
        assert!(frame.len() >= 6, "a masked frame names its key: {frame:?}");
        let key = [frame[2], frame[3], frame[4], frame[5]];
        offset = 6;
        Some(key)
    } else {
        None
    };
    let mut payload = frame[offset..offset + len].to_vec();
    if let Some(key) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[index % 4];
        }
    }
    payload
}

async fn open(proxy: std::net::SocketAddr, url: &str, host: &str) -> TcpStream {
    let mut stream = TcpStream::connect(proxy)
        .await
        .expect("connect to the proxy");
    stream
        .write_all(&upgrade_request(url, host))
        .await
        .expect("send the handshake");
    stream.flush().await.expect("flush");
    let handshake = read_handshake(&mut stream).await;
    assert_eq!(handshake.status, 101);
    stream
}

fn watching(proxy: &Proxy) -> PauseStream {
    PauseStream::attach(Arc::clone(proxy.controls().breakpoints()) as Arc<dyn BreakpointControl>)
}

async fn next_pause(watcher: &mut PauseStream) -> proxy_ipc::PausedExchange {
    loop {
        match timeout(TEST_TIMEOUT, watcher.recv())
            .await
            .expect("a message is paused")
            .expect("the pause stream is healthy")
        {
            PauseNotice::Paused(paused) => return paused,
            PauseNotice::Released { .. } => continue,
        }
    }
}

#[tokio::test]
async fn text_messages_are_captured_individually_in_both_directions() {
    let origin = WsOrigin::echoing().await;
    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    let handshake = next_exchange(&mut events).await;
    assert_eq!(handshake.status(), Some(101));

    let hello = text_frame(b"hello");
    stream.write_all(&hello).await.expect("send");
    stream.flush().await.expect("flush");
    assert_eq!(read_exactly(&mut stream, hello.len()).await, hello);

    let up = next_stream_message(&mut streams).await;
    assert_eq!(up.exchange_id, handshake.id);
    assert_eq!(up.sequence, 0);
    assert_eq!(up.direction, StreamDirection::ClientToServer);
    assert_eq!(up.kind, StreamKind::WebsocketText);
    assert_eq!(up.payload_size, 5);

    let down = next_stream_message(&mut streams).await;
    assert_eq!(down.exchange_id, handshake.id);
    assert_eq!(down.sequence, 1);
    assert_eq!(down.direction, StreamDirection::ServerToClient);
    assert_eq!(down.kind, StreamKind::WebsocketText);
    assert_eq!(down.payload_size, 5);
    assert!(down.captured_at >= up.captured_at);

    proxy.stop().await;
}

#[tokio::test]
async fn a_binary_payload_survives_unchanged() {
    let origin = WsOrigin::echoing().await;
    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    let payload = [0x00, 0xff, 0x7f];
    let frame = binary_frame(&payload);
    stream.write_all(&frame).await.expect("send");
    stream.flush().await.expect("flush");
    assert_eq!(read_exactly(&mut stream, frame.len()).await, frame);

    let up = next_stream_message(&mut streams).await;
    assert_eq!(up.kind, StreamKind::WebsocketBinary);
    assert_eq!(up.payload_size, 3);
    assert!(!up.payload_truncated);

    let down = next_stream_message(&mut streams).await;
    assert_eq!(down.kind, StreamKind::WebsocketBinary);
    assert_eq!(down.payload_size, 3);

    proxy.stop().await;
}

#[tokio::test]
async fn json_text_is_captured_as_the_bytes_that_were_sent() {
    let origin = WsOrigin::echoing().await;
    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    let json = br#"{"ok":true,"n":1}"#;
    let frame = text_frame(json);
    stream.write_all(&frame).await.expect("send");
    stream.flush().await.expect("flush");
    assert_eq!(read_exactly(&mut stream, frame.len()).await, frame);

    let up = next_stream_message(&mut streams).await;
    assert_eq!(up.kind, StreamKind::WebsocketText);
    assert_eq!(up.payload_size, json.len() as u64);

    proxy.stop().await;
}

#[tokio::test]
async fn opaque_bytes_are_still_forwarded_without_being_messages() {
    let origin = WsOrigin::echoing().await;
    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let proxy = Proxy::with_capture(capture).await;
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    stream.write_all(b"not-a-frame").await.expect("send");
    stream.flush().await.expect("flush");
    assert_eq!(read_exactly(&mut stream, 11).await, b"not-a-frame");

    let extra = timeout(std::time::Duration::from_millis(200), streams.recv()).await;
    assert!(extra.is_err(), "opaque bytes must not become messages");

    proxy.stop().await;
}

#[tokio::test]
async fn an_edited_text_message_is_what_the_origin_echoes() {
    let origin = WsOrigin::echoing().await;
    let capture = Capture::with_defaults();
    let mut streams = capture.subscribe_streams();
    let proxy = Proxy::with_capture(capture).await;
    let mut watcher = watching(&proxy);
    proxy.controls().breakpoints().create(NewBreakpoint::new(
        BreakpointDirection::WebSocket,
        RuleMatcher::any().with_host(origin.addr().ip().to_string()),
    ));
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    stream.write_all(&text_frame(b"hello")).await.expect("send");
    stream.flush().await.expect("flush");

    let paused = next_pause(&mut watcher).await;
    assert_eq!(paused.direction, BreakpointDirection::WebSocket);
    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        PauseDecision::Continue {
            request: None,
            response: None,
            websocket: Some(WebSocketEdit {
                payload: Some(b"edited".to_vec()),
            }),
        },
    );

    // The origin echoes the edited frame, which is another text message and
    // is held by the same breakpoint. Let it through unchanged.
    let echoed_pause = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(echoed_pause.pause_id, PauseDecision::unchanged());

    let echoed = timeout(TEST_TIMEOUT, async {
        let mut buf = vec![0_u8; 32];
        let n = stream.read(&mut buf).await.expect("read the echo");
        buf.truncate(n);
        buf
    })
    .await
    .expect("the origin still answers");
    assert_eq!(frame_payload(&echoed), b"edited");

    let up = next_stream_message(&mut streams).await;
    assert_eq!(up.direction, StreamDirection::ClientToServer);
    assert_eq!(up.payload_size, 5);

    proxy.stop().await;
}

#[tokio::test]
async fn cancelling_a_text_message_drops_it_and_leaves_the_socket_open() {
    let origin = WsOrigin::start(AGREED, |stream| async move {
        let (mut from_client, mut to_client) = tokio::io::split(stream);
        let mut buf = vec![0_u8; 32];
        let n = tokio::io::AsyncReadExt::read(&mut from_client, &mut buf)
            .await
            .unwrap_or(0);
        if n > 0 {
            let _ = to_client.write_all(&buf[..n]).await;
            let _ = to_client.flush().await;
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    proxy.controls().breakpoints().create(NewBreakpoint::new(
        BreakpointDirection::WebSocket,
        RuleMatcher::any().with_host(origin.addr().ip().to_string()),
    ));
    let url = format!("{}/socket", origin.base_url());

    let mut stream = open(proxy.addr(), &url, &origin.addr().to_string()).await;
    stream
        .write_all(&text_frame(b"drop-me"))
        .await
        .expect("send");
    stream.flush().await.expect("flush");

    let paused = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::Cancel);

    proxy.controls().breakpoints().delete(paused.breakpoint_id);

    stream
        .write_all(&text_frame(b"kept"))
        .await
        .expect("send after cancel");
    stream.flush().await.expect("flush");

    let echoed = timeout(TEST_TIMEOUT, async {
        let mut buf = vec![0_u8; 16];
        let n = stream.read(&mut buf).await.expect("read");
        buf.truncate(n);
        buf
    })
    .await
    .expect("the second message is still carried");
    assert!(
        echoed.windows(4).any(|window| window == b"kept"),
        "got {echoed:?}"
    );
    assert!(
        !echoed.windows(7).any(|window| window == b"drop-me"),
        "the cancelled message was forwarded: {echoed:?}"
    );

    proxy.stop().await;
}
