//! A whole session, end to end: capture it, write it out, read it back, and
//! keep a request out of it.
//!
//! ```text
//! browser ──▶ proxy ──▶ origin          real traffic, plaintext and intercepted
//!               │
//!               ▼
//!            storage ──▶ export_har ──▶ a .har file on disk
//!                            │
//!                            ▼
//!                        import_har ──▶ storage ──▶ get_traffic
//!
//!            storage ──▶ save_request_from_traffic ──▶ replay_request ──▶ origin
//! ```
//!
//! Everything in this file is real: a real listener on an OS-assigned port, a
//! real TLS interception with a CA generated for the test, a real SQLite
//! database in a temporary directory, a real Unix socket, a real file written
//! and read back, and a real origin server answering a replayed request.
//! Nothing reaches the network and no test runs a process.
//!
//! What is under test is the join. That HAR 1.2 is spelled correctly is
//! `proxy-har`'s to prove and it does, with no socket and no database; that
//! the operations are shaped correctly on the wire is `proxy-ipc`'s. What is
//! only provable here is that traffic the proxy actually carried survives the
//! round trip through a file, and that a request saved out of it is one the
//! proxy will send again.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use support::{request, HttpsClient, Origin, Proxy, TlsOrigin, TEST_TIMEOUT};

use proxy_engine::capture::Capture;
use proxy_engine::replay::Replayer;
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

/// An answer with a JSON body, so an exported entry has one worth reading.
const CREATED: &[u8] = b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
Content-Length: 8\r\n\r\n{\"id\":1}";

/// The proxy, its storage, the replayer, and the IPC server over all three:
/// what the binary starts, assembled the same way.
struct Running {
    proxy: Proxy,
    handle: StorageHandle,
    worker: StorageWorker,
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    ipc: Option<JoinHandle<std::io::Result<()>>>,
    directory: TempDir,
}

impl Running {
    /// Starts everything, trusting `trust` upstream so an intercepted request
    /// can reach a TLS origin the test generated.
    async fn start(trust: proxy_tls::UpstreamTrust) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage =
            Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
        let worker = StorageWorker::spawn(storage.clone());
        let handle = worker.handle();
        let capture = Capture::new(CaptureConfig::default(), EventPublisher::new())
            .storing_into(Arc::new(handle.clone()));
        let proxy = Proxy::capturing(trust, capture.clone()).await;

        // The same capture, the same upstream pool and the same rules the
        // listener is using: a replayed request goes through the pipeline the
        // proxy is already running.
        let replayer = Arc::new(Replayer::new(
            capture.clone(),
            Arc::clone(proxy.upstream()),
            Some(proxy.engine()),
        ));
        let service =
            TrafficService::new(storage, capture.publisher().clone()).replaying_with(replayer);

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
            handle,
            worker,
            socket,
            shutdown: Some(shutdown),
            ipc: Some(ipc),
            directory,
        }
    }

    /// A client of the IPC socket, speaking the protocol by hand.
    async fn connect(&self) -> IpcClient {
        IpcClient::connect(&self.socket).await
    }

    /// A path in this run's own temporary directory.
    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
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
    next: u64,
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
            next: 0,
        }
    }

    /// Sends one operation with `parameters` and reads the answer.
    async fn call(&mut self, operation: &str, parameters: Value) -> Value {
        self.next += 1;
        let mut message = json!({
            "version": 1,
            "request_id": self.next.to_string(),
            "type": operation,
        });
        for (name, value) in parameters.as_object().expect("an object") {
            message
                .as_object_mut()
                .expect("an object")
                .insert(name.clone(), value.clone());
        }

        let mut line = serde_json::to_vec(&message).expect("serializes");
        line.push(b'\n');
        timeout(TEST_TIMEOUT, async {
            self.writer.write_all(&line).await?;
            self.writer.flush().await
        })
        .await
        .expect("the write finishes")
        .expect("the server is reading");

        let mut answer = String::new();
        let read = timeout(TEST_TIMEOUT, self.reader.read_line(&mut answer))
            .await
            .expect("the server answers")
            .expect("the connection is readable");
        assert!(read > 0, "the server closed the connection");

        let answer: Value = serde_json::from_str(&answer).expect("a JSON message");
        assert_eq!(answer["ok"], true, "{operation} was refused: {answer}");

        answer
    }
}

/// A request in the origin form an intercepted session carries, with a body.
///
/// `support::origin_form_request` writes headers and no body, and what an
/// exported entry is worth checking against is a request that had one.
fn origin_form_post(path: &str, host: &str, body: &[u8]) -> Vec<u8> {
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);

    request
}

/// Every entry of the HAR document at `path`.
fn entries(path: &Path) -> Vec<Value> {
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the file is there"))
            .expect("it is JSON");

    assert_eq!(document["log"]["version"], "1.2", "{document}");
    assert_eq!(document["log"]["creator"]["name"], "ProxyLab");

    document["log"]["entries"]
        .as_array()
        .expect("entries")
        .clone()
}

/// Whether `entry` has every field HAR 1.2 requires of one.
///
/// Checked here rather than taken on trust: the whole point of the format is
/// that another tool reads the file, and another tool reading it will look
/// for exactly these.
fn is_valid_har_entry(entry: &Value) -> bool {
    let required_of_entry = [
        "startedDateTime",
        "time",
        "request",
        "response",
        "cache",
        "timings",
    ];
    let required_of_request = [
        "method",
        "url",
        "httpVersion",
        "cookies",
        "headers",
        "queryString",
        "headersSize",
        "bodySize",
    ];
    let required_of_response = [
        "status",
        "statusText",
        "httpVersion",
        "cookies",
        "headers",
        "content",
        "redirectURL",
        "headersSize",
        "bodySize",
    ];

    required_of_entry
        .iter()
        .all(|name| entry.get(name).is_some())
        && required_of_request
            .iter()
            .all(|name| entry["request"].get(name).is_some())
        && required_of_response
            .iter()
            .all(|name| entry["response"].get(name).is_some())
        && entry["response"]["content"].get("size").is_some()
        && entry["response"]["content"].get("mimeType").is_some()
        && ["send", "wait", "receive"]
            .iter()
            .all(|name| entry["timings"].get(name).is_some())
}

#[tokio::test]
async fn a_captured_session_is_exported_read_back_and_inspected() {
    let origin = TlsOrigin::responding_with("localhost", CREATED).await;
    let plaintext = Origin::responding_with(HELLO).await;
    let running = Running::start(origin.trust()).await;

    // Two exchanges the proxy actually carried: one plaintext and forwarded,
    // one read out of an intercepted TLS session.
    let answered = request(
        running.proxy.addr(),
        "GET",
        &format!("{}/hello", plaintext.base_url()),
        &[],
    )
    .await;
    assert_eq!(answered.status, 200);

    let client = HttpsClient::trusting(running.proxy.trust());
    let intercepted = client
        .exchange(
            running.proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_post("/users", &origin.authority(), br#"{"name":"ada"}"#),
        )
        .await;
    assert_eq!(intercepted.status, 201);

    running.flush().await;
    let mut ipc = running.connect().await;

    // 1. Export everything.
    let path = running.path("session.har");
    let exported = ipc
        .call("export_har", json!({"path": path.to_string_lossy()}))
        .await;
    assert_eq!(exported["result"]["entries"], 2, "{exported}");
    assert_eq!(exported["result"]["truncated"], false);

    // 2. The file is a HAR 1.2 document another tool would read.
    let written = entries(&path);
    assert_eq!(written.len(), 2);
    for entry in &written {
        assert!(is_valid_har_entry(entry), "{entry}");
    }
    // In the order the traffic happened, whatever end of the list it is read
    // from.
    assert!(written[0]["request"]["url"]
        .as_str()
        .expect("a url")
        .starts_with("http://"));
    assert_eq!(written[1]["request"]["method"], "POST");
    assert!(written[1]["request"]["url"]
        .as_str()
        .expect("a url")
        .starts_with("https://"));
    assert_eq!(written[1]["response"]["status"], 201);
    assert_eq!(written[1]["response"]["content"]["text"], "{\"id\":1}");
    assert_eq!(
        written[1]["request"]["postData"]["text"],
        "{\"name\":\"ada\"}"
    );
    // The timings add up to the duration, which is what HAR says they must.
    for entry in &written {
        let timings = &entry["timings"];
        let measured: f64 = ["connect", "send", "wait", "receive"]
            .iter()
            .map(|name| timings[name].as_f64().expect("a duration"))
            .filter(|duration| *duration >= 0.0)
            .sum();
        let total = entry["time"].as_f64().expect("a duration");
        assert!(
            (measured - total).abs() < 0.001,
            "{measured} does not add up to {total}"
        );
    }

    // 3. Clear the history and read the file back into it.
    ipc.call("clear_traffic", json!({})).await;
    let imported = ipc
        .call("import_har", json!({"path": path.to_string_lossy()}))
        .await;
    assert_eq!(imported["result"]["imported"], 2, "{imported}");
    assert_eq!(imported["result"]["skipped"], 0);

    // 4. What came back is inspected through the traffic operations the
    //    inspector already uses, and nothing else.
    let listed = ipc.call("list_traffic", json!({"order": "oldest"})).await;
    assert_eq!(listed["result"]["total"], 2);
    let second = listed["result"]["exchanges"][1]["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let detail = ipc
        .call("get_traffic", json!({"exchange_id": second}))
        .await;
    let exchange = &detail["result"]["exchange"];
    assert_eq!(exchange["request"]["method"], "POST");
    assert_eq!(exchange["response"]["status"], 201);
    assert_eq!(exchange["connection"]["scheme"], "https");
    // The body is the bytes, base64 as every captured body is.
    assert_eq!(exchange["request"]["body"]["data"], "eyJuYW1lIjoiYWRhIn0=");
    assert_eq!(exchange["response"]["body"]["data"], "eyJpZCI6MX0=");

    running.stop().await;
}

#[tokio::test]
async fn a_captured_request_is_saved_renamed_replayed_and_deleted() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start(proxy_tls::UpstreamTrust::PublicRoots).await;

    let answered = request(
        running.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[("X-Api-Key", "sk-secret")],
    )
    .await;
    assert_eq!(answered.status, 200);

    running.flush().await;
    let mut ipc = running.connect().await;

    let listed = ipc.call("list_traffic", json!({})).await;
    let captured = listed["result"]["exchanges"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    // 1. Save it. The request is the core's, read out of its own store.
    let saved = ipc
        .call(
            "save_request_from_traffic",
            json!({"exchange_id": captured, "name": "The hello call"}),
        )
        .await;
    let id = saved["result"]["saved_request"]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert_eq!(saved["result"]["saved_request"]["name"], "The hello call");
    assert_eq!(saved["result"]["saved_request"]["method"], "GET");
    // The credential came with it: a saved request that dropped it would
    // reproduce nothing.
    let headers = saved["result"]["saved_request"]["headers"]
        .as_array()
        .expect("headers");
    assert!(
        headers.iter().any(|header| {
            header["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("x-api-key"))
                && header["value"]["text"] == "sk-secret"
        }),
        "{headers:?}"
    );

    // 2. Rename it.
    let renamed = ipc
        .call(
            "update_saved_request",
            json!({"saved_request_id": id, "name": "Health check"}),
        )
        .await;
    assert_eq!(renamed["result"]["saved_request"]["name"], "Health check");
    assert_eq!(
        renamed["result"]["saved_request"]["url"],
        saved["result"]["saved_request"]["url"]
    );

    // 3. Replay it, through the one replay path there is.
    let request = &renamed["result"]["saved_request"];
    let replayed = ipc
        .call(
            "replay_request",
            json!({
                "method": request["method"],
                "url": request["url"],
                "headers": request["headers"],
                "body": request["body"]["data"],
            }),
        )
        .await;
    assert_eq!(replayed["result"]["status"], 200, "{replayed}");

    // It reached the origin, and it carried the credential.
    let received = origin.requests();
    assert_eq!(received.len(), 2);
    assert!(
        received[1].header("x-api-key") == Some("sk-secret"),
        "the replay dropped the credential"
    );

    // And it is traffic: a new exchange, in the list beside the original.
    running.flush().await;
    let listed = ipc.call("list_traffic", json!({})).await;
    assert_eq!(listed["result"]["total"], 2);

    // 4. Edit it, and send the edit.
    let edited = ipc
        .call(
            "update_saved_request",
            json!({
                "saved_request_id": id,
                "method": "POST",
                "url": format!("{}/edited", origin.base_url()),
                "body": "ZWRpdGVk",
            }),
        )
        .await;
    assert_eq!(edited["result"]["saved_request"]["method"], "POST");
    assert_eq!(edited["result"]["saved_request"]["body"]["size"], 6);
    assert_eq!(edited["result"]["saved_request"]["name"], "Health check");

    let request = &edited["result"]["saved_request"];
    let replayed = ipc
        .call(
            "replay_request",
            json!({
                "method": request["method"],
                "url": request["url"],
                "headers": request["headers"],
                "body": request["body"]["data"],
            }),
        )
        .await;
    assert_eq!(replayed["result"]["status"], 200, "{replayed}");
    let received = origin.requests();
    assert_eq!(received.len(), 3);
    assert_eq!(received[2].method, "POST");
    assert_eq!(received[2].body, b"edited");
    // The `Host` the edited URL implies, not the one the capture carried: a
    // saved request whose URL was changed must not go on naming the old one.
    assert_eq!(
        received[2].header("host"),
        Some(origin.addr().to_string().as_str())
    );

    // 5. Delete it. The traffic it produced stays where it is — a saved
    //    request is not the exchanges it was sent as.
    let deleted = ipc
        .call("delete_saved_request", json!({"saved_request_id": id}))
        .await;
    assert_eq!(deleted["result"]["deleted"], true);

    let remaining = ipc.call("list_saved_requests", json!({})).await;
    assert_eq!(remaining["result"]["saved_requests"], json!([]));

    running.flush().await;
    let listed = ipc.call("list_traffic", json!({})).await;
    assert_eq!(listed["result"]["total"], 3);

    running.stop().await;
}

#[tokio::test]
async fn saved_requests_outlive_the_traffic_being_cleared() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start(proxy_tls::UpstreamTrust::PublicRoots).await;

    request(
        running.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    running.flush().await;

    let mut ipc = running.connect().await;
    let listed = ipc.call("list_traffic", json!({})).await;
    let captured = listed["result"]["exchanges"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    ipc.call(
        "save_request_from_traffic",
        json!({"exchange_id": captured, "name": "Kept"}),
    )
    .await;

    ipc.call("clear_traffic", json!({})).await;

    let remaining = ipc.call("list_saved_requests", json!({})).await;
    assert_eq!(
        remaining["result"]["saved_requests"]
            .as_array()
            .expect("a list")
            .len(),
        1
    );
    assert_eq!(
        ipc.call("list_traffic", json!({})).await["result"]["total"],
        0
    );

    running.stop().await;
}

#[tokio::test]
async fn an_import_sends_nothing_to_the_origin_it_names() {
    // The whole of what "read-only" means: reading a file somebody sent you
    // must not make the requests in it. The origin here is the one the
    // entries name, and it must not be contacted at all.
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start(proxy_tls::UpstreamTrust::PublicRoots).await;

    request(
        running.proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;
    running.flush().await;

    let mut ipc = running.connect().await;
    let path = running.path("session.har");
    ipc.call("export_har", json!({"path": path.to_string_lossy()}))
        .await;
    ipc.call("clear_traffic", json!({})).await;

    let before = origin.requests().len();
    ipc.call("import_har", json!({"path": path.to_string_lossy()}))
        .await;

    assert_eq!(origin.requests().len(), before, "the import sent a request");

    running.stop().await;
}
