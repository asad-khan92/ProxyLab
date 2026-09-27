//! Persisting exchanges without making the proxy wait for a database.
//!
//! ```text
//! finished exchange
//!        ↓
//! bounded queue        ← try to send, never wait
//!        ↓
//! storage worker       ← a blocking thread of its own
//!        ↓
//! SQLite
//! ```
//!
//! [`Storage`] does its work on the calling thread, because that is what
//! SQLite does. The proxy's threads are not free to do that: they are
//! carrying HTTP, and an insert that waits on a disk would be a response that
//! waits on a disk. So an exchange is handed to a queue and the queue is
//! drained by one worker, which sits on a thread from tokio's blocking pool
//! and is the only thing that touches the database on this side.
//!
//! # The queue is bounded, and full means dropped
//!
//! Traffic can arrive faster than a disk can take it. A queue that grew to
//! meet that would turn a burst of traffic into memory the proxy never gets
//! back — the failure the traffic event stream is bounded to avoid, arriving
//! by another route. So the queue holds
//! [`DEFAULT_STORAGE_QUEUE_CAPACITY`] exchanges, and an exchange offered to a
//! full queue is dropped and counted rather than waited on.
//!
//! Dropping is the right trade because of what is lost: a row in a debugging
//! history. What is protected is the thing that must not be lost — the
//! request being carried. [`StorageHandle::dropped`] says how many went that
//! way, and the first one is reported, so this is visible rather than
//! silent.
//!
//! # A failure here does not reach the proxy
//!
//! [`StorageHandle::store`] cannot block and cannot fail: it hands over an
//! exchange and returns. Whether the database took it is discovered by the
//! worker, which counts the failure and logs one safe line. Nothing about it
//! reaches the request that produced the exchange — see the crate
//! documentation.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::events::TrafficEvent;
use crate::stream::StreamMessage;
use crate::traffic::ExchangeId;

use super::error::StorageError;
use super::store::Storage;

/// How many finished exchanges may be waiting to be stored.
///
/// Large enough to absorb the burst a page load is, small enough that the
/// queue is a bounded amount of memory rather than a second traffic history.
pub const DEFAULT_STORAGE_QUEUE_CAPACITY: usize = 1_024;

/// Somewhere a finished exchange is sent to be kept.
///
/// The proxy holds one of these and knows nothing else about persistence:
/// what is on the other side is a database here, and a test's own collector
/// in a test. Accepting an exchange must not block and must not fail, which
/// is what keeps a storage problem from becoming a proxying problem.
pub trait ExchangeSink: fmt::Debug + Send + Sync {
    /// Takes a finished exchange, to be kept if it can be.
    fn accept(&self, exchange: TrafficEvent);
}

/// Somewhere a captured stream message is sent to be kept.
///
/// Separate from [`ExchangeSink`] because a stream message is not an
/// exchange, and a test that only cares about exchanges should not have to
/// implement this. The production worker implements both, on one queue, so
/// an exchange is stored before the messages that belong to it when they are
/// offered in that order.
pub trait StreamMessageSink: fmt::Debug + Send + Sync {
    /// Takes a captured stream message, to be kept if it can be.
    fn accept_stream(&self, message: StreamMessage);
}

/// What the worker is asked to do.
enum Job {
    /// Store this exchange.
    Store(TrafficEvent),
    /// Store this stream message.
    StoreStream(Box<StreamMessage>),
    /// Answer once everything queued before this has been dealt with.
    Flush(oneshot::Sender<()>),
    /// Stop, once everything queued before this has been dealt with.
    Stop,
}

/// The counts a handle reports.
#[derive(Debug, Default)]
struct Counts {
    stored: AtomicU64,
    failed: AtomicU64,
    dropped: AtomicU64,
    reported_a_full_queue: AtomicBool,
}

/// The storage worker: a bounded queue and the thread that drains it.
#[derive(Debug)]
pub struct StorageWorker {
    handle: StorageHandle,
    task: JoinHandle<()>,
}

impl StorageWorker {
    /// Starts a worker for `storage`, with a queue of
    /// [`DEFAULT_STORAGE_QUEUE_CAPACITY`].
    ///
    /// # Panics
    ///
    /// If there is no tokio runtime to start the worker on. The worker runs
    /// on a blocking thread of the runtime, so there has to be one.
    pub fn spawn(storage: Storage) -> Self {
        Self::with_capacity(storage, DEFAULT_STORAGE_QUEUE_CAPACITY)
    }

    /// Starts a worker for `storage`, with a queue of `capacity` exchanges.
    ///
    /// # Panics
    ///
    /// If `capacity` is zero — a queue that can hold nothing would drop
    /// every exchange, which is not a configuration but a mistake — or if
    /// there is no tokio runtime to start the worker on.
    pub fn with_capacity(storage: Storage, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "a storage queue must be able to hold an exchange"
        );

        let (jobs, receiver) = mpsc::channel(capacity);
        let counts = Arc::new(Counts::default());
        let worked = Arc::clone(&counts);
        // A blocking thread, because everything it does is blocking. The
        // alternative — a `spawn_blocking` per exchange — would put an insert
        // on a different thread each time and open a connection's worth of
        // contention for no gain.
        let task = tokio::task::spawn_blocking(move || run(storage, receiver, worked));

        Self {
            handle: StorageHandle { jobs, counts },
            task,
        }
    }

    /// A handle for whoever is producing exchanges.
    pub fn handle(&self) -> StorageHandle {
        self.handle.clone()
    }

    /// Stops the worker, having stored everything already queued.
    ///
    /// The stop goes through the queue like anything else, so exchanges
    /// handed over before it are stored before the worker finishes.
    pub async fn shutdown(self) {
        let _ = self.handle.jobs.send(Job::Stop).await;
        drop(self.handle);

        let _ = self.task.await;
    }
}

/// Hands finished exchanges to a [`StorageWorker`].
///
/// Cloning gives another handle to the same queue and the same counts.
#[derive(Clone, Debug)]
pub struct StorageHandle {
    jobs: mpsc::Sender<Job>,
    counts: Arc<Counts>,
}

impl StorageHandle {
    /// Offers an exchange to be stored, and says whether it was taken.
    ///
    /// Never blocks and never fails. `false` means the queue was full, or
    /// the worker is gone, and this exchange will not be stored; it is
    /// counted in [`dropped`](Self::dropped).
    pub fn store(&self, exchange: TrafficEvent) -> bool {
        match self.jobs.try_send(Job::Store(exchange)) {
            Ok(()) => true,
            Err(_) => {
                self.counts.dropped.fetch_add(1, Ordering::Relaxed);
                // Reported once. A full queue stays full for as long as the
                // burst lasts, and a line per dropped exchange would be a
                // storage problem turning into a logging problem.
                if !self
                    .counts
                    .reported_a_full_queue
                    .swap(true, Ordering::Relaxed)
                {
                    eprintln!("{}", full_queue_report());
                }

                false
            }
        }
    }

    /// Offers a stream message to be stored, and says whether it was taken.
    ///
    /// Never blocks and never fails, for the same reason [`store`](Self::store)
    /// does not.
    pub fn store_stream(&self, message: StreamMessage) -> bool {
        match self.jobs.try_send(Job::StoreStream(Box::new(message))) {
            Ok(()) => true,
            Err(_) => {
                self.counts.dropped.fetch_add(1, Ordering::Relaxed);
                if !self
                    .counts
                    .reported_a_full_queue
                    .swap(true, Ordering::Relaxed)
                {
                    eprintln!("{}", full_queue_report());
                }

                false
            }
        }
    }

    /// Waits until everything offered so far has been dealt with.
    ///
    /// `false` means the worker is gone and there is nothing to wait for.
    /// Unlike [`store`](Self::store) this waits, so it is for a caller that
    /// is allowed to — a shutdown, or a test that wants to read back what it
    /// stored.
    pub async fn flush(&self) -> bool {
        let (reply, waiting) = oneshot::channel();

        if self.jobs.send(Job::Flush(reply)).await.is_err() {
            return false;
        }

        waiting.await.is_ok()
    }

    /// How many exchanges have been stored.
    pub fn stored(&self) -> u64 {
        self.counts.stored.load(Ordering::Relaxed)
    }

    /// How many exchanges the database would not take.
    pub fn failed(&self) -> u64 {
        self.counts.failed.load(Ordering::Relaxed)
    }

    /// How many exchanges were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.counts.dropped.load(Ordering::Relaxed)
    }
}

/// Accepting an exchange is offering it to the queue, and nothing more.
impl ExchangeSink for StorageHandle {
    fn accept(&self, exchange: TrafficEvent) {
        self.store(exchange);
    }
}

impl StreamMessageSink for StorageHandle {
    fn accept_stream(&self, message: StreamMessage) {
        self.store_stream(message);
    }
}

/// Drains the queue until it is closed or a stop arrives.
fn run(storage: Storage, mut jobs: mpsc::Receiver<Job>, counts: Arc<Counts>) {
    // `blocking_recv` is what makes this a worker rather than a task: it
    // waits on the queue from a blocking thread, so nothing on the runtime
    // is held while the database works.
    while let Some(job) = jobs.blocking_recv() {
        match job {
            Job::Store(exchange) => match storage.upsert(&exchange) {
                Ok(()) => {
                    counts.stored.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    counts.failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{}", failure_report(exchange.id, &error));
                }
            },
            Job::StoreStream(message) => match storage.insert_stream_message(&message) {
                Ok(()) => {
                    counts.stored.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    counts.failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("Failed to store stream message {}: {error}", message.id);
                }
            },
            Job::Flush(reply) => {
                let _ = reply.send(());
            }
            Job::Stop => break,
        }
    }
}

/// The line reporting an exchange the database would not take.
///
/// The id, and what the storage layer said. No URL, no header, and no body:
/// an exchange that could not be stored is not a reason to write its
/// contents somewhere else.
fn failure_report(id: ExchangeId, error: &StorageError) -> String {
    format!("Failed to store exchange {id}: {error}")
}

/// The line reporting that exchanges are being dropped.
fn full_queue_report() -> String {
    "The traffic storage queue is full; exchanges are being dropped rather than stored.".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;
    use std::sync::Mutex;

    use tempfile::TempDir;
    use tokio::time::{timeout, Duration};

    use crate::body::BodyCapture;
    use crate::error::{TrafficError, TrafficErrorKind};
    use crate::headers::HeaderValue;
    use crate::timing::Timestamp;
    use crate::traffic::{
        CapturedRequest, ConnectionInfo, Destination, HttpVersion, TrafficExchange,
    };

    use super::super::config::StorageConfig;

    /// Long enough to absorb a loaded machine, short enough that a worker
    /// which never answers fails the test instead of hanging it.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("a valid address")
    }

    fn exchange() -> TrafficExchange {
        let mut exchange = TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::http(client(), Some(Destination::new("example.com", 80))),
            CapturedRequest::new("GET", "http://example.com/x", HttpVersion::Http11),
        );
        exchange.timing = crate::timing::Timing {
            started_at: Timestamp::now(),
            upstream_connect_micros: Some(100),
            upstream_response_micros: Some(2_000),
            completed_micros: Some(3_500),
        };

        exchange
    }

    fn storage(directory: &TempDir) -> Storage {
        Storage::open_in(directory.path()).expect("a store")
    }

    /// A store whose body files cannot be written, because something that is
    /// not a directory is where the body directory should be. Every non-empty
    /// captured body goes to a file, so every exchange with a body fails.
    fn broken_storage(directory: &TempDir) -> Storage {
        let config = StorageConfig::in_directory(directory.path()).with_inline_body_max_bytes(0);
        let storage = Storage::open(config.clone()).expect("a store");

        std::fs::remove_dir_all(&config.bodies).expect("the body directory goes");
        std::fs::write(&config.bodies, b"not a directory").expect("something in its place");

        storage
    }

    async fn flushed(handle: &StorageHandle) {
        assert!(
            timeout(TEST_TIMEOUT, handle.flush())
                .await
                .expect("the worker answers a flush"),
            "the worker was gone"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_exchange_handed_to_the_worker_is_stored() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = storage(&directory);
        let worker = StorageWorker::spawn(storage.clone());
        let handle = worker.handle();
        let exchange = exchange();
        let id = exchange.id;

        assert!(handle.store(Arc::new(exchange)));
        flushed(&handle).await;

        assert_eq!(handle.stored(), 1);
        assert_eq!(handle.failed(), 0);
        assert_eq!(handle.dropped(), 0);
        assert!(storage.get(id).expect("reads").is_some());

        worker.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn everything_queued_before_a_shutdown_is_stored() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = storage(&directory);
        let worker = StorageWorker::spawn(storage.clone());
        let handle = worker.handle();

        for _ in 0..32 {
            assert!(handle.store(Arc::new(exchange())));
        }
        worker.shutdown().await;

        assert_eq!(storage.count().expect("counts"), 32);
        assert_eq!(handle.stored(), 32);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_that_cannot_store_keeps_going_and_counts_the_failures() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = broken_storage(&directory);
        let worker = StorageWorker::spawn(storage);
        let handle = worker.handle();

        for _ in 0..3 {
            let mut exchange = exchange();
            exchange.request.body = BodyCapture::complete(b"a body that needs a file".to_vec());
            assert!(handle.store(Arc::new(exchange)));
        }
        flushed(&handle).await;

        assert_eq!(handle.failed(), 3);
        assert_eq!(handle.stored(), 0);

        // Still answering, still counting: a failure is not the end of the
        // worker.
        assert!(handle.store(Arc::new(exchange())));
        flushed(&handle).await;
        assert_eq!(handle.stored(), 1);

        worker.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_exchange_offered_to_a_full_queue_is_dropped_rather_than_waited_for() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = storage(&directory);
        let worker = StorageWorker::with_capacity(storage.clone(), 1);
        let handle = worker.handle();

        // Far more than the queue holds, offered without ever waiting. If
        // `store` waited on the worker this would not finish.
        let offered = timeout(TEST_TIMEOUT, async {
            let mut taken = 0;
            for _ in 0..2_048 {
                if handle.store(Arc::new(exchange())) {
                    taken += 1;
                }
            }

            taken
        })
        .await
        .expect("offering never blocks");

        flushed(&handle).await;

        assert!(
            handle.dropped() > 0,
            "nothing was dropped by a queue of one"
        );
        assert_eq!(handle.dropped() + offered as u64, 2_048);
        assert_eq!(handle.stored(), offered as u64);

        worker.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_handle_whose_worker_is_gone_drops_what_it_is_given() {
        let directory = TempDir::new().expect("a temporary directory");
        let worker = StorageWorker::spawn(storage(&directory));
        let handle = worker.handle();
        worker.shutdown().await;

        assert!(!handle.store(Arc::new(exchange())));
        assert_eq!(handle.dropped(), 1);
        assert!(!handle.flush().await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sink_is_how_the_proxy_sees_the_worker() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = storage(&directory);
        let worker = StorageWorker::spawn(storage.clone());
        let sink: Arc<dyn ExchangeSink> = Arc::new(worker.handle());

        sink.accept(Arc::new(exchange()));
        flushed(&worker.handle()).await;

        assert_eq!(storage.count().expect("counts"), 1);

        worker.shutdown().await;
    }

    #[test]
    #[should_panic(expected = "hold an exchange")]
    fn a_queue_that_can_hold_nothing_is_refused() {
        let directory = TempDir::new().expect("a temporary directory");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let storage = storage(&directory);

        let _guard = runtime.enter();
        StorageWorker::with_capacity(storage, 0);
    }

    #[test]
    fn a_failure_report_names_the_exchange_and_nothing_it_carried() {
        let id = ExchangeId::next();
        let error = StorageError::Database(rusqlite::Error::QueryReturnedNoRows);

        let report = failure_report(id, &error);

        assert!(report.contains(&id.to_string()), "{report}");
        assert!(!report.contains("http"), "{report}");
    }

    #[test]
    fn a_report_never_carries_a_header_a_body_or_a_url() {
        // The exchange the report is about is deliberately full of the things
        // that must not be logged.
        let mut exchange = exchange();
        exchange.request.url = "http://alice:hunter2@example.com/secret".to_owned();
        exchange.request.headers.push(
            "Authorization",
            HeaderValue::Text("Bearer sk-secret".into()),
        );
        exchange.request.body = BodyCapture::complete(b"password=hunter2".to_vec());
        exchange.error = Some(TrafficError::of(TrafficErrorKind::UpstreamConnect));

        let report = failure_report(
            exchange.id,
            &StorageError::Database(rusqlite::Error::QueryReturnedNoRows),
        );

        for secret in ["hunter2", "sk-secret", "alice", "example.com", "secret"] {
            assert!(!report.contains(secret), "{report} carries {secret}");
        }
    }

    #[test]
    fn a_full_queue_is_reported_without_naming_what_was_dropped() {
        let report = full_queue_report();

        assert!(report.contains("full"), "{report}");
        assert!(!report.contains("http"), "{report}");
    }

    #[test]
    fn the_default_queue_holds_a_burst_of_traffic() {
        assert_eq!(DEFAULT_STORAGE_QUEUE_CAPACITY, 1_024);
    }

    /// A sink that keeps what it is given, which is what a test wants
    /// instead of a database.
    #[derive(Debug, Default)]
    struct Collector {
        accepted: Mutex<Vec<ExchangeId>>,
    }

    impl ExchangeSink for Collector {
        fn accept(&self, exchange: TrafficEvent) {
            self.accepted
                .lock()
                .expect("the collector is writable")
                .push(exchange.id);
        }
    }

    #[test]
    fn a_sink_can_be_anything_that_takes_an_exchange() {
        let collector = Arc::new(Collector::default());
        let sink: Arc<dyn ExchangeSink> = Arc::clone(&collector) as Arc<dyn ExchangeSink>;
        let exchange = exchange();
        let id = exchange.id;

        sink.accept(Arc::new(exchange));

        assert_eq!(
            *collector.accepted.lock().expect("readable"),
            vec![id],
            "the sink did not receive the exchange"
        );
    }
}
