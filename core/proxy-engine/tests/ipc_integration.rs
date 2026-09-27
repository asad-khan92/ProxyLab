//! The proxy and the IPC server running together, as the binary runs them.
//!
//! What is under test is the join between them: traffic that goes through the
//! proxy reaching a client over the socket, and the proxy carrying on
//! regardless of what that client does. Everything runs in this process on
//! OS-assigned ports and in a temporary directory; nothing reaches the
//! network.
//!
//! Waiting is deterministic. A finished exchange is handed to storage before
//! it is published, so a test that has the event knows the exchange is
//! already queued; flushing the storage worker then waits for exactly that
//! work, and never for a length of time.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use support::{request, Origin, Proxy, TEST_TIMEOUT};

use proxy_engine::capture::Capture;
use proxy_ipc::{IpcConfig, IpcServer, TrafficService};
use proxy_storage::{
    CaptureConfig, EventPublisher, Storage, StorageConfig, StorageHandle, StorageWorker,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

/// The proxy, its storage, and the IPC server over both: what the binary
/// starts, assembled the same way.
struct Running {
    proxy: Proxy,
    storage: Storage,
    handle: StorageHandle,
    worker: StorageWorker,
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    ipc: Option<JoinHandle<std::io::Result<()>>>,
    directory: TempDir,
}

impl Running {
    async fn start() -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage =
            Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
        let worker = StorageWorker::spawn(storage.clone());
        let handle = worker.handle();
        let capture = Capture::new(CaptureConfig::default(), EventPublisher::new())
            .storing_into(Arc::new(handle.clone()))
            .storing_streams(Arc::new(handle.clone()));
        let streams = capture.stream_publisher().clone();
        let service = TrafficService::new(storage.clone(), capture.publisher().clone())
            .publishing_streams(streams);
        let proxy = Proxy::with_capture(capture).await;

        let socket = directory.path().join("proxylab.sock");
        let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("the socket binds");
        let (shutdown, signal) = oneshot::channel();
        let ipc = tokio::spawn(async move {
            server
                .run(service, async {
                    let _ = signal.await;
                })
                .await
        });

        Self {
            proxy,
            storage,
            handle,
            worker,
            socket,
            shutdown: Some(shutdown),
            ipc: Some(ipc),
            directory,
        }
    }

    /// A client of the IPC socket.
    async fn connect(&self) -> IpcClient {
        IpcClient::connect(&self.socket).await
    }

    /// Waits for everything handed to storage so far to be dealt with.
    async fn flush(&self) {
        assert!(
            timeout(TEST_TIMEOUT, self.handle.flush())
                .await
                .expect("the storage worker answers a flush"),
            "the storage worker was gone"
        );
    }

    /// Sends one request through the proxy to `origin`.
    async fn proxy_request(&self, origin: &Origin, path: &str) -> support::RawResponse {
        request(
            self.proxy.addr(),
            "GET",
            &format!("{}{path}", origin.base_url()),
            &[],
        )
        .await
    }

    async fn stop(mut self) {
        self.proxy.stop().await;
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(ipc) = self.ipc.take() {
            timeout(TEST_TIMEOUT, ipc)
                .await
                .expect("the IPC server shuts down")
                .expect("the IPC task does not panic")
                .expect("a clean shutdown");
        }
        self.worker.shutdown().await;
        drop(self.directory);
    }
}

/// A client speaking the protocol over the socket by hand.
struct IpcClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl IpcClient {
    async fn connect(socket: &Path) -> Self {
        let stream = timeout(TEST_TIMEOUT, UnixStream::connect(socket))
            .await
            .expect("connecting finishes")
            .expect("the socket accepts a connection");
        let (reader, writer) = stream.into_split();

        Self {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_vec(message).expect("serializes");
        line.push(b'\n');

        timeout(TEST_TIMEOUT, async {
            self.writer.write_all(&line).await?;
            self.writer.flush().await
        })
        .await
        .expect("the write finishes")
        .expect("the server is reading");
    }

    async fn next_message(&mut self) -> Value {
        let mut line = String::new();
        let read = timeout(TEST_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .expect("the server answers")
            .expect("the connection is readable");

        assert!(read > 0, "the server closed the connection");
        serde_json::from_str(&line).expect("a JSON message")
    }

    async fn request(&mut self, message: &Value) -> Value {
        self.send(message).await;

        self.next_message().await
    }
}

/// A request of `operation`, labelled `id`, with `parameters` merged in.
fn ipc_request(id: &str, operation: &str, parameters: Value) -> Value {
    let mut request = json!({"version": 1, "request_id": id, "type": operation});
    let object = request.as_object_mut().expect("an object");
    for (name, value) in parameters.as_object().expect("an object") {
        object.insert(name.clone(), value.clone());
    }

    request
}

#[tokio::test]
async fn traffic_the_proxy_carries_can_be_listed_over_the_socket() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;

    let response = running.proxy_request(&origin, "/hello").await;
    assert_eq!(response.status, 200);
    running.flush().await;

    let listed = client
        .request(&ipc_request("r1", "list_traffic", json!({})))
        .await;

    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["result"]["total"], 1);
    let summary = &listed["result"]["exchanges"][0];
    assert_eq!(summary["method"], "GET");
    assert_eq!(summary["status"], 200);
    assert_eq!(summary["url"], format!("{}/hello", origin.base_url()));

    running.stop().await;
}

#[tokio::test]
async fn a_subscriber_is_told_about_traffic_as_the_proxy_carries_it() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    let subscribed = client
        .request(&ipc_request("r1", "subscribe", json!({})))
        .await;
    assert_eq!(subscribed["ok"], true, "{subscribed}");

    running.proxy_request(&origin, "/watched").await;

    let event = client.next_message().await;

    assert!(event.get("ok").is_none(), "expected an event: {event}");
    assert_eq!(event["type"], "traffic_added");
    assert_eq!(event["exchange"]["status"], 200);
    let id = event["exchange"]["id"].as_str().expect("an id").to_owned();

    // The event names the exchange, and the id it names is one the server
    // will hand over in full.
    running.flush().await;
    let detail = client
        .request(&ipc_request(
            "r2",
            "get_traffic",
            json!({"exchange_id": id}),
        ))
        .await;

    assert_eq!(detail["ok"], true, "{detail}");
    assert_eq!(detail["result"]["exchange"]["id"], id);
    assert_eq!(
        detail["result"]["exchange"]["response"]["body"]["size"], 5,
        "the captured body did not come back"
    );

    running.stop().await;
}

#[tokio::test]
async fn the_whole_round_trip_works_over_one_connection() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request("subscribe", "subscribe", json!({})))
        .await;

    // Traffic, then the event, then the exchange, then delete it, then clear
    // what is left: the sequence a UI performs.
    running.proxy_request(&origin, "/one").await;
    let event = client.next_message().await;
    let id = event["exchange"]["id"].as_str().expect("an id").to_owned();
    running.flush().await;

    let detail = client
        .request(&ipc_request(
            "get",
            "get_traffic",
            json!({"exchange_id": id}),
        ))
        .await;
    assert_eq!(detail["ok"], true, "{detail}");

    let deleted = client
        .request(&ipc_request(
            "delete",
            "delete_traffic",
            json!({"exchange_id": id}),
        ))
        .await;
    assert_eq!(deleted["result"]["deleted"], true, "{deleted}");

    running.proxy_request(&origin, "/two").await;
    client.next_message().await;
    running.flush().await;

    let cleared = client
        .request(&ipc_request("clear", "clear_traffic", json!({})))
        .await;
    assert_eq!(cleared["result"]["removed"], 1, "{cleared}");
    assert_eq!(running.storage.count().expect("counts"), 0);

    running.stop().await;
}

#[tokio::test]
async fn the_proxy_keeps_carrying_traffic_while_clients_are_connected() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;

    // Clients doing every awkward thing at once: subscribed and reading,
    // subscribed and silent, asking questions, and sending nonsense.
    let mut watching = running.connect().await;
    let mut silent = running.connect().await;
    let mut asking = running.connect().await;
    let mut noisy = running.connect().await;
    watching
        .request(&ipc_request("r1", "subscribe", json!({})))
        .await;
    silent
        .request(&ipc_request("r2", "subscribe", json!({})))
        .await;
    noisy.send(&json!("not a request")).await;

    for index in 0..8 {
        let response = running.proxy_request(&origin, &format!("/{index}")).await;

        // What the proxy does for its own clients is unchanged by any of it.
        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), "hello");

        let event = watching.next_message().await;
        assert_eq!(event["type"], "traffic_added");
    }

    running.flush().await;
    let listed = asking
        .request(&ipc_request("r3", "list_traffic", json!({"limit": 100})))
        .await;
    assert_eq!(listed["result"]["total"], 8, "{listed}");

    running.stop().await;
}

#[tokio::test]
async fn the_proxy_keeps_carrying_traffic_after_every_client_has_gone() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request("r1", "subscribe", json!({})))
        .await;
    running.proxy_request(&origin, "/before").await;
    client.next_message().await;
    drop(client);

    // Nobody is connected and nobody is subscribed: the proxy carries on,
    // and what it carries is still stored.
    let response = running.proxy_request(&origin, "/after").await;
    assert_eq!(response.status, 200);
    running.flush().await;

    // And a new client sees everything, including what happened while there
    // was no client at all.
    let mut returning = running.connect().await;
    let listed = returning
        .request(&ipc_request("r2", "list_traffic", json!({})))
        .await;
    assert_eq!(listed["result"]["total"], 2, "{listed}");

    running.stop().await;
}

#[tokio::test]
async fn a_client_that_stops_reading_does_not_slow_the_proxy_down() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut silent = running.connect().await;
    silent
        .request(&ipc_request("r1", "subscribe", json!({})))
        .await;

    // A subscriber that never reads another byte, while the proxy carries
    // request after request. Every one of them is answered, and every one of
    // them is stored: a client on the other side of a socket cannot reach
    // back into the proxy.
    for index in 0..32 {
        let response = running.proxy_request(&origin, &format!("/{index}")).await;

        assert_eq!(response.status, 200, "request {index}");
    }
    running.flush().await;

    assert_eq!(running.storage.count().expect("counts"), 32);

    running.stop().await;
}
