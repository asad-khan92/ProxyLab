//! Recording one exchange, from the request head to the published event.
//!
//! A recorder is created when a request arrives and lives as long as anything
//! is still happening to that request: the service handling it, the request
//! body on its way upstream, the response body on its way back. It is shared
//! rather than owned, so whichever of those finishes last is the one that
//! publishes.
//!
//! Exactly one exchange is published per recorder. The normal end is the
//! response body reaching the client; a failure publishes immediately with no
//! response and an error; and if neither happens — a client that went away, a
//! response that was dropped part-written — the recorder publishes as it is
//! dropped, so a request never simply disappears.
//!
//! The one thing that publishes nothing is [`Recorder::discard`], for a
//! `CONNECT` that turned into an intercepted session: the requests inside the
//! session are the traffic, and each of those is recorded on its own.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use hyper::body::Bytes;
use hyper::{Request, Response};
use proxy_storage::{
    BodyCollector, CaptureConfig, CapturedRequest, CapturedResponse, ConnectionInfo, Destination,
    EventPublisher, ExchangeId, ExchangeSink, StreamMessageSink, StreamPublisher, TimingRecorder,
    TrafficError, TrafficErrorKind, TrafficExchange,
};

use super::body::{CapturingBody, Outcome, Role};
use super::message;
use super::stream::StreamTap;

/// Records what happens to one request.
///
/// Cloning gives another handle to the same recording; the exchange is
/// published when the last of them is finished with it.
#[derive(Clone)]
pub struct Recorder {
    recording: Arc<Recording>,
}

impl Recorder {
    /// Starts recording, with the request head already known.
    pub(super) fn new(
        config: CaptureConfig,
        publisher: EventPublisher,
        stream_publisher: StreamPublisher,
        sink: Option<Arc<dyn ExchangeSink>>,
        stream_sink: Option<Arc<dyn StreamMessageSink>>,
        connection: ConnectionInfo,
        request: CapturedRequest,
    ) -> Self {
        Self {
            recording: Arc::new(Recording {
                id: ExchangeId::next(),
                config,
                publisher,
                stream_publisher,
                sink,
                stream_sink,
                state: Mutex::new(State {
                    connection,
                    request,
                    request_body: BodyCollector::with_config(&config),
                    response: None,
                    response_body: BodyCollector::with_config(&config),
                    error: None,
                    timing: TimingRecorder::started(),
                    published: false,
                    announced: false,
                    discarded: false,
                }),
            }),
        }
    }

    /// Which exchange this is.
    pub fn id(&self) -> ExchangeId {
        self.recording.id
    }

    /// Records where the request is going, once that is known.
    pub fn destination(&self, destination: Destination) {
        self.recording.lock().connection.destination = Some(destination);
    }

    /// Records that the connection to the origin is being opened.
    pub fn upstream_connect_started(&self) {
        self.recording.lock().timing.upstream_connect();
    }

    /// Records the response head the origin sent.
    pub fn upstream_responded<B>(&self, response: &Response<B>) {
        let mut state = self.recording.lock();

        state.response = Some(message::response_head(response));
        state.timing.upstream_response();
    }

    /// Records the request head as a Modify Request rule left it.
    ///
    /// The exchange is remembered by the request that was sent, which is the
    /// same promise a breakpoint edit keeps — there is one request in the
    /// capture, and it is the one the origin received. The body is not
    /// touched here: it is captured as it goes upstream, so a replaced body
    /// is captured as the replacement.
    pub fn request_edited<B>(&self, request: &Request<B>, url: String) {
        self.recording.lock().request = message::request_head(request, url);
    }

    /// Records the response head as a breakpoint or a Modify Response rule
    /// left it.
    ///
    /// The timing is deliberately not touched: the moment the origin answered
    /// is the moment the origin answered, whatever was done to what it said
    /// afterwards. What changes is the head, because the head the client is
    /// given is the one this exchange should be remembered by — see
    /// [`crate::controls`], which captures the message that actually
    /// continued through the pipeline rather than keeping two of them.
    pub fn response_edited<B>(&self, response: &Response<B>) {
        self.recording.lock().response = Some(message::response_head(response));
    }

    /// Ends the exchange with a failure and publishes it.
    ///
    /// The first failure is the one kept: what went wrong first is what
    /// explains the rest.
    pub fn fail(&self, error: TrafficError) {
        self.recording.fail(error);
    }

    /// Ends the exchange without publishing anything.
    pub fn discard(&self) {
        self.recording.lock().discarded = true;
    }

    /// Wraps a request body so it is captured as it is forwarded.
    pub fn capture_request<B>(&self, body: B) -> CapturingBody<B>
    where
        B: hyper::body::Body<Data = Bytes>,
    {
        CapturingBody::new(body, self.clone(), Role::Request)
    }

    /// Wraps a response body so it is captured as it is forwarded.
    ///
    /// This is also what ends the exchange: when the body has been forwarded
    /// in full, the recorder publishes.
    pub fn capture_response<B>(&self, body: B) -> CapturingBody<B>
    where
        B: hyper::body::Body<Data = Bytes>,
    {
        CapturingBody::new(body, self.clone(), Role::Response)
    }

    /// Publishes the exchange as it stands, so a long-lived stream can be
    /// selected before it ends.
    ///
    /// A handshake and an event-stream head are stored and shown as soon as
    /// they exist. Later [`publish`](Recording::publish) updates the same
    /// row rather than inserting a second one.
    pub fn announce(&self) {
        self.recording.announce();
    }

    /// A tap that records WebSocket messages and SSE events onto this exchange.
    pub fn stream_tap(&self) -> StreamTap {
        StreamTap::new(
            self.recording.id,
            self.recording.config.max_capture_body_size,
            self.recording.stream_publisher.clone(),
            self.recording.stream_sink.clone(),
        )
    }

    /// How much of a body or a stream payload this recording will keep.
    pub fn capture_limit(&self) -> usize {
        self.recording.config.max_capture_body_size
    }

    /// Records body bytes that have just been forwarded.
    pub(super) fn record(&self, role: Role, bytes: &[u8]) {
        let mut state = self.recording.lock();

        match role {
            Role::Request => state.request_body.push(bytes),
            Role::Response => state.response_body.push(bytes),
        }
    }

    /// Records that a body has finished, one way or another.
    ///
    /// Only the response body decides the exchange. A request body that
    /// stopped early is not itself a failure — the origin is allowed to
    /// answer without reading all of it — and if it did break the exchange,
    /// the failure that follows is what says so.
    pub(super) fn finish(&self, role: Role, outcome: Outcome) {
        if role == Role::Request {
            return;
        }

        match outcome {
            Outcome::Complete => self.recording.publish(),
            Outcome::Failed => self.fail(TrafficError::new(
                TrafficErrorKind::ResponseForwarding,
                "the response body ended before it was complete",
            )),
            Outcome::Abandoned => self.fail(TrafficError::new(
                TrafficErrorKind::ClientDisconnected,
                "the client connection closed before the response was complete",
            )),
        }
    }
}

/// The id, and nothing an exchange is carrying.
impl fmt::Debug for Recorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorder")
            .field("id", &self.recording.id)
            .finish()
    }
}

/// One exchange being recorded.
struct Recording {
    id: ExchangeId,
    config: CaptureConfig,
    publisher: EventPublisher,
    stream_publisher: StreamPublisher,
    sink: Option<Arc<dyn ExchangeSink>>,
    stream_sink: Option<Arc<dyn StreamMessageSink>>,
    state: Mutex<State>,
}

impl Recording {
    /// The recording's state.
    ///
    /// A poisoned lock is recovered from rather than panicked on: something
    /// else already panicked, and turning that into a second panic — inside a
    /// `Drop`, on a connection task — would take the proxy down over a
    /// capture.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Records a failure and publishes the exchange.
    fn fail(&self, error: TrafficError) {
        {
            let mut state = self.lock();
            if state.error.is_none() {
                state.error = Some(error);
            }
        }

        self.publish();
    }

    /// Stores and publishes the exchange as it stands, without ending it.
    fn announce(&self) {
        {
            let state = self.lock();
            if state.published || state.announced || state.discarded {
                return;
            }
        }

        let exchange = self.snapshot(false);
        self.emit(&exchange, true);

        self.lock().announced = true;
    }

    /// Publishes the exchange, if it has not been published or discarded,
    /// and hands it to the storage sink.
    ///
    /// Never blocks and never fails. The publisher drops an event rather
    /// than wait for a subscriber, and the sink drops an exchange rather
    /// than wait for a database, so finishing a response is never held up by
    /// whoever is listening or by whatever is keeping it.
    ///
    /// Storage first, then subscribers: see the module documentation for
    /// why. The exchange is shared rather than copied, so both get the same
    /// one and its captured bodies are held once.
    fn publish(&self) {
        {
            let state = self.lock();
            if state.published || state.discarded {
                return;
            }
        }

        let already_announced = self.lock().announced;
        let exchange = self.snapshot(true);

        {
            let mut state = self.lock();
            state.published = true;
            state.announced = true;
        }

        self.emit(&exchange, !already_announced);
    }

    /// Builds the exchange as it currently stands.
    ///
    /// `complete` records that the timing is finished, which is true of a
    /// publish and not of an announce: an event stream that is still open
    /// has not completed.
    fn snapshot(&self, complete: bool) -> Arc<TrafficExchange> {
        let mut state = self.lock();
        if complete {
            state.timing.completed();
        }

        let mut request = state.request.clone();
        request.body = state.request_body.capture();

        let response = match state.response.clone() {
            Some(mut response) => {
                response.body = state.response_body.capture();
                Some(response)
            }
            None => None,
        };

        Arc::new(TrafficExchange {
            id: self.id,
            connection: state.connection.clone(),
            request,
            response,
            error: state.error.clone(),
            timing: state.timing.timing(),
        })
    }

    fn emit(&self, exchange: &Arc<TrafficExchange>, to_subscribers: bool) {
        if let Some(sink) = &self.sink {
            sink.accept(Arc::clone(exchange));
        }

        if to_subscribers {
            self.publisher.publish_shared(Arc::clone(exchange));
        }
    }
}

/// A recording that was never finished is still traffic, so it is published
/// as what it is: an exchange that did not complete.
impl Drop for Recording {
    fn drop(&mut self) {
        {
            let mut state = self.lock();
            if state.published || state.discarded {
                return;
            }
            if state.error.is_none() && !state.announced {
                state.error = Some(TrafficError::of(TrafficErrorKind::Incomplete));
            }
        }

        self.publish();
    }
}

/// Everything recorded about one exchange so far.
struct State {
    connection: ConnectionInfo,
    request: CapturedRequest,
    request_body: BodyCollector,
    response: Option<CapturedResponse>,
    response_body: BodyCollector,
    error: Option<TrafficError>,
    timing: TimingRecorder,
    published: bool,
    announced: bool,
    discarded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;
    use std::sync::Mutex;

    use hyper::StatusCode;
    use proxy_storage::{EventSubscriber, TrafficEvent};

    use crate::capture::Capture;

    /// A sink that keeps what it is given, which is what a test wants
    /// instead of a database.
    #[derive(Debug, Default)]
    struct Collector {
        accepted: Mutex<Vec<TrafficEvent>>,
    }

    impl Collector {
        fn accepted(&self) -> Vec<TrafficEvent> {
            self.accepted
                .lock()
                .expect("the collector is readable")
                .clone()
        }
    }

    impl ExchangeSink for Collector {
        fn accept(&self, exchange: TrafficEvent) {
            self.accepted
                .lock()
                .expect("the collector is writable")
                .push(exchange);
        }
    }

    /// A capture that publishes and stores, and the sink it stores into.
    fn storing_capture(limit: usize) -> (Capture, Arc<Collector>) {
        let collector = Arc::new(Collector::default());
        let capture = capture(limit).storing_into(Arc::clone(&collector) as Arc<dyn ExchangeSink>);

        (capture, collector)
    }

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    fn capture(limit: usize) -> Capture {
        Capture::new(CaptureConfig::new(limit), EventPublisher::new())
    }

    fn recorder(capture: &Capture) -> Recorder {
        capture.begin(
            ConnectionInfo::http(client(), Some(Destination::new("example.com", 80))),
            CapturedRequest::new(
                "GET",
                "http://example.com/x",
                proxy_storage::HttpVersion::Http11,
            ),
        )
    }

    fn published(subscriber: &mut EventSubscriber) -> TrafficEvent {
        subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy")
    }

    fn nothing_published(subscriber: &mut EventSubscriber) {
        assert!(
            subscriber.try_recv().is_none(),
            "an exchange was published when none should have been"
        );
    }

    fn response(status: StatusCode) -> Response<()> {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(())
            .expect("valid response")
    }

    #[test]
    fn nothing_is_published_while_the_exchange_is_still_running() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.upstream_connect_started();
        recorder.upstream_responded(&response(StatusCode::OK));

        nothing_published(&mut subscriber);
        drop(recorder);
    }

    #[test]
    fn a_finished_response_publishes_the_whole_exchange() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.upstream_connect_started();
        recorder.record(Role::Request, b"asked");
        recorder.upstream_responded(&response(StatusCode::OK));
        recorder.record(Role::Response, b"answered");
        recorder.finish(Role::Response, Outcome::Complete);

        let event = published(&mut subscriber);
        assert!(event.succeeded());
        assert_eq!(event.status(), Some(200));
        assert_eq!(event.request.body.as_text(), Some("asked"));
        assert_eq!(
            event.response.as_ref().expect("a response").body.as_text(),
            Some("answered")
        );
        assert!(event.timing.upstream_connect_micros.is_some());
        assert!(event.timing.upstream_response_micros.is_some());
        assert!(event.timing.completed_micros.is_some());
    }

    #[test]
    fn an_exchange_is_published_only_once() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Response, Outcome::Complete);
        recorder.finish(Role::Response, Outcome::Complete);
        recorder.fail(TrafficError::of(TrafficErrorKind::UpstreamConnect));
        drop(recorder);

        published(&mut subscriber);
        nothing_published(&mut subscriber);
    }

    #[test]
    fn a_failure_publishes_an_exchange_with_no_response() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.fail(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        let event = published(&mut subscriber);
        assert!(!event.succeeded());
        assert_eq!(event.response, None);
        assert_eq!(
            event.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::UpstreamConnect)
        );
        assert!(event.timing.completed_micros.is_some());
    }

    #[test]
    fn the_first_failure_is_the_one_reported() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.fail(TrafficError::of(TrafficErrorKind::UpstreamTls));
        recorder.fail(TrafficError::of(TrafficErrorKind::Incomplete));

        assert_eq!(
            published(&mut subscriber)
                .error
                .as_ref()
                .map(TrafficError::kind),
            Some(TrafficErrorKind::UpstreamTls)
        );
    }

    #[test]
    fn an_abandoned_exchange_is_published_as_incomplete() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();

        drop(recorder(&capture));

        let event = published(&mut subscriber);
        assert_eq!(
            event.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::Incomplete)
        );
    }

    #[test]
    fn a_response_the_client_stopped_reading_is_published_as_a_disconnect() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.upstream_responded(&response(StatusCode::OK));
        recorder.record(Role::Response, b"half");
        recorder.finish(Role::Response, Outcome::Abandoned);

        let event = published(&mut subscriber);
        assert_eq!(
            event.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::ClientDisconnected)
        );
        // What did arrive is still captured: a half-delivered response is
        // exactly the thing a developer is trying to look at.
        assert_eq!(
            event.response.as_ref().expect("a response").body.as_text(),
            Some("half")
        );
    }

    #[test]
    fn a_response_body_that_broke_is_published_as_a_forwarding_failure() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.upstream_responded(&response(StatusCode::OK));
        recorder.finish(Role::Response, Outcome::Failed);

        assert_eq!(
            published(&mut subscriber)
                .error
                .as_ref()
                .map(TrafficError::kind),
            Some(TrafficErrorKind::ResponseForwarding)
        );
    }

    #[test]
    fn a_request_body_ending_does_not_end_the_exchange() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Request, Outcome::Complete);
        recorder.finish(Role::Request, Outcome::Abandoned);

        nothing_published(&mut subscriber);
        drop(recorder);
    }

    #[test]
    fn a_discarded_recording_publishes_nothing() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.discard();
        recorder.fail(TrafficError::of(TrafficErrorKind::ClientTls));
        drop(recorder);

        nothing_published(&mut subscriber);
    }

    #[test]
    fn a_body_over_the_limit_is_truncated_and_says_how_long_it_was() {
        let capture = capture(4);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.upstream_responded(&response(StatusCode::OK));
        recorder.record(Role::Response, b"hello ");
        recorder.record(Role::Response, b"world");
        recorder.finish(Role::Response, Outcome::Complete);

        let event = published(&mut subscriber);
        let body = &event.response.as_ref().expect("a response").body;
        assert_eq!(body.data(), b"hell");
        assert_eq!(body.size(), 11);
        assert!(body.is_truncated());
    }

    #[test]
    fn a_destination_learned_later_reaches_the_published_exchange() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = capture.begin(
            ConnectionInfo::https(client(), None),
            CapturedRequest::new(
                "CONNECT",
                "example.com:443",
                proxy_storage::HttpVersion::Http11,
            ),
        );

        recorder.destination(Destination::new("example.com", 443));
        recorder.fail(TrafficError::of(TrafficErrorKind::ClientTls));

        assert_eq!(
            published(&mut subscriber).connection.destination,
            Some(Destination::new("example.com", 443))
        );
    }

    #[test]
    fn every_exchange_gets_an_id_of_its_own() {
        let capture = capture(1024);

        let first = recorder(&capture);
        let second = recorder(&capture);

        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn a_finished_exchange_reaches_the_storage_sink() {
        let (capture, stored) = storing_capture(1024);
        let recorder = recorder(&capture);

        recorder.upstream_responded(&response(StatusCode::OK));
        recorder.record(Role::Response, b"answered");
        recorder.finish(Role::Response, Outcome::Complete);

        let accepted = stored.accepted();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].id, recorder.id());
        assert_eq!(
            accepted[0]
                .response
                .as_ref()
                .expect("a response")
                .body
                .as_text(),
            Some("answered")
        );
    }

    #[test]
    fn a_failed_exchange_reaches_the_storage_sink_too() {
        let (capture, stored) = storing_capture(1024);
        let recorder = recorder(&capture);

        recorder.fail(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        let accepted = stored.accepted();
        assert_eq!(accepted.len(), 1);
        assert_eq!(
            accepted[0].error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::UpstreamConnect)
        );
    }

    #[test]
    fn an_exchange_is_stored_once_however_many_subscribers_there_are() {
        let (capture, stored) = storing_capture(1024);
        let mut first = capture.subscribe();
        let mut second = capture.subscribe();
        let mut third = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Response, Outcome::Complete);

        for subscriber in [&mut first, &mut second, &mut third] {
            published(subscriber);
        }
        assert_eq!(stored.accepted().len(), 1);
    }

    #[test]
    fn an_exchange_that_reaches_a_subscriber_has_already_reached_storage() {
        let (capture, stored) = storing_capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Response, Outcome::Complete);

        // Read the event first, then look at what storage was given: an
        // exchange a subscriber has seen was handed over before it was
        // published, so this can never be a race.
        let event = published(&mut subscriber);
        assert_eq!(
            stored.accepted().first().map(|stored| stored.id),
            Some(event.id)
        );
    }

    #[test]
    fn a_subscriber_and_storage_are_given_the_same_exchange() {
        let (capture, stored) = storing_capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Response, Outcome::Complete);

        let event = published(&mut subscriber);
        let accepted = stored.accepted();
        assert!(
            Arc::ptr_eq(&event, &accepted[0]),
            "the exchange was copied rather than shared"
        );
    }

    #[test]
    fn a_discarded_recording_stores_nothing() {
        let (capture, stored) = storing_capture(1024);
        let recorder = recorder(&capture);

        recorder.discard();
        drop(recorder);

        assert!(stored.accepted().is_empty());
    }

    #[test]
    fn an_abandoned_recording_still_reaches_storage() {
        let (capture, stored) = storing_capture(1024);

        drop(recorder(&capture));

        assert_eq!(stored.accepted().len(), 1);
        assert_eq!(
            stored.accepted()[0].error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::Incomplete)
        );
    }

    #[test]
    fn a_capture_with_no_sink_publishes_just_the_same() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        recorder.finish(Role::Response, Outcome::Complete);

        assert!(!capture.is_storing());
        published(&mut subscriber);
    }

    #[test]
    fn debug_names_the_exchange_and_nothing_it_carries() {
        let capture = capture(1024);
        let recorder = recorder(&capture);

        let rendered = format!("{recorder:?}");

        assert!(rendered.contains(&recorder.id().to_string()), "{rendered}");
        assert!(!rendered.contains("example.com"), "{rendered}");
    }
}
