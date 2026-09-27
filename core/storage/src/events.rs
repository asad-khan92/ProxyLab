//! Publishing finished exchanges to whoever is listening.
//!
//! ```text
//! Proxy engine
//!      |
//!      +----> UI subscriber
//!      |
//!      +----> storage subscriber
//!      |
//!      +----> a test
//! ```
//!
//! The proxy publishes; subscribers consume. Nothing subscribed is known to
//! the publisher beyond a count, so a future UI or storage layer can attach
//! without the HTTP code learning anything about it.
//!
//! # What is published
//!
//! One event per exchange, when the exchange is over: a [`TrafficExchange`]
//! with its request, its response or its error, and its timing. A subscriber
//! is never asked to reassemble a request from a stream of lower-level
//! events.
//!
//! # A subscriber that falls behind
//!
//! The channel is bounded, and that is the point. A subscriber that stops
//! reading — a UI that is busy, a debugger sitting on a breakpoint — must not
//! make the proxy grow without limit, and must not make the proxy wait.
//! Publishing never blocks and never fails: once
//! [`DEFAULT_EVENT_CAPACITY`] events are queued for a subscriber, the oldest
//! is dropped to make room. That subscriber's next read is
//! [`StreamError::Lagged`], which says how many it missed, and reading
//! continues from the oldest event still held.
//!
//! Losing events this way is a deliberate trade: this crate does not keep a
//! history, and building one inside the proxy is what would leak. Keeping
//! exchanges is a subscriber's job, and the storage phase's.
//!
//! # Subscribing late
//!
//! A subscriber receives what is published after it subscribes. There is no
//! backlog to catch up on, so anything that needs every exchange subscribes
//! before the proxy starts serving.

use std::fmt;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::traffic::TrafficExchange;

/// How many events are held for a subscriber that is not keeping up.
///
/// Enough to absorb a burst of traffic while a subscriber is busy elsewhere,
/// and small enough that the queue is a bounded amount of memory rather than
/// a traffic history nobody asked for.
pub const DEFAULT_EVENT_CAPACITY: usize = 256;

/// What a subscriber receives: one finished exchange.
///
/// Shared rather than copied, because every subscriber gets the same one and
/// an exchange carries its captured bodies with it.
pub type TrafficEvent = Arc<TrafficExchange>;

/// Publishes finished exchanges.
///
/// Cloning one gives another handle to the same stream, which is how the
/// proxy hands it to each connection.
#[derive(Clone, Debug)]
pub struct EventPublisher {
    sender: broadcast::Sender<TrafficEvent>,
    capacity: usize,
}

impl EventPublisher {
    /// A publisher holding [`DEFAULT_EVENT_CAPACITY`] events per subscriber.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }

    /// A publisher holding `capacity` events per subscriber.
    ///
    /// # Panics
    ///
    /// If `capacity` is zero. A channel that can hold nothing would drop
    /// every event, which is not a configuration — it is a mistake.
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "an event channel must be able to hold an event"
        );

        let (sender, _) = broadcast::channel(capacity);

        Self { sender, capacity }
    }

    /// Starts receiving events published from now on.
    pub fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
        }
    }

    /// Publishes a finished exchange, and says how many subscribers it went
    /// to.
    ///
    /// Never blocks, never waits for a subscriber, and never fails: with no
    /// subscribers the event is dropped, and with a subscriber that is behind
    /// its oldest queued event is dropped instead. The proxy calls this while
    /// finishing a response, so it must not be able to hold one up.
    pub fn publish(&self, exchange: TrafficExchange) -> usize {
        self.publish_shared(Arc::new(exchange))
    }

    /// Publishes an exchange that is already shared.
    ///
    /// For a caller that has somewhere else to put the same exchange — the
    /// persistence layer is given the one the subscribers get — so that an
    /// exchange and its captured bodies are held once rather than copied per
    /// destination.
    pub fn publish_shared(&self, exchange: TrafficEvent) -> usize {
        self.sender.send(exchange).unwrap_or(0)
    }

    /// How many subscribers are listening.
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// How many events are held for a subscriber that is not keeping up.
    ///
    /// At least what the publisher was built with; the channel may round it
    /// up.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for EventPublisher {
    fn default() -> Self {
        Self::new()
    }
}

/// Receives finished exchanges.
#[derive(Debug)]
pub struct EventSubscriber {
    receiver: broadcast::Receiver<TrafficEvent>,
}

impl EventSubscriber {
    /// Waits for the next exchange.
    ///
    /// [`StreamError::Lagged`] means this subscriber was too slow and the
    /// events it names were dropped; reading continues afterwards.
    /// [`StreamError::Closed`] means the proxy is gone and no further event
    /// will arrive.
    pub async fn recv(&mut self) -> Result<TrafficEvent, StreamError> {
        self.receiver.recv().await.map_err(StreamError::from)
    }

    /// The next exchange, if one is already waiting.
    pub fn try_recv(&mut self) -> Option<Result<TrafficEvent, StreamError>> {
        match self.receiver.try_recv() {
            Ok(event) => Some(Ok(event)),
            Err(broadcast::error::TryRecvError::Empty) => None,
            Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                Some(Err(StreamError::Lagged(missed)))
            }
            Err(broadcast::error::TryRecvError::Closed) => Some(Err(StreamError::Closed)),
        }
    }

    /// How many exchanges have been published since this subscriber last
    /// read one.
    ///
    /// A lag indicator, not a memory one: it counts exchanges this subscriber
    /// has not seen, including any that were dropped because it fell behind.
    /// What the channel actually holds is capped by
    /// [`EventPublisher::capacity`].
    pub fn pending(&self) -> usize {
        self.receiver.len()
    }

    /// Another subscriber, receiving what is published from now on.
    pub fn resubscribe(&self) -> Self {
        Self {
            receiver: self.receiver.resubscribe(),
        }
    }
}

/// Why a subscriber did not receive the next exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// The subscriber fell behind and this many exchanges were dropped.
    Lagged(u64),
    /// The publisher is gone; there will be no more exchanges.
    Closed,
}

impl From<broadcast::error::RecvError> for StreamError {
    fn from(error: broadcast::error::RecvError) -> Self {
        match error {
            broadcast::error::RecvError::Lagged(missed) => Self::Lagged(missed),
            broadcast::error::RecvError::Closed => Self::Closed,
        }
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lagged(missed) => {
                write!(f, "fell behind and missed {missed} exchanges")
            }
            Self::Closed => f.write_str("the traffic event stream is closed"),
        }
    }
}

impl std::error::Error for StreamError {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;

    use tokio::time::{timeout, Duration};

    use crate::traffic::{CapturedRequest, ConnectionInfo, ExchangeId, HttpVersion};

    /// Long enough to absorb a loaded machine, short enough that a publisher
    /// which never delivers fails the test instead of hanging it.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    /// An exchange numbered `number`, which is carried in its URL: ids are
    /// minted rather than chosen, so the number is how a test recognises the
    /// exchange it published.
    fn exchange(number: u64) -> TrafficExchange {
        TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::http(client(), None),
            CapturedRequest::new(
                "GET",
                format!("http://example.com/{number}"),
                HttpVersion::Http11,
            ),
        )
    }

    /// The number the exchange an event carries was built with.
    fn number_of(event: &TrafficEvent) -> u64 {
        event
            .request
            .url
            .rsplit('/')
            .next()
            .and_then(|number| number.parse().ok())
            .expect("the url ends in the exchange number")
    }

    async fn next(subscriber: &mut EventSubscriber) -> TrafficEvent {
        timeout(TEST_TIMEOUT, subscriber.recv())
            .await
            .expect("an event arrives")
            .expect("the event stream is healthy")
    }

    #[tokio::test]
    async fn a_subscriber_receives_a_published_exchange() {
        let publisher = EventPublisher::new();
        let mut subscriber = publisher.subscribe();

        publisher.publish(exchange(1));

        assert_eq!(number_of(&next(&mut subscriber).await), 1);
    }

    #[tokio::test]
    async fn every_subscriber_receives_the_same_exchange() {
        let publisher = EventPublisher::new();
        let mut first = publisher.subscribe();
        let mut second = publisher.subscribe();
        let mut third = first.resubscribe();

        assert_eq!(publisher.subscribers(), 3);
        assert_eq!(publisher.publish(exchange(1)), 3);

        for subscriber in [&mut first, &mut second, &mut third] {
            assert_eq!(number_of(&next(subscriber).await), 1);
        }
    }

    #[tokio::test]
    async fn an_exchange_that_is_already_shared_is_published_as_it_is() {
        let publisher = EventPublisher::new();
        let mut subscriber = publisher.subscribe();
        let shared = Arc::new(exchange(1));

        assert_eq!(publisher.publish_shared(Arc::clone(&shared)), 1);

        let event = next(&mut subscriber).await;
        assert!(
            Arc::ptr_eq(&event, &shared),
            "the subscriber received a copy rather than the exchange"
        );
    }

    #[tokio::test]
    async fn exchanges_arrive_in_the_order_they_were_published() {
        let publisher = EventPublisher::new();
        let mut subscriber = publisher.subscribe();

        for id in 1..=5 {
            publisher.publish(exchange(id));
        }

        for id in 1..=5 {
            assert_eq!(number_of(&next(&mut subscriber).await), id);
        }
    }

    #[tokio::test]
    async fn publishing_with_nobody_listening_is_not_a_failure() {
        let publisher = EventPublisher::new();

        assert_eq!(publisher.subscribers(), 0);
        assert_eq!(publisher.publish(exchange(1)), 0);
    }

    #[tokio::test]
    async fn publishing_never_waits_for_a_subscriber_to_read() {
        let publisher = EventPublisher::with_capacity(4);
        // Subscribed and never read from: the queue fills and stays full.
        let _slow = publisher.subscribe();

        // Far more than the channel holds. If publishing waited on a
        // subscriber this would never finish.
        let published = timeout(TEST_TIMEOUT, async {
            for id in 1..=1_000 {
                publisher.publish(exchange(id));
            }
        })
        .await;

        assert!(published.is_ok(), "publishing blocked on a slow subscriber");
    }

    #[tokio::test]
    async fn a_slow_subscribers_queue_does_not_grow_without_bound() {
        let publisher = EventPublisher::with_capacity(4);
        let mut slow = publisher.subscribe();

        for id in 1..=1_000 {
            publisher.publish(exchange(id));
        }

        // The subscriber is told it fell behind, and what it can still read
        // is the handful the channel kept — not the thousand published.
        let mut held = 0;
        let mut lagged = false;
        while let Some(event) = slow.try_recv() {
            match event {
                Ok(_) => held += 1,
                Err(StreamError::Lagged(_)) => lagged = true,
                Err(StreamError::Closed) => break,
            }
        }

        assert!(lagged, "the subscriber was not told it fell behind");
        assert!(
            held <= publisher.capacity(),
            "kept {held} events with a capacity of {}",
            publisher.capacity()
        );
        assert!(held > 0, "the subscriber was left with nothing to read");
    }

    #[tokio::test]
    async fn a_subscriber_can_see_how_far_behind_it_is() {
        let publisher = EventPublisher::with_capacity(8);
        let mut subscriber = publisher.subscribe();

        assert_eq!(subscriber.pending(), 0);
        publisher.publish(exchange(1));
        publisher.publish(exchange(2));
        assert_eq!(subscriber.pending(), 2);

        next(&mut subscriber).await;

        assert_eq!(subscriber.pending(), 1);
    }

    #[tokio::test]
    async fn a_subscriber_that_fell_behind_is_told_how_much_it_missed() {
        let publisher = EventPublisher::with_capacity(4);
        let mut slow = publisher.subscribe();

        for id in 1..=10 {
            publisher.publish(exchange(id));
        }

        let error = timeout(TEST_TIMEOUT, slow.recv())
            .await
            .expect("the subscriber is answered")
            .expect_err("the subscriber fell behind");

        assert_eq!(error, StreamError::Lagged(6));
        assert!(error.to_string().contains("missed 6"), "{error}");

        // Reading carries on from the oldest event still held.
        assert_eq!(number_of(&next(&mut slow).await), 7);
    }

    #[tokio::test]
    async fn a_slow_subscriber_does_not_cost_a_keeping_up_subscriber_anything() {
        let publisher = EventPublisher::with_capacity(4);
        let _slow = publisher.subscribe();
        let mut keeping_up = publisher.subscribe();

        for id in 1..=4 {
            publisher.publish(exchange(id));
            assert_eq!(number_of(&next(&mut keeping_up).await), id);
        }
    }

    #[tokio::test]
    async fn a_subscriber_is_told_when_the_publisher_is_gone() {
        let publisher = EventPublisher::new();
        let mut subscriber = publisher.subscribe();

        drop(publisher);

        let error = timeout(TEST_TIMEOUT, subscriber.recv())
            .await
            .expect("the subscriber is answered")
            .expect_err("the publisher is gone");

        assert_eq!(error, StreamError::Closed);
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn nothing_waiting_is_not_an_error() {
        let publisher = EventPublisher::new();
        let mut subscriber = publisher.subscribe();

        assert!(subscriber.try_recv().is_none());

        publisher.publish(exchange(1));

        let event = subscriber
            .try_recv()
            .expect("an event is waiting")
            .expect("the event stream is healthy");
        assert_eq!(number_of(&event), 1);
    }

    #[tokio::test]
    async fn a_subscriber_that_starts_late_gets_what_follows_it() {
        let publisher = EventPublisher::new();

        publisher.publish(exchange(1));
        let mut subscriber = publisher.subscribe();
        publisher.publish(exchange(2));

        assert_eq!(number_of(&next(&mut subscriber).await), 2);
    }

    #[test]
    #[should_panic(expected = "hold an event")]
    fn a_channel_that_can_hold_nothing_is_refused() {
        EventPublisher::with_capacity(0);
    }
}
