//! Harness for the IPC server tests.
//!
//! Every test gets a server of its own: its own socket in its own temporary
//! directory, its own database, and its own event publisher. Nothing here
//! touches a fixed path, a developer's captured traffic, or the network, and
//! the temporary directory is removed with the test.
//!
//! Waiting is always for something in particular — a line arriving, a
//! response to a request that was sent — and never for a length of time. The
//! one timeout is the backstop that turns a server which never answers into
//! a failed test instead of a hung one.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex};

use proxy_http::RequestSpec;
use proxy_ipc::{
    Breakpoint, BreakpointControl, BreakpointId, BreakpointUpdate, IpcConfig, IpcServer,
    NewBreakpoint, PauseDecision, PauseId, PauseNotice, PausedExchange, ReleaseReason,
    ReplayFuture, ReplayOutcome, RequestReplayer, ThrottleControl, ThrottleSettings,
    TrafficService,
};
use proxy_storage::{
    BodyCapture, CapturedRequest, CapturedResponse, ConnectionInfo, Destination, EventPublisher,
    ExchangeId, HeaderValue, HttpVersion, Storage, StorageConfig, StreamMessage, StreamPublisher,
    Timestamp, Timing, TrafficError, TrafficErrorKind, TrafficExchange,
};
use rules::RuleEngine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

/// Long enough to absorb a loaded machine, short enough that a server which
/// never answers fails the test instead of hanging it.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A running IPC server, with the storage and the event stream behind it.
pub struct TestServer {
    socket: PathBuf,
    pub storage: Storage,
    pub publisher: EventPublisher,
    pub streams: StreamPublisher,
    pub rules: Arc<RuleEngine>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
    directory: TempDir,
}

impl TestServer {
    /// Starts a server on a socket of its own, sending no requests.
    pub async fn start() -> Self {
        Self::started(None).await
    }

    /// Starts a server that holds exchanges still and shapes traffic, through
    /// controls the test holds.
    ///
    /// The two are boundaries the proxy engine implements — see
    /// `proxy_ipc::breakpoint` and `proxy_ipc::throttle` — so these tests
    /// exercise the protocol without a proxy, an origin, or any traffic.
    pub async fn breaking() -> (Self, RecordingBreakpoints, RecordingThrottle) {
        let breakpoints = RecordingBreakpoints::new();
        let throttle = RecordingThrottle::new();
        let directory = TempDir::new().expect("a temporary directory");
        let storage =
            Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
        let server = Self::wired(
            directory,
            storage,
            EventPublisher::new(),
            StreamPublisher::new(),
            None,
            Some(Arc::new(breakpoints.clone())),
            Some(Arc::new(throttle.clone())),
        )
        .await;

        (server, breakpoints, throttle)
    }

    /// Starts a server that sends requests through a replayer the test holds.
    ///
    /// The replayer is the boundary the proxy engine implements — see
    /// `proxy_ipc::replay` — so these tests exercise the protocol without an
    /// origin server, a socket, or a proxy.
    pub async fn replaying() -> (Self, RecordingReplayer) {
        let replayer = RecordingReplayer::new();
        let server = Self::started(Some(Arc::new(replayer.clone()))).await;

        (server, replayer)
    }

    /// Starts a server on a socket of its own.
    async fn started(replayer: Option<Arc<dyn RequestReplayer>>) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage =
            Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
        let publisher = EventPublisher::new();
        let streams = StreamPublisher::new();

        Self::over_with(directory, storage, publisher, streams, replayer).await
    }

    /// Starts a server over storage and a publisher the test already has.
    pub async fn over(directory: TempDir, storage: Storage, publisher: EventPublisher) -> Self {
        Self::over_with(directory, storage, publisher, StreamPublisher::new(), None).await
    }

    /// The same, with a replayer.
    async fn over_with(
        directory: TempDir,
        storage: Storage,
        publisher: EventPublisher,
        streams: StreamPublisher,
        replayer: Option<Arc<dyn RequestReplayer>>,
    ) -> Self {
        Self::wired(directory, storage, publisher, streams, replayer, None, None).await
    }

    /// The same again, with whichever of the boundaries the test wants served.
    async fn wired(
        directory: TempDir,
        storage: Storage,
        publisher: EventPublisher,
        streams: StreamPublisher,
        replayer: Option<Arc<dyn RequestReplayer>>,
        breakpoints: Option<Arc<dyn BreakpointControl>>,
        throttle: Option<Arc<dyn ThrottleControl>>,
    ) -> Self {
        let socket = directory.path().join("proxylab.sock");
        let server = IpcServer::bind(&IpcConfig::at(&socket)).expect("the server binds");
        // The engine the rule operations keep in step, exactly as the binary
        // wires it: a test that managed rules without one would be testing a
        // path a running proxy never takes.
        let rules = Arc::new(RuleEngine::with(
            proxy_ipc::load_rules(&storage).expect("the stored rules load"),
        ));
        let mut service = TrafficService::new(storage.clone(), publisher.clone())
            .publishing_streams(streams.clone())
            .serving_rules(Arc::clone(&rules));
        if let Some(replayer) = replayer {
            service = service.replaying_with(replayer);
        }
        if let Some(breakpoints) = breakpoints {
            service = service.breaking_on(breakpoints);
        }
        if let Some(throttle) = throttle {
            service = service.throttling_with(throttle);
        }
        let (shutdown, signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .run(service, async {
                    let _ = signal.await;
                })
                .await
        });

        Self {
            socket,
            storage,
            publisher,
            streams,
            rules,
            shutdown: Some(shutdown),
            task: Some(task),
            directory,
        }
    }

    /// The socket clients connect to.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// A client connected to this server.
    pub async fn connect(&self) -> Client {
        Client::connect(&self.socket).await
    }

    /// Stores `exchange` and gives back its id.
    pub fn store(&self, exchange: &TrafficExchange) -> ExchangeId {
        self.storage.insert(exchange).expect("stores");

        exchange.id
    }

    /// Stores `message` on an exchange that is already stored.
    pub fn store_stream(&self, message: &StreamMessage) {
        self.storage
            .insert_stream_message(message)
            .expect("stores a stream message");
    }

    /// Publishes `exchange` to whoever is subscribed.
    ///
    /// How many subscribers received it, which is what makes an event test
    /// deterministic: a test can assert that the server was one of them.
    pub fn publish(&self, exchange: TrafficExchange) -> usize {
        self.publisher.publish(exchange)
    }

    /// Signals shutdown and waits for the server to stop.
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }

        if let Some(task) = self.task.take() {
            timeout(TEST_TIMEOUT, task)
                .await
                .expect("the server shuts down")
                .expect("the server task does not panic")
                .expect("a clean shutdown");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = &self.directory;
    }
}

/// A client of the IPC server, speaking the protocol by hand.
///
/// Deliberately not built on anything in `proxy-ipc` beyond the socket: what
/// is under test is the bytes on the wire, which a client sharing the
/// server's own types would not exercise.
pub struct Client {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Client {
    /// Connects to `socket`.
    pub async fn connect(socket: &Path) -> Self {
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

    /// Sends one line exactly as given.
    pub async fn send_raw(&mut self, line: &str) {
        timeout(TEST_TIMEOUT, async {
            self.writer.write_all(line.as_bytes()).await?;
            self.writer.write_all(b"\n").await?;
            self.writer.flush().await
        })
        .await
        .expect("the write finishes")
        .expect("the server is reading");
    }

    /// Sends one JSON value as a request.
    pub async fn send(&mut self, request: &Value) {
        self.send_raw(&serde_json::to_string(request).expect("serializes"))
            .await;
    }

    /// Reads the next message, whatever it is.
    pub async fn next_message(&mut self) -> Value {
        let mut line = String::new();
        let read = timeout(TEST_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .expect("the server answers")
            .expect("the connection is readable");

        assert!(read > 0, "the server closed the connection");
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("not JSON: {line:?} ({error})"))
    }

    /// Sends a request and reads the response to it.
    pub async fn request(&mut self, request: &Value) -> Value {
        self.send(request).await;

        self.next_message().await
    }

    /// Sends a raw line and reads the response to it.
    pub async fn request_raw(&mut self, line: &str) -> Value {
        self.send_raw(line).await;

        self.next_message().await
    }

    /// Whether the connection has been closed by the server.
    pub async fn is_closed(&mut self) -> bool {
        let mut line = String::new();
        let read = timeout(TEST_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .expect("the server answers or closes");

        matches!(read, Ok(0))
    }

    /// Closes the connection.
    pub async fn disconnect(mut self) {
        let _ = self.writer.shutdown().await;
    }
}

/// A request of `operation`, labelled `id`.
pub fn request(id: &str, operation: &str) -> Value {
    json!({"version": 1, "request_id": id, "type": operation})
}

/// A request of `operation` with `parameters` merged into it.
pub fn request_with(id: &str, operation: &str, parameters: Value) -> Value {
    let mut request = request(id, operation);
    let object = request.as_object_mut().expect("a request is an object");

    for (name, value) in parameters.as_object().expect("parameters are an object") {
        object.insert(name.clone(), value.clone());
    }

    request
}

/// The address every test's client connects from.
pub fn client_addr() -> SocketAddr {
    "127.0.0.1:54321".parse().expect("a valid address")
}

/// A finished exchange to `path` on `host`, answered with `status`.
pub fn exchange(host: &str, path: &str, status: u16) -> TrafficExchange {
    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo::http(client_addr(), Some(Destination::new(host, 80))),
        CapturedRequest::new("GET", format!("http://{host}{path}"), HttpVersion::Http11),
    );
    exchange.response = Some(CapturedResponse::new(status, HttpVersion::Http11));
    exchange.timing = Timing {
        started_at: Timestamp::now(),
        upstream_connect_micros: Some(120),
        upstream_response_micros: Some(2_000),
        completed_micros: Some(3_500),
    };

    exchange
}

/// The same exchange, started at `moment`.
pub fn started_at(mut exchange: TrafficExchange, moment: Timestamp) -> TrafficExchange {
    exchange.timing.started_at = moment;

    exchange
}

/// The same exchange, with `method` as its request method.
pub fn with_method(mut exchange: TrafficExchange, method: &str) -> TrafficExchange {
    exchange.request.method = method.to_owned();

    exchange
}

/// An exchange carrying everything that must never leave the server by
/// accident: credentials in a header, a cookie, and bodies.
pub fn secretive_exchange(host: &str) -> TrafficExchange {
    let mut exchange = exchange(host, "/orders", 201);
    exchange.request.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer sk-secret".into()),
    );
    exchange
        .request
        .headers
        .push("Cookie", HeaderValue::Text("session=hunter2".into()));
    exchange.request.body = BodyCapture::complete(b"password=hunter2".to_vec());

    let response = exchange.response.as_mut().expect("a response");
    response
        .headers
        .push("Set-Cookie", HeaderValue::Text("session=refreshed".into()));
    response.body = BodyCapture::complete(br#"{"token":"sk-secret"}"#.to_vec());

    exchange
}

/// An exchange that never got a response, and says why.
pub fn failed_exchange(host: &str, kind: TrafficErrorKind) -> TrafficExchange {
    let mut exchange = exchange(host, "/x", 200);
    exchange.response = None;
    exchange.error = Some(TrafficError::of(kind));

    exchange
}

/// A timestamp `seconds` after the epoch.
pub fn at(seconds: u64) -> Timestamp {
    Timestamp { seconds, nanos: 0 }
}

/// A replayer that records what it was asked to send and answers as told.
///
/// Stands in for the proxy engine, which is what implements
/// [`RequestReplayer`] for real. Nothing here opens a connection: what these
/// tests are about is the protocol between the UI and the server, and the
/// engine's own replay behaviour is tested against real origin servers in
/// `proxy-engine`.
#[derive(Clone, Debug, Default)]
pub struct RecordingReplayer {
    log: Arc<Mutex<ReplayLog>>,
}

/// What a recording replayer has been asked and what it will answer.
#[derive(Debug)]
struct ReplayLog {
    requests: Vec<RequestSpec>,
    answer: Answer,
}

/// What the next replay comes back as.
#[derive(Debug)]
enum Answer {
    /// A status from the destination.
    Status(u16),
    /// A failure, as a captured exchange records one.
    Failure(TrafficError),
}

impl Default for ReplayLog {
    fn default() -> Self {
        Self {
            requests: Vec::new(),
            answer: Answer::Status(200),
        }
    }
}

impl RecordingReplayer {
    /// A replayer that answers `200` until told otherwise.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every request it has been asked to send, in order.
    pub fn requests(&self) -> Vec<RequestSpec> {
        self.locked().requests.clone()
    }

    /// The single request it has been asked to send.
    pub fn only_request(&self) -> RequestSpec {
        let mut requests = self.requests();

        assert_eq!(requests.len(), 1, "expected exactly one replayed request");
        requests.remove(0)
    }

    /// Makes the next replay come back with `status`.
    pub fn answer_with(&self, status: u16) {
        self.locked().answer = Answer::Status(status);
    }

    /// Makes the next replay come back as a failure.
    pub fn fail_with(&self, kind: TrafficErrorKind) {
        self.locked().answer = Answer::Failure(TrafficError::of(kind));
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, ReplayLog> {
        self.log.lock().expect("the log is readable")
    }
}

impl RequestReplayer for RecordingReplayer {
    fn replay(&self, request: RequestSpec) -> ReplayFuture<'_> {
        let outcome = {
            let mut log = self.locked();
            log.requests.push(request);

            // A new id every time, as the engine mints one every time.
            let id = ExchangeId::next();
            match &log.answer {
                Answer::Status(status) => ReplayOutcome::answered(id, *status),
                Answer::Failure(error) => ReplayOutcome::failed(id, error.clone()),
            }
        };

        Box::pin(async move { outcome })
    }
}

/// A `BreakpointControl` the test drives by hand.
///
/// Exactly the boundary the proxy engine implements, and nothing behind it:
/// no matching, no traffic, no pipeline. A test pauses something by calling
/// [`pause`](RecordingBreakpoints::pause), and reads back what the server was
/// told to do about it.
#[derive(Clone, Debug)]
pub struct RecordingBreakpoints {
    inner: Arc<BreakpointState>,
}

#[derive(Debug)]
pub struct BreakpointState {
    configured: Mutex<Vec<Breakpoint>>,
    /// The decisions the server passed on, in order.
    decisions: Mutex<Vec<(PauseId, PauseDecision)>>,
    /// Which pauses are still being held.
    pending: Mutex<Vec<PauseId>>,
    watchers: Mutex<usize>,
    notices: tokio::sync::broadcast::Sender<PauseNotice>,
}

impl RecordingBreakpoints {
    pub fn new() -> Self {
        let (notices, _) = tokio::sync::broadcast::channel(64);

        Self {
            inner: Arc::new(BreakpointState {
                configured: Mutex::new(Vec::new()),
                decisions: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
                watchers: Mutex::new(0),
                notices,
            }),
        }
    }

    /// Starts holding `paused`, and tells whoever is watching.
    pub fn pause(&self, paused: PausedExchange) {
        self.inner
            .pending
            .lock()
            .expect("readable")
            .push(paused.pause_id);
        let _ = self.inner.notices.send(PauseNotice::Paused(paused));
    }

    /// Every decision the server passed on, in order.
    pub fn decisions(&self) -> Vec<(PauseId, PauseDecision)> {
        self.inner.decisions.lock().expect("readable").clone()
    }

    /// How many clients are watching right now.
    pub fn watchers(&self) -> usize {
        *self.inner.watchers.lock().expect("readable")
    }

    /// How many exchanges are still being held.
    pub fn pending(&self) -> usize {
        self.inner.pending.lock().expect("readable").len()
    }
}

impl BreakpointControl for RecordingBreakpoints {
    fn list(&self) -> Vec<Breakpoint> {
        self.inner.configured.lock().expect("readable").clone()
    }

    fn create(&self, new: NewBreakpoint) -> Breakpoint {
        let breakpoint = Breakpoint {
            id: BreakpointId::next(),
            enabled: new.enabled,
            direction: new.direction,
            matcher: new.matcher,
        };
        self.inner
            .configured
            .lock()
            .expect("writable")
            .push(breakpoint.clone());

        breakpoint
    }

    fn update(&self, id: BreakpointId, update: BreakpointUpdate) -> Option<Breakpoint> {
        let mut configured = self.inner.configured.lock().expect("writable");
        let breakpoint = configured.iter_mut().find(|held| held.id == id)?;

        if let Some(enabled) = update.enabled {
            breakpoint.enabled = enabled;
        }
        if let Some(direction) = update.direction {
            breakpoint.direction = direction;
        }
        if let Some(matcher) = update.matcher {
            breakpoint.matcher = matcher;
        }

        Some(breakpoint.clone())
    }

    fn delete(&self, id: BreakpointId) -> bool {
        let mut configured = self.inner.configured.lock().expect("writable");
        let before = configured.len();
        configured.retain(|held| held.id != id);

        configured.len() != before
    }

    fn resolve(&self, id: PauseId, decision: PauseDecision) -> bool {
        let mut pending = self.inner.pending.lock().expect("writable");
        let Some(position) = pending.iter().position(|held| *held == id) else {
            return false;
        };
        pending.remove(position);
        drop(pending);

        let reason = match decision {
            PauseDecision::Continue { .. } => ReleaseReason::Continued,
            PauseDecision::Cancel => ReleaseReason::Cancelled,
        };
        self.inner
            .decisions
            .lock()
            .expect("writable")
            .push((id, decision));
        let _ = self.inner.notices.send(PauseNotice::Released {
            pause_id: id,
            reason,
        });

        true
    }

    fn attach(&self) -> tokio::sync::broadcast::Receiver<PauseNotice> {
        let receiver = self.inner.notices.subscribe();
        *self.inner.watchers.lock().expect("writable") += 1;

        receiver
    }

    fn detach(&self) {
        let mut watchers = self.inner.watchers.lock().expect("writable");
        *watchers = watchers.saturating_sub(1);

        // The engine releases everything pending when the last watcher goes,
        // and so does this: what is under test is that the server calls
        // `detach` at all, which is what makes that happen.
        if *watchers == 0 {
            self.inner.pending.lock().expect("writable").clear();
        }
    }
}

/// A `ThrottleControl` that remembers what it was last set to.
#[derive(Clone, Debug)]
pub struct RecordingThrottle {
    settings: Arc<Mutex<ThrottleSettings>>,
}

impl RecordingThrottle {
    pub fn new() -> Self {
        Self {
            settings: Arc::new(Mutex::new(ThrottleSettings::none())),
        }
    }

    /// What the server last applied.
    pub fn applied(&self) -> ThrottleSettings {
        *self.settings.lock().expect("readable")
    }
}

impl ThrottleControl for RecordingThrottle {
    fn settings(&self) -> ThrottleSettings {
        self.applied()
    }

    fn apply(&self, settings: ThrottleSettings) -> ThrottleSettings {
        *self.settings.lock().expect("writable") = settings;

        settings
    }
}
