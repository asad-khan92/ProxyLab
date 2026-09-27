//! Capturing the traffic that goes through the proxy.
//!
//! ```text
//! HTTP request
//!       ↓
//! capture the request head
//!       ↓
//! forward upstream        ← the body streams past, and a copy is kept
//!       ↓
//! capture the response head
//!       ↓
//! forward downstream      ← the body streams past, and a copy is kept
//!       ↓
//! publish a TrafficExchange
//! ```
//!
//! [`Capture`] is the shared half: the limit on how much of a body is kept,
//! the publisher every finished exchange goes to, and — when the proxy is
//! storing traffic — the sink it is persisted through. One is built when the
//! proxy starts and handed to each connection.
//!
//! [`Recorder`] is the per-exchange half. It is created when a request
//! arrives, collects what happens to it, and publishes once — when the
//! response has been forwarded in full, when the exchange fails, or when it
//! is abandoned. Whichever of those happens, exactly one exchange is
//! published, and it is published without waiting for anybody to read it.
//!
//! # Capture never changes what is forwarded
//!
//! A body is copied as it streams past, in [`body`], not buffered and
//! replayed. The proxy forwards a 20 MB response in full whether the capture
//! limit is 20 MB or 1 KB; what the limit decides is how much of it ProxyLab
//! remembers, and a capture that hit the limit says so.
//!
//! # Publishing and persisting
//!
//! A finished exchange goes to both, in this order:
//!
//! ```text
//! exchange finished
//!        ↓
//! hand to the storage sink   durable, and dropped only if its queue is full
//!        ↓
//! publish to subscribers     live, and lossy for a subscriber that is behind
//! ```
//!
//! Both are a hand-off to a channel: neither blocks, neither fails, and
//! neither waits for what is on the other end. So the order is not about
//! latency — it is about what a subscriber can rely on. Storage first means
//! that an exchange which has reached a subscriber has already been offered
//! to storage: it is queued, or it was dropped because the queue was full,
//! but it is never still waiting to be handed over. A future UI that sees an
//! exchange go past and asks the database for it is asking about something
//! storage has already been told about.
//!
//! Persistence is a sink of its own and not one of the subscribers. A
//! subscriber that falls behind loses events by design, which is right for a
//! live view and wrong for a history; and there is exactly one sink, so an
//! exchange is stored once however many subscribers are watching.
//!
//! # What reaches a log
//!
//! Nothing from here. The model's `Debug` renders sizes and names rather than
//! header values and bodies (see `proxy-storage`), and this module writes no
//! log lines of its own: the proxy's existing per-request reporting is
//! unchanged.

pub mod body;
pub mod message;
pub mod stream;

mod recorder;

use std::net::SocketAddr;
use std::sync::Arc;

use hyper::Request;
use proxy_storage::{
    CaptureConfig, CapturedRequest, ConnectionInfo, Destination, EventPublisher, EventSubscriber,
    ExchangeSink, StreamMessageSink, StreamPublisher, StreamSubscriber,
};

pub use recorder::Recorder;
pub use stream::StreamTap;

/// What the proxy captures, and where finished exchanges go.
///
/// Cheap to clone: a clone shares the same publisher, the same storage sink,
/// and the same configuration, which is how one capture serves every
/// connection.
#[derive(Clone, Debug)]
pub struct Capture {
    config: CaptureConfig,
    publisher: EventPublisher,
    stream_publisher: StreamPublisher,
    sink: Option<Arc<dyn ExchangeSink>>,
    stream_sink: Option<Arc<dyn StreamMessageSink>>,
}

impl Capture {
    /// A capture that publishes on `publisher`, keeping bodies up to
    /// `config`'s limit, and stores nothing.
    pub fn new(config: CaptureConfig, publisher: EventPublisher) -> Self {
        Self {
            config,
            publisher,
            stream_publisher: StreamPublisher::new(),
            sink: None,
            stream_sink: None,
        }
    }

    /// A capture with the default body limit and a publisher of its own.
    pub fn with_defaults() -> Self {
        Self::new(CaptureConfig::default(), EventPublisher::new())
    }

    /// The same capture, persisting every finished exchange through `sink`.
    ///
    /// One sink, however many subscribers: an exchange is published to
    /// everyone watching and stored once.
    pub fn storing_into(mut self, sink: Arc<dyn ExchangeSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// The same capture, persisting every stream message through `sink`.
    pub fn storing_streams(mut self, sink: Arc<dyn StreamMessageSink>) -> Self {
        self.stream_sink = Some(sink);
        self
    }

    /// What the capture is allowed to keep.
    pub fn config(&self) -> CaptureConfig {
        self.config
    }

    /// The publisher finished exchanges go to.
    pub fn publisher(&self) -> &EventPublisher {
        &self.publisher
    }

    /// The publisher captured stream messages go to.
    pub fn stream_publisher(&self) -> &StreamPublisher {
        &self.stream_publisher
    }

    /// Whether finished exchanges are being persisted.
    pub fn is_storing(&self) -> bool {
        self.sink.is_some()
    }

    /// Starts receiving exchanges finished from now on.
    pub fn subscribe(&self) -> EventSubscriber {
        self.publisher.subscribe()
    }

    /// Starts receiving stream-message summaries from now on.
    pub fn subscribe_streams(&self) -> StreamSubscriber {
        self.stream_publisher.subscribe()
    }

    /// Starts recording an exchange that is already described.
    pub fn begin(&self, connection: ConnectionInfo, request: CapturedRequest) -> Recorder {
        Recorder::new(
            self.config,
            self.publisher.clone(),
            self.stream_publisher.clone(),
            self.sink.clone(),
            self.stream_sink.clone(),
            connection,
            request,
        )
    }

    /// Starts recording a plaintext request the proxy is about to forward.
    ///
    /// The URL and the destination are read from the absolute-form request
    /// target, so a request that is about to be refused for not having one
    /// is still recorded — with no destination, because it named none.
    pub fn forwarded<B>(&self, client: SocketAddr, request: &Request<B>) -> Recorder {
        let uri = request.uri();
        let connection = ConnectionInfo::http(client, message::destination(uri));

        self.begin(
            connection,
            message::request_head(request, message::forwarded_url(uri)),
        )
    }

    /// Starts recording a request read out of an intercepted TLS session.
    ///
    /// `url` names the host the session was opened to and the path the
    /// request asked for; the request itself carries only the path, because a
    /// client inside a session believes it is talking to the origin.
    pub fn intercepted<B>(
        &self,
        client: SocketAddr,
        destination: Destination,
        url: String,
        request: &Request<B>,
    ) -> Recorder {
        self.begin(
            ConnectionInfo::https(client, Some(destination)),
            message::request_head(request, url),
        )
    }

    /// Starts recording a `CONNECT` request.
    ///
    /// A `CONNECT` that becomes an intercepted session is not itself traffic:
    /// the requests inside the session are, and each of those is recorded on
    /// its own. This recording exists for the `CONNECT` that never becomes a
    /// session — a destination that could not be reached, a handshake that
    /// failed — which would otherwise be traffic that simply vanished. Its
    /// URL is the authority the client named, because that is all a `CONNECT`
    /// names.
    pub fn connecting<B>(&self, client: SocketAddr, url: String, request: &Request<B>) -> Recorder {
        self.begin(
            ConnectionInfo::https(client, None),
            message::request_head(request, url),
        )
    }
}

impl Default for Capture {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use hyper::Method;
    use proxy_storage::Scheme;

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    fn request(method: &str, target: &str) -> Request<()> {
        Request::builder()
            .method(method)
            .uri(target)
            .header("accept", "application/json")
            .body(())
            .expect("valid request")
    }

    #[test]
    fn a_capture_shares_its_publisher_with_its_clones() {
        let capture = Capture::with_defaults();
        let subscriber = capture.subscribe();
        let clone = capture.clone();

        assert_eq!(clone.publisher().subscribers(), 1);
        drop(subscriber);
        assert_eq!(clone.publisher().subscribers(), 0);
    }

    #[test]
    fn the_default_capture_keeps_the_default_amount_of_a_body() {
        assert_eq!(Capture::with_defaults().config(), CaptureConfig::default());
    }

    #[test]
    fn a_forwarded_request_is_recorded_with_its_url_and_destination() {
        let capture = Capture::with_defaults();
        let mut subscriber = capture.subscribe();

        let recorder =
            capture.forwarded(client(), &request("GET", "http://example.com:8080/x?y=1"));
        drop(recorder);

        let event = subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy");
        assert_eq!(event.request.method, "GET");
        assert_eq!(event.request.url, "http://example.com:8080/x?y=1");
        assert_eq!(
            event.request.headers.text("accept"),
            Some("application/json")
        );
        assert_eq!(event.connection.scheme, Scheme::Http);
        assert_eq!(
            event.connection.destination,
            Some(Destination::new("example.com", 8080))
        );
    }

    #[test]
    fn a_request_target_that_names_no_host_is_recorded_without_a_destination() {
        let capture = Capture::with_defaults();
        let mut subscriber = capture.subscribe();

        drop(capture.forwarded(client(), &request("GET", "/index.html")));

        let event = subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy");
        assert_eq!(event.request.url, "/index.html");
        assert_eq!(event.connection.destination, None);
    }

    #[test]
    fn an_intercepted_request_is_recorded_as_https() {
        let capture = Capture::with_defaults();
        let mut subscriber = capture.subscribe();

        drop(capture.intercepted(
            client(),
            Destination::new("example.com", 443),
            "https://example.com/api".to_owned(),
            &request("GET", "/api"),
        ));

        let event = subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy");
        assert_eq!(event.request.url, "https://example.com/api");
        assert_eq!(event.connection.scheme, Scheme::Https);
        assert_eq!(
            event.connection.destination,
            Some(Destination::new("example.com", 443))
        );
    }

    #[test]
    fn a_connect_is_recorded_by_the_authority_it_named() {
        let capture = Capture::with_defaults();
        let mut subscriber = capture.subscribe();

        let mut connect = request("GET", "example.com:443");
        *connect.method_mut() = Method::CONNECT;
        drop(capture.connecting(client(), "example.com:443".to_owned(), &connect));

        let event = subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy");
        assert_eq!(event.request.method, "CONNECT");
        assert_eq!(event.request.url, "example.com:443");
        assert_eq!(event.connection.scheme, Scheme::Https);
    }
}
