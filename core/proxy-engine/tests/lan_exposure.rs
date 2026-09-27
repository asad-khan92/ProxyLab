//! The proxy on the local network: off by default, on when asked, off again.
//!
//! What is under test is the join between the IPC boundary and the second
//! listener — that `enable_lan` produces a listening socket on the address a
//! client picked, that traffic arriving on it goes through the pipeline the
//! loopback listener uses rather than one of its own, and that `disable_lan`
//! leaves nothing behind.
//!
//! Everything runs in this process, on OS-assigned ports and in a temporary
//! directory. The one thing that is not made up is the address: a LAN
//! listener has to bind an address this machine really has, so these tests
//! ask the machine for one. A machine with no usable LAN address — a build
//! agent with only loopback and a tunnel — has nothing to bind, and those
//! tests return rather than fail: what they would be reporting is the
//! network, not the code.

mod support;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use support::{request, Origin, Proxy, TEST_TIMEOUT};

use proxy_engine::lan::{discovery, LanExposure};
use proxy_ipc::{IpcConfig, IpcServer, LanControl, TrafficService};
use proxy_storage::{CaptureConfig, EventPublisher, Storage, StorageConfig, StorageWorker};
use rules::{NewRule, Rule, RuleAction, RuleMatcher, RuleSet};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

/// One address of this machine a LAN listener could use, if there is one.
fn an_address() -> Option<Ipv4Addr> {
    discovery::usable()
        .first()
        .and_then(|address| discovery::parse(&address.address))
}

/// A port the operating system has just handed out and released again.
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");

    listener.local_addr().expect("a bound address").port()
}

/// Whether anything is listening at `addr`.
async fn is_reachable(addr: SocketAddr) -> bool {
    timeout(TEST_TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("connecting resolves")
        .is_ok()
}

/// The proxy, the storage, the IPC server, and the LAN switch: what the
/// binary starts, assembled the same way.
struct Running {
    proxy: Proxy,
    lan: Arc<LanExposure>,
    lan_port: u16,
    socket: std::path::PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    ipc: Option<JoinHandle<std::io::Result<()>>>,
    worker: StorageWorker,
    stopping: watch::Sender<bool>,
    directory: TempDir,
}

impl Running {
    async fn start() -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage =
            Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
        let worker = StorageWorker::spawn(storage.clone());
        let capture =
            proxy_engine::capture::Capture::new(CaptureConfig::default(), EventPublisher::new())
                .storing_into(Arc::new(worker.handle()));
        let proxy = Proxy::with_capture(capture.clone()).await;

        // The same interceptor, the same upstream connections, the same
        // capture and the same rule engine the loopback listener is using.
        // That sharing is the feature under test.
        let (stopping, signal) = watch::channel(false);
        let lan_port = free_port().await;
        let lan = Arc::new(LanExposure::new(
            lan_port,
            Arc::clone(proxy.interceptor()),
            Arc::clone(proxy.upstream()),
            capture.clone(),
            Some(Arc::clone(proxy.rule_engine())),
            proxy.controls().clone(),
            signal,
        ));

        let service = TrafficService::new(storage, capture.publisher().clone())
            .exposing_lan(Arc::clone(&lan) as Arc<dyn LanControl>);
        let socket = directory.path().join("proxylab.sock");
        let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("the socket binds");
        let (shutdown, ipc_signal) = oneshot::channel();
        let ipc = tokio::spawn(async move {
            server
                .run(service, async {
                    let _ = ipc_signal.await;
                })
                .await
        });

        Self {
            proxy,
            lan,
            lan_port,
            socket,
            shutdown: Some(shutdown),
            ipc: Some(ipc),
            worker,
            stopping,
            directory,
        }
    }

    async fn connect(&self) -> IpcClient {
        IpcClient::connect(&self.socket).await
    }

    /// Where the LAN listener is, once it is enabled.
    fn lan_addr(&self, address: Ipv4Addr) -> SocketAddr {
        SocketAddr::from((address, self.lan_port))
    }

    async fn stop(mut self) {
        let _ = self.stopping.send(true);
        self.lan.shutdown().await;
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

    async fn request(&mut self, message: &Value) -> Value {
        let mut line = serde_json::to_vec(message).expect("serializes");
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
        serde_json::from_str(&answer).expect("a JSON message")
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
async fn a_proxy_that_has_just_started_is_on_loopback_and_nothing_else() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let status = client
        .request(&ipc_request("r1", "lan_status", json!({})))
        .await;

    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["enabled"], false);
    assert_eq!(status["result"]["address"], Value::Null);

    // And the loopback listener is where it always was.
    assert!(running.proxy.addr().ip().is_loopback());

    running.stop().await;
}

#[tokio::test]
async fn the_status_names_the_port_even_with_lan_mode_off() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let status = client
        .request(&ipc_request("r1", "lan_status", json!({})))
        .await;

    assert_eq!(status["result"]["port"], running.lan_port);

    running.stop().await;
}

#[tokio::test]
async fn the_addresses_offered_are_ones_a_phone_could_reach() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let status = client
        .request(&ipc_request("r1", "lan_status", json!({})))
        .await;

    for address in status["result"]["available"]
        .as_array()
        .expect("a list of addresses")
    {
        let parsed = discovery::parse(address["address"].as_str().expect("an address"))
            .expect("an address the core offered");

        assert!(!parsed.is_loopback(), "{parsed}");
        assert!(parsed.is_private(), "{parsed}");
        assert!(!address["interface"].as_str().expect("a name").is_empty());
    }

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enabling_lan_mode_puts_the_proxy_on_the_address_that_was_chosen() {
    let Some(address) = an_address() else { return };
    let running = Running::start().await;
    let mut client = running.connect().await;

    let enabled = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": address.to_string()}),
        ))
        .await;

    assert_eq!(enabled["ok"], true, "{enabled}");
    assert_eq!(enabled["type"], "enable_lan");
    assert_eq!(enabled["result"]["enabled"], true);
    assert_eq!(enabled["result"]["address"], address.to_string());
    assert!(is_reachable(running.lan_addr(address)).await);

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn traffic_arriving_on_the_lan_listener_is_carried_and_captured_like_any_other() {
    let Some(address) = an_address() else { return };
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": address.to_string()}),
        ))
        .await;

    // Sent to the LAN listener, not to loopback.
    let response = request(
        running.lan_addr(address),
        "GET",
        &format!("{}/from-the-phone", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");
    assert_eq!(origin.only_request().target, "/from-the-phone");

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_applies_to_lan_traffic_exactly_as_it_does_to_loopback_traffic() {
    let Some(address) = an_address() else { return };
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": address.to_string()}),
        ))
        .await;

    // The engine the loopback listener consults is the one the LAN listener
    // consults, so changing it changes both.
    running.proxy.set_rules(blocking_everything());

    let over_lan = request(
        running.lan_addr(address),
        "GET",
        &format!("{}/blocked", origin.base_url()),
        &[],
    )
    .await;
    let over_loopback = request(
        running.proxy.addr(),
        "GET",
        &format!("{}/blocked", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(over_lan.status, 403);
    assert_eq!(over_loopback.status, 403);
    assert!(origin.requests().is_empty(), "the origin was reached");

    running.stop().await;
}

/// A rule set that blocks every request, whichever listener carried it.
fn blocking_everything() -> RuleSet {
    let rule = Rule::create(NewRule::new(
        "block everything",
        RuleMatcher::any(),
        RuleAction::block_with(403),
    ))
    .expect("a valid rule");

    RuleSet::new([rule])
}

#[tokio::test(flavor = "multi_thread")]
async fn disabling_lan_mode_closes_the_listener_and_leaves_loopback_working() {
    let Some(address) = an_address() else { return };
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": address.to_string()}),
        ))
        .await;
    assert!(is_reachable(running.lan_addr(address)).await);

    let disabled = client
        .request(&ipc_request("r2", "disable_lan", json!({})))
        .await;

    assert_eq!(disabled["ok"], true, "{disabled}");
    assert_eq!(disabled["result"]["enabled"], false);
    assert_eq!(disabled["result"]["address"], Value::Null);
    assert!(
        !is_reachable(running.lan_addr(address)).await,
        "the LAN listener was still reachable after being disabled"
    );

    // The proxy is still a proxy.
    let response = request(
        running.proxy.addr(),
        "GET",
        &format!("{}/still-here", origin.base_url()),
        &[],
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lan_mode_can_be_turned_on_again_after_being_turned_off() {
    let Some(address) = an_address() else { return };
    let running = Running::start().await;
    let mut client = running.connect().await;

    for round in 0..2 {
        client
            .request(&ipc_request(
                &format!("on{round}"),
                "enable_lan",
                json!({"address": address.to_string()}),
            ))
            .await;
        assert!(
            is_reachable(running.lan_addr(address)).await,
            "round {round}"
        );

        client
            .request(&ipc_request(
                &format!("off{round}"),
                "disable_lan",
                json!({}),
            ))
            .await;
        assert!(
            !is_reachable(running.lan_addr(address)).await,
            "round {round}"
        );
    }

    running.stop().await;
}

#[tokio::test]
async fn an_address_this_machine_does_not_have_is_refused_and_changes_nothing() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": "10.255.255.254"}),
        ))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "lan_unavailable");

    let status = client
        .request(&ipc_request("r2", "lan_status", json!({})))
        .await;
    assert_eq!(status["result"]["enabled"], false);

    running.stop().await;
}

#[tokio::test]
async fn loopback_is_refused_as_a_lan_address() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": "127.0.0.1"}),
        ))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "lan_unavailable");

    running.stop().await;
}

#[tokio::test]
async fn the_unspecified_address_is_refused() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": "0.0.0.0"}),
        ))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "lan_unavailable");

    running.stop().await;
}

#[tokio::test]
async fn something_that_is_not_an_address_is_a_parameter_problem() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": "the wi-fi one"}),
        ))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_parameters");

    running.stop().await;
}

#[tokio::test]
async fn enable_lan_without_an_address_is_refused() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request("r1", "enable_lan", json!({})))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_parameters");

    running.stop().await;
}

#[tokio::test]
async fn a_lan_operation_never_repeats_what_the_client_sent() {
    let running = Running::start().await;
    let mut client = running.connect().await;

    let refused = client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": "203.0.113.77"}),
        ))
        .await;

    assert!(
        !refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("203.0.113.77"),
        "{refused}"
    );

    running.stop().await;
}

#[tokio::test]
async fn a_server_that_is_not_managing_lan_exposure_says_so() {
    let directory = TempDir::new().expect("a temporary directory");
    let storage =
        Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
    let service = TrafficService::new(storage, EventPublisher::new());
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

    let mut client = IpcClient::connect(&socket).await;
    let refused = client
        .request(&ipc_request("r1", "lan_status", json!({})))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "internal");

    let _ = shutdown.send(());
    timeout(TEST_TIMEOUT, ipc)
        .await
        .expect("the IPC server shuts down")
        .expect("the IPC task does not panic")
        .expect("a clean shutdown");
}

// The setup page: how the certificate gets onto a device that is already
// going through the proxy. It is answered on whichever listener the request
// arrived at, because a simulator and a browser on this Mac need it too.

#[tokio::test]
async fn the_setup_host_serves_this_proxy_s_own_certificate() {
    let running = Running::start().await;

    let response = request(
        running.proxy.addr(),
        "GET",
        "http://proxylab.test/ca.pem",
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), running.proxy.ca_certificate_pem());

    running.stop().await;
}

#[tokio::test]
async fn the_certificate_is_served_as_something_a_phone_will_install() {
    let running = Running::start().await;

    let response = request(
        running.proxy.addr(),
        "GET",
        "http://proxylab.test/ca.cer",
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("application/x-x509-ca-cert")
    );
    assert!(!response.body().is_empty());

    running.stop().await;
}

#[tokio::test]
async fn the_setup_host_has_a_page_saying_what_to_do() {
    let running = Running::start().await;

    let response = request(running.proxy.addr(), "GET", "http://proxylab.test/", &[]).await;

    assert_eq!(response.status, 200);
    let page = response.body_text();
    assert!(page.contains("Certificate Trust Settings"), "{page}");
    assert!(page.contains("/ca.cer"), "{page}");

    running.stop().await;
}

#[tokio::test]
async fn nothing_the_setup_host_serves_is_a_private_key() {
    let running = Running::start().await;

    for path in ["/", "/ca.cer", "/ca.pem", "/ca-key.pem"] {
        let response = request(
            running.proxy.addr(),
            "GET",
            &format!("http://proxylab.test{path}"),
            &[],
        )
        .await;

        // As bytes, because one of these is DER and is not text at all.
        let body = response.body();
        assert!(
            !body.windows(11).any(|window| window == b"PRIVATE KEY"),
            "{path} served a private key"
        );
    }

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_on_the_lan_listener_can_fetch_the_certificate_too() {
    let Some(address) = an_address() else { return };
    let running = Running::start().await;
    let mut client = running.connect().await;
    client
        .request(&ipc_request(
            "r1",
            "enable_lan",
            json!({"address": address.to_string()}),
        ))
        .await;

    let response = request(
        running.lan_addr(address),
        "GET",
        "http://proxylab.test/ca.pem",
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), running.proxy.ca_certificate_pem());

    running.stop().await;
}

#[tokio::test]
async fn every_other_host_is_still_forwarded() {
    let origin = Origin::responding_with(HELLO).await;
    let running = Running::start().await;

    // A host that merely contains the setup host's name is not it.
    let response = request(
        running.proxy.addr(),
        "GET",
        &format!("{}/proxylab.test", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");
    assert_eq!(origin.only_request().target, "/proxylab.test");

    running.stop().await;
}
