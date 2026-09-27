//! End-to-end tests for persisting the traffic the proxy captures.
//!
//! Every test runs the real proxy and an in-process origin server on
//! OS-assigned ports, sends a real request through the proxy, and then reads
//! the exchange back out of a real SQLite database in a temporary directory.
//! Nothing here reaches the network, DNS, or any external service, and
//! nothing writes outside its own temporary directory.
//!
//! Waiting is deterministic throughout and never a sleep. An exchange is
//! handed to storage before it is published, so a test that has received the
//! event knows the exchange is already in the storage queue; flushing the
//! worker then waits for exactly that work to finish.

mod support;

use std::sync::Arc;

use support::{
    next_exchange, origin_form_request, request, request_with_body, websocket, HttpsClient, Origin,
    Proxy, TlsOrigin, TEST_TIMEOUT,
};

use proxy_engine::capture::Capture;
use proxy_storage::{
    CaptureConfig, EventPublisher, EventSubscriber, ExchangeId, Scheme, Storage, StorageConfig,
    StorageHandle, StorageWorker, TrafficErrorKind, TrafficExchange, TrafficQuery,
};
use proxy_tls::UpstreamTrust;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

/// A proxy that stores what it captures, and the store it writes to.
struct StoringProxy {
    proxy: Proxy,
    storage: Storage,
    worker: StorageWorker,
    handle: StorageHandle,
    events: EventSubscriber,
    directory: TempDir,
}

impl StoringProxy {
    /// A proxy storing into a database in a temporary directory of its own.
    async fn start() -> Self {
        Self::storing_into(|directory| {
            StorageConfig::in_directory(directory).with_inline_body_max_bytes(64)
        })
        .await
    }

    /// A proxy storing into the database `configure` describes.
    async fn storing_into(configure: impl FnOnce(&std::path::Path) -> StorageConfig) -> Self {
        Self::trusting(UpstreamTrust::PublicRoots, configure).await
    }

    /// A proxy that verifies origin servers against `trust` and stores into
    /// the database `configure` describes.
    async fn trusting(
        trust: UpstreamTrust,
        configure: impl FnOnce(&std::path::Path) -> StorageConfig,
    ) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = Storage::open(configure(directory.path())).expect("a store opens");
        let worker = StorageWorker::spawn(storage.clone());
        let handle = worker.handle();
        let capture = Capture::new(CaptureConfig::default(), EventPublisher::new())
            .storing_into(Arc::new(handle.clone()))
            .storing_streams(Arc::new(handle.clone()));
        let events = capture.subscribe();
        let proxy = Proxy::capturing(trust, capture).await;

        Self {
            proxy,
            storage,
            worker,
            handle,
            events,
            directory,
        }
    }

    /// Waits for the next exchange to be published and then stored.
    ///
    /// The event is what says the exchange is finished and has been handed
    /// to storage; the flush is what says storage has dealt with it.
    async fn next_stored(&mut self) -> ExchangeId {
        let id = next_exchange(&mut self.events).await.id;
        self.flush().await;

        id
    }

    /// Waits for everything handed to storage so far to be dealt with.
    async fn flush(&self) {
        assert!(
            tokio::time::timeout(TEST_TIMEOUT, self.handle.flush())
                .await
                .expect("the storage worker answers a flush"),
            "the storage worker was gone"
        );
    }

    /// The stored form of `id`.
    fn stored(&self, id: ExchangeId) -> TrafficExchange {
        self.storage
            .get(id)
            .expect("the database is readable")
            .expect("the exchange was stored")
    }

    /// Shuts the proxy and the storage worker down.
    async fn stop(self) {
        self.proxy.stop().await;
        self.worker.shutdown().await;
        drop(self.directory);
    }
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
async fn a_forwarded_request_is_stored_as_it_was_captured() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::start().await;

    let response = request(
        proxy.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[("accept", "text/plain")],
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.request.method, "GET");
    assert_eq!(stored.request.url, format!("{}/hello", origin.base_url()));
    assert_eq!(stored.connection.scheme, Scheme::Http);
    assert_eq!(stored.status(), Some(200));
    assert_eq!(
        stored.response.as_ref().expect("a response").body.as_text(),
        Some("hello")
    );
    assert_eq!(stored.request.headers.text("accept"), Some("text/plain"));
    assert!(stored.timing.completed_micros.is_some());
    assert_eq!(proxy.storage.count().expect("counts"), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_stored_exchange_is_the_exchange_that_was_published() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::start().await;

    request(
        proxy.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    let published = next_exchange(&mut proxy.events).await;
    proxy.flush().await;

    assert_eq!(proxy.stored(published.id), *published);

    proxy.stop().await;
}

#[tokio::test]
async fn an_intercepted_https_request_is_stored_as_https_traffic() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let mut proxy = StoringProxy::trusting(origin.trust(), |directory| {
        StorageConfig::in_directory(directory)
    })
    .await;
    let client = HttpsClient::trusting(proxy.proxy.trust());

    let response = client
        .exchange(
            proxy.proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request(
                "GET",
                "/hello",
                &origin.authority(),
                &[("authorization", "Bearer sk-secret")],
            ),
        )
        .await;
    assert_eq!(response.status, 200);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.connection.scheme, Scheme::Https);
    assert_eq!(
        stored.request.url,
        format!("https://{}/hello", origin.authority())
    );
    assert_eq!(stored.status(), Some(200));
    assert_eq!(
        stored.response.as_ref().expect("a response").body.as_text(),
        Some("hello")
    );
    // The credentials the session carried are kept: that is what a debugging
    // proxy is for, and the database is where they belong.
    assert_eq!(
        stored.request.headers.text("authorization"),
        Some("Bearer sk-secret")
    );
    // The `CONNECT` that opened the session is not traffic of its own.
    assert_eq!(proxy.storage.count().expect("counts"), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_body_is_stored_and_still_reaches_the_origin() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::start().await;

    request_with_body(
        proxy.proxy.addr(),
        "POST",
        &format!("{}/orders", origin.base_url()),
        &[("content-type", "application/json")],
        r#"{"name":"ada"}"#,
    )
    .await;

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.request.body.as_text(), Some(r#"{"name":"ada"}"#));
    assert_eq!(origin.only_request().body_text(), r#"{"name":"ada"}"#);

    proxy.stop().await;
}

#[tokio::test]
async fn a_body_too_large_to_inline_is_stored_in_a_file_and_read_back_whole() {
    let body: Vec<u8> = (0..=255_u8).cycle().take(8_192).collect();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    let origin = Origin::start(move |_| response.clone()).await;
    let mut proxy = StoringProxy::start().await;

    let received = request(
        proxy.proxy.addr(),
        "GET",
        &format!("{}/download", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(received.body(), body);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(
        stored.response.as_ref().expect("a response").body.data(),
        body
    );
    let file = proxy.storage.config().bodies.join(format!("{id}.response"));
    assert!(file.is_file(), "the captured body has no file");
    assert_eq!(std::fs::read(&file).expect("readable"), body);

    proxy.stop().await;
}

#[tokio::test]
async fn a_truncated_capture_is_stored_as_truncated() {
    let body = vec![b'x'; 4_096];
    let mut response =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(&body);
    let origin = Origin::start(move |_| response.clone()).await;
    // The proxy keeps 16 bytes of any body; the client still gets all 4096.
    let directory = TempDir::new().expect("a temporary directory");
    let storage = Storage::open(StorageConfig::in_directory(directory.path())).expect("a store");
    let worker = StorageWorker::spawn(storage.clone());
    let capture = Capture::new(CaptureConfig::new(16), EventPublisher::new())
        .storing_into(Arc::new(worker.handle()));
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;

    let received = request(
        proxy.addr(),
        "GET",
        &format!("{}/download", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(received.body().len(), 4_096, "the client was short-changed");

    let id = next_exchange(&mut events).await.id;
    assert!(worker.handle().flush().await);
    let stored = storage.get(id).expect("reads").expect("stored");

    let body = &stored.response.as_ref().expect("a response").body;
    assert_eq!(body.captured_len(), 16);
    assert_eq!(body.size(), 4_096);
    assert!(body.is_truncated());

    proxy.stop().await;
    worker.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_origin_is_stored_as_a_failed_exchange() {
    let mut proxy = StoringProxy::start().await;
    let nowhere = unused_addr().await;

    let response = request(
        proxy.proxy.addr(),
        "GET",
        &format!("http://{nowhere}/x"),
        &[],
    )
    .await;
    assert_eq!(response.status, 502);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.response, None);
    assert_eq!(
        stored.error.as_ref().map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::UpstreamConnect)
    );
    assert!(stored.timing.upstream_connect_micros.is_some());

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_the_proxy_refuses_is_stored_too() {
    let mut proxy = StoringProxy::start().await;

    let response = support::send_raw(
        proxy.proxy.addr(),
        b"GET /index.html HTTP/1.1\r\nHost: proxy.test\r\n\r\n",
    )
    .await;
    assert_eq!(response.status, 400);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.request.url, "/index.html");
    assert_eq!(stored.response, None);
    assert_eq!(
        stored.error.as_ref().map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::RequestRejected)
    );

    proxy.stop().await;
}

#[tokio::test]
async fn every_exchange_of_a_busy_proxy_is_stored_exactly_once() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::start().await;
    let addr = proxy.proxy.addr();

    let clients: Vec<_> = (0..8)
        .map(|index| {
            let url = format!("{}/item-{index}", origin.base_url());
            tokio::spawn(async move { request(addr, "GET", &url, &[]).await })
        })
        .collect();
    for client in clients {
        assert_eq!(
            client.await.expect("client task does not panic").status,
            200
        );
    }

    let mut ids = Vec::new();
    for _ in 0..8 {
        ids.push(next_exchange(&mut proxy.events).await.id);
    }
    proxy.flush().await;

    assert_eq!(proxy.storage.count().expect("counts"), 8);
    let listed = proxy
        .storage
        .list(&TrafficQuery::newest().with_limit(100))
        .expect("lists");
    assert_eq!(listed.len(), 8);
    for id in ids {
        assert!(proxy.storage.get(id).expect("reads").is_some());
    }

    proxy.stop().await;
}

#[tokio::test]
async fn stored_traffic_outlives_the_proxy_that_captured_it() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::start().await;
    request(
        proxy.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    let id = proxy.next_stored().await;
    let config = proxy.storage.config().clone();
    let directory = TempDir::new().expect("a temporary directory");

    // Everything the running proxy held goes, as it would when the process
    // ends. The database is opened again from nothing but its path.
    proxy.proxy.stop().await;
    proxy.worker.shutdown().await;
    drop(proxy.storage);

    let reopened = Storage::open(config).expect("the store reopens");

    assert_eq!(reopened.count().expect("counts"), 1);
    assert_eq!(reopened.get(id).expect("reads").map(|e| e.id), Some(id));
    drop(directory);
}

#[tokio::test]
async fn a_storage_failure_does_not_stop_the_proxy_forwarding() {
    let origin = Origin::responding_with(HELLO).await;
    let mut proxy = StoringProxy::storing_into(|directory| {
        // Every captured body goes to a file, so making the body directory
        // unusable makes every insert with a body fail.
        StorageConfig::in_directory(directory).with_inline_body_max_bytes(0)
    })
    .await;
    let bodies = proxy.storage.config().bodies.clone();
    std::fs::remove_dir_all(&bodies).expect("the body directory goes");
    std::fs::write(&bodies, b"not a directory").expect("a file in its place");

    // Several requests, all of them carrying a body the proxy cannot store.
    for index in 0..3 {
        let response = request(
            proxy.proxy.addr(),
            "GET",
            &format!("{}/item-{index}", origin.base_url()),
            &[],
        )
        .await;

        // The client is answered normally: nothing about a database reaches
        // it, and certainly not a 500.
        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), "hello");

        next_exchange(&mut proxy.events).await;
    }
    proxy.flush().await;

    // The failures were counted and nothing was stored, and the proxy is
    // still running.
    assert_eq!(proxy.handle.failed(), 3);
    assert_eq!(proxy.handle.stored(), 0);
    assert_eq!(proxy.storage.count().expect("counts"), 0);

    let after = request(
        proxy.proxy.addr(),
        "GET",
        &format!("{}/after", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(after.status, 200);

    proxy.stop().await;
}

#[tokio::test]
async fn a_proxy_with_no_storage_at_all_still_forwards_and_publishes() {
    let origin = Origin::responding_with(HELLO).await;
    let capture = Capture::with_defaults();
    let mut events = capture.subscribe();
    let proxy = Proxy::with_capture(capture).await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert!(next_exchange(&mut events).await.succeeded());

    proxy.stop().await;
}

#[tokio::test]
async fn an_origin_that_never_answers_a_read_is_still_stored() {
    // An origin that accepts the connection and then closes it without a
    // response: the exchange fails upstream rather than being carried.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    let origin = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut discarded = [0_u8; 1024];
            let _ = stream.read(&mut discarded).await;
            drop(stream);
        }
    });
    let mut proxy = StoringProxy::start().await;

    let response = request(proxy.proxy.addr(), "GET", &format!("http://{addr}/x"), &[]).await;
    assert_eq!(response.status, 502);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.response, None);
    assert!(stored.error.is_some());

    origin.abort();
    proxy.stop().await;
}

// ---------------------------------------------------------------------------
// Long-lived protocols
//
// A WebSocket and an SSE response are stored through the same pipeline as any
// other exchange, and these prove it: the same table, the same columns, and
// the same survival of the process that captured them. Neither is stored
// per-frame or per-event — one connection is one row, however long it stayed
// open.
// ---------------------------------------------------------------------------

/// Sends a WebSocket upgrade through `proxy` and reads the handshake back.
async fn upgrade_through(proxy: std::net::SocketAddr, url: &str, host: &str) -> TcpStream {
    let mut stream = TcpStream::connect(proxy)
        .await
        .expect("connect to the proxy");
    stream
        .write_all(&websocket::upgrade_request(url, host))
        .await
        .expect("send the handshake");
    stream.flush().await.expect("flush");

    let handshake = websocket::read_handshake(&mut stream).await;
    assert_eq!(handshake.status, 101);

    stream
}

#[tokio::test]
async fn a_websocket_handshake_is_stored_and_read_back_after_the_proxy_is_gone() {
    let origin = websocket::WsOrigin::echoing().await;
    let mut proxy = StoringProxy::start().await;
    let url = format!("{}/socket", origin.base_url());

    let mut stream = upgrade_through(proxy.proxy.addr(), &url, &origin.addr().to_string()).await;

    // The handshake is the exchange. It is finished and stored while the
    // socket it opened is still carrying bytes, which is the point: a
    // WebSocket that stayed open for hours would otherwise be a row that was
    // never written.
    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.request.method, "GET");
    assert_eq!(stored.request.url, url);
    assert_eq!(stored.status(), Some(101));
    let response = stored.response.as_ref().expect("the handshake answered");
    assert_eq!(response.headers.text("upgrade"), Some("websocket"));
    assert_eq!(
        response.headers.text("sec-websocket-accept"),
        Some(websocket::ACCEPT)
    );
    // Nothing that crossed after the `101` is part of it.
    assert!(response.body.is_empty());

    // Opaque bytes still cross, and are not stored as messages: one
    // handshake remains one exchange.
    assert_eq!(websocket::round_trip(&mut stream, b"after").await, b"after");
    proxy.flush().await;
    assert_eq!(proxy.storage.count().expect("counts"), 1);

    let config = proxy.storage.config().clone();
    proxy.proxy.stop().await;
    proxy.worker.shutdown().await;
    drop(proxy.storage);

    let reopened = Storage::open(config).expect("the store reopens");
    let read_back = reopened
        .get(id)
        .expect("the database is readable")
        .expect("the handshake outlived the proxy");

    assert_eq!(read_back.status(), Some(101));
    assert_eq!(
        read_back
            .response
            .as_ref()
            .expect("a response")
            .headers
            .text("upgrade"),
        Some("websocket")
    );
    drop(proxy.directory);
}

#[tokio::test]
async fn websocket_messages_are_stored_on_the_handshake_and_deleted_with_it() {
    let origin = websocket::WsOrigin::echoing().await;
    let mut proxy = StoringProxy::start().await;
    let url = format!("{}/socket", origin.base_url());
    let mut stream = upgrade_through(proxy.proxy.addr(), &url, &origin.addr().to_string()).await;
    let id = proxy.next_stored().await;

    let frame = {
        let payload = b"hello";
        let mut frame = vec![0x81, payload.len() as u8];
        frame.extend_from_slice(payload);
        frame
    };
    assert_eq!(
        websocket::round_trip(&mut stream, &frame).await,
        frame.as_slice()
    );
    proxy.flush().await;

    let page = proxy
        .storage
        .list_stream_messages(id, None, None)
        .expect("lists");
    assert!(
        page.messages.len() >= 2,
        "client and echo should both be stored: {}",
        page.messages.len()
    );
    assert_eq!(
        page.messages[0].direction,
        proxy_storage::StreamDirection::ClientToServer
    );
    assert_eq!(
        page.messages[0].kind,
        proxy_storage::StreamKind::WebsocketText
    );
    assert_eq!(page.messages[0].payload_size, 5);

    let stored = proxy
        .storage
        .get_stream_message(page.messages[0].id)
        .expect("reads")
        .expect("the payload was kept");
    assert_eq!(stored.payload.data(), b"hello");

    proxy.storage.delete(id).expect("deletes the handshake");
    let gone = proxy
        .storage
        .list_stream_messages(id, None, None)
        .expect("lists");
    assert!(gone.messages.is_empty());
    assert!(proxy
        .storage
        .get_stream_message(page.messages[0].id)
        .expect("reads")
        .is_none());

    proxy.stop().await;
}

#[tokio::test]
async fn an_event_stream_is_stored_once_the_stream_has_ended() {
    // An origin that streams two events and then ends, framed as an event
    // stream is: chunked, and open for as long as it likes.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    let origin = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut head = [0_u8; 1024];
            let _ = stream.read(&mut head).await;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Content-Type: text/event-stream\r\n\
                      Transfer-Encoding: chunked\r\n\r\n",
                )
                .await;
            let _ = stream.flush().await;
            for event in ["data: one\n\n", "data: two\n\n"] {
                let _ = stream
                    .write_all(format!("{:x}\r\n{event}\r\n", event.len()).as_bytes())
                    .await;
                let _ = stream.flush().await;
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
            let _ = stream.flush().await;
        }
    });
    let mut proxy = StoringProxy::start().await;
    let url = format!("http://{addr}/events");

    let response = request(proxy.proxy.addr(), "GET", &url, &[]).await;
    assert_eq!(response.status, 200);

    let id = proxy.next_stored().await;
    let stored = proxy.stored(id);

    assert_eq!(stored.request.url, url);
    assert_eq!(stored.status(), Some(200));
    let body = &stored.response.as_ref().expect("a response").body;
    assert_eq!(
        stored
            .response
            .as_ref()
            .expect("a response")
            .headers
            .text("content-type"),
        Some("text/event-stream")
    );
    // The events are still the response body, and they are also stored
    // individually beside the exchange.
    let text = String::from_utf8_lossy(body.data()).into_owned();
    assert!(text.contains("data: one"), "got: {text}");
    assert!(text.contains("data: two"), "got: {text}");
    assert_eq!(proxy.storage.count().expect("counts"), 1);
    let events = proxy
        .storage
        .list_stream_messages(id, None, None)
        .expect("lists");
    assert_eq!(events.messages.len(), 2);
    assert_eq!(events.messages[0].kind, proxy_storage::StreamKind::Sse);
    assert_eq!(
        events.messages[0].direction,
        proxy_storage::StreamDirection::ServerToClient
    );

    let config = proxy.storage.config().clone();
    proxy.proxy.stop().await;
    proxy.worker.shutdown().await;
    drop(proxy.storage);

    let reopened = Storage::open(config).expect("the store reopens");
    let read_back = reopened
        .get(id)
        .expect("the database is readable")
        .expect("the stream outlived the proxy");

    assert_eq!(read_back.status(), Some(200));
    assert!(
        String::from_utf8_lossy(read_back.response.as_ref().expect("a response").body.data())
            .contains("data: two")
    );

    origin.abort();
    drop(proxy.directory);
}
