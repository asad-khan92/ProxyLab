//! The idle HTTP/1.1 connections, and the rules for keeping one.
//!
//! An HTTP/1.1 connection carries one exchange at a time. That is the whole
//! shape of this pool: a connection is either *in the pool*, idle and owned
//! by nobody, or *checked out*, owned by exactly one request. There is no
//! third state, and nothing is ever handed to two requests at once, so a
//! physical connection cannot end up with two exchanges on it and this pool
//! does not pipeline.
//!
//! ```text
//!   checkout ──▶ request ──▶ response head ──▶ response body ──▶ release
//!      ▲                                                            │
//!      └────────────────────────  idle  ◀───────────────────────────┘
//! ```
//!
//! # What may go back
//!
//! A connection is returned only when its response body has been read to the
//! end — [`PooledBody`](super::PooledBody) is what notices — and only when
//! the exchange left it fit for another request:
//!
//! * the response did not say `Connection: close`, and was not an HTTP/1.0
//!   response without `Connection: keep-alive`;
//! * the HTTP library has not already closed the connection, which is what
//!   [`SendRequest::is_closed`] reports for an origin that hung up, sent
//!   something unparsable, or framed a body it then broke off.
//!
//! Anything else is dropped, which closes the socket and ends the task
//! driving it.
//!
//! # What is kept, and for how long
//!
//! Bounded on both axes: at most [`PoolLimits::max_idle_per_key`] connections
//! to one origin and at most [`PoolLimits::max_idle_total`] in the pool, with
//! the oldest idle connection dropped to make room. An idle connection older
//! than [`PoolLimits::idle_timeout`] is dropped whether or not there is
//! pressure.
//!
//! Expiry is swept on access rather than by a timer: every checkout and every
//! release walks the pool, which is small by construction, and drops what has
//! expired or been closed. There is no background task, so there is nothing
//! to leak and nothing to shut down — the cost is that a pool nothing touches
//! again keeps its connections until the proxy stops, which
//! [`Http1Pool::close`] then takes care of.
//!
//! # Locking
//!
//! One `std::sync::Mutex` over the whole thing, held for the length of a
//! `HashMap` operation and never across an `await`. Waiting for a connection
//! to be ready, opening one, and sending on one all happen with the lock
//! released — see [`super::manager`], which is the only caller.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hyper::client::conn::http1::SendRequest;

use super::body::UpstreamBody;
use super::key::UpstreamKey;

/// How many idle connections to one origin are kept.
pub const DEFAULT_MAX_IDLE_PER_KEY: usize = 8;

/// How many idle connections are kept in total.
pub const DEFAULT_MAX_IDLE_TOTAL: usize = 64;

/// How long an idle connection is kept before it is dropped.
///
/// Deliberately shorter than the keep-alive timeout of the servers a
/// developer is most likely to be debugging, so the usual outcome is that
/// ProxyLab closes an idle connection rather than discovering that the origin
/// already did.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// How much the pool may hold, and for how long.
///
/// Conservative by default and not configurable from the UI: these are the
/// numbers a debugging proxy on a developer's machine wants, and nothing in
/// ProxyLab asks a user to tune them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolLimits {
    /// Idle connections kept per origin. Default
    /// [`DEFAULT_MAX_IDLE_PER_KEY`].
    pub max_idle_per_key: usize,
    /// Idle connections kept across every origin. Default
    /// [`DEFAULT_MAX_IDLE_TOTAL`].
    pub max_idle_total: usize,
    /// How long an idle connection may sit before it is dropped. Default
    /// [`DEFAULT_IDLE_TIMEOUT`].
    pub idle_timeout: Duration,
}

impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            max_idle_per_key: DEFAULT_MAX_IDLE_PER_KEY,
            max_idle_total: DEFAULT_MAX_IDLE_TOTAL,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

/// One idle connection, and when it became idle.
struct Parked {
    sender: SendRequest<UpstreamBody>,
    since: Instant,
}

/// Everything the lock covers.
#[derive(Default)]
struct Idle {
    connections: HashMap<UpstreamKey, VecDeque<Parked>>,
    /// Kept alongside the map so a limit check is not a walk.
    total: usize,
    /// Set by [`Http1Pool::close`]: nothing is kept after that.
    closed: bool,
}

/// The pool of idle HTTP/1.1 upstream connections.
pub struct Http1Pool {
    limits: PoolLimits,
    idle: Mutex<Idle>,
}

impl Http1Pool {
    /// An empty pool with `limits`.
    pub fn new(limits: PoolLimits) -> Self {
        Self {
            limits,
            idle: Mutex::new(Idle::default()),
        }
    }

    /// The limits this pool keeps to.
    pub fn limits(&self) -> PoolLimits {
        self.limits
    }

    /// Takes an idle connection for `key`, if there is a usable one.
    ///
    /// The connection is *removed* from the pool, so a checked-out connection
    /// belongs to one caller until it is released. Expired and closed
    /// connections are dropped on the way past rather than returned.
    ///
    /// A connection that comes back here is not yet known to be usable — an
    /// origin can hang up at any moment and nothing local hears about it
    /// until something is written. That is what [`super::manager`]'s retry is
    /// for.
    pub fn checkout(&self, key: &UpstreamKey) -> Option<SendRequest<UpstreamBody>> {
        let mut guard = self.lock();
        if guard.closed {
            return None;
        }

        self.sweep(&mut guard);

        let idle = &mut *guard;
        let parked = idle.connections.get_mut(key)?;
        // Most recently idle first: the connection least likely to have been
        // closed at the far end while nobody was looking.
        let taken = parked.pop_back();
        let emptied = parked.is_empty();

        if emptied {
            idle.connections.remove(key);
        }
        if taken.is_some() {
            idle.total -= 1;
        }

        taken.map(|parked| parked.sender)
    }

    /// Puts a connection back, if it is fit to be reused.
    ///
    /// A closed connection, a pool that is shutting down, and a pool with no
    /// room all mean the same thing here: the connection is dropped, which
    /// closes the socket.
    pub fn release(&self, key: UpstreamKey, sender: SendRequest<UpstreamBody>) {
        if sender.is_closed() {
            return;
        }

        let mut guard = self.lock();
        if guard.closed {
            return;
        }

        self.sweep(&mut guard);

        let idle = &mut *guard;
        let parked = idle.connections.entry(key).or_default();
        parked.push_back(Parked {
            sender,
            since: Instant::now(),
        });
        idle.total += 1;

        while parked.len() > self.limits.max_idle_per_key {
            parked.pop_front();
            idle.total -= 1;
        }

        while idle.total > self.limits.max_idle_total {
            if !Self::drop_oldest(idle) {
                break;
            }
        }
    }

    /// Closes every idle connection and keeps no more.
    ///
    /// What a proxy that is stopping does: an idle connection is nobody's,
    /// and holding one open would keep a socket and the task driving it alive
    /// for a process that is ending. Connections already checked out are not
    /// touched — they belong to a request that is still being answered.
    pub fn close(&self) {
        let mut idle = self.lock();

        idle.closed = true;
        idle.connections.clear();
        idle.total = 0;
    }

    /// Whether this pool has stopped keeping connections.
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// How many idle connections are held, across every origin.
    pub fn idle_count(&self) -> usize {
        self.lock().total
    }

    /// How many idle connections are held for `key`.
    pub fn idle_count_for(&self, key: &UpstreamKey) -> usize {
        self.lock()
            .connections
            .get(key)
            .map_or(0, std::collections::VecDeque::len)
    }

    /// Drops every connection that has expired or been closed.
    ///
    /// Called with the lock held, from every path that takes it: the pool is
    /// bounded by [`PoolLimits::max_idle_total`], so this is a walk over a
    /// few dozen entries and not a scan.
    fn sweep(&self, idle: &mut Idle) {
        let timeout = self.limits.idle_timeout;
        let now = Instant::now();
        let mut dropped = 0;

        idle.connections.retain(|_, parked| {
            parked.retain(|connection| {
                let keep = now.duration_since(connection.since) < timeout
                    && !connection.sender.is_closed();
                if !keep {
                    dropped += 1;
                }

                keep
            });

            !parked.is_empty()
        });

        idle.total -= dropped;
    }

    /// Drops the connection that has been idle the longest, anywhere.
    ///
    /// Returns whether there was one to drop.
    fn drop_oldest(idle: &mut Idle) -> bool {
        let oldest = idle
            .connections
            .iter()
            .filter_map(|(key, parked)| parked.front().map(|first| (key.clone(), first.since)))
            .min_by_key(|(_, since)| *since)
            .map(|(key, _)| key);

        let Some(key) = oldest else {
            return false;
        };

        if let Some(parked) = idle.connections.get_mut(&key) {
            if parked.pop_front().is_some() {
                idle.total -= 1;
            }
            if parked.is_empty() {
                idle.connections.remove(&key);
            }
        }

        true
    }

    /// The lock, recovered from a panic elsewhere rather than propagated.
    ///
    /// Nothing under this lock is a data structure a panic could leave
    /// half-written — an entry is pushed or it is not — so a poisoned lock is
    /// no reason to take the proxy down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, Idle> {
        self.idle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// How much is held, and nothing about who is holding it.
impl fmt::Debug for Http1Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let idle = self.lock();

        f.debug_struct("Http1Pool")
            .field("idle", &idle.total)
            .field("origins", &idle.connections.len())
            .field("closed", &idle.closed)
            .finish()
    }
}

/// A checked-out connection's ticket back into the pool.
///
/// Held by the response body, and spent when the body ends. Dropping one
/// without spending it is how a connection is given up: see
/// [`super::PooledBody`].
pub struct Reservation {
    pool: Arc<Http1Pool>,
    key: UpstreamKey,
    sender: SendRequest<UpstreamBody>,
}

impl Reservation {
    /// The ticket for `sender`, which came out of `pool` under `key`.
    pub(super) fn new(
        pool: Arc<Http1Pool>,
        key: UpstreamKey,
        sender: SendRequest<UpstreamBody>,
    ) -> Self {
        Self { pool, key, sender }
    }

    /// Puts the connection back.
    pub(super) fn release(self) {
        self.pool.release(self.key, self.sender);
    }
}

/// The origin the connection reaches, and nothing it carried.
impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reservation")
            .field("origin", &self.key.to_string())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::sleep;

    use crate::http1::upstream;
    use crate::upstream::key::UpstreamDestination;

    fn key(host: &str, port: u16) -> UpstreamKey {
        UpstreamDestination::plaintext(host, port)
            .expect("a usable destination")
            .key()
            .clone()
    }

    /// A server that accepts connections and then says nothing, so a
    /// connection to it stays open and idle.
    struct Quiet {
        addr: std::net::SocketAddr,
        task: tokio::task::JoinHandle<()>,
    }

    impl Quiet {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind ephemeral port");
            let addr = listener.local_addr().expect("local address");
            let task = tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((stream, _)) = listener.accept().await {
                    held.push(stream);
                }
            });

            Self { addr, task }
        }
    }

    impl Drop for Quiet {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// An open, idle HTTP/1.1 connection to `server`.
    async fn connection(server: &Quiet) -> SendRequest<UpstreamBody> {
        let stream = TcpStream::connect(server.addr)
            .await
            .expect("connect to the server");

        upstream::connection(stream)
            .await
            .expect("start an HTTP/1.1 conversation")
    }

    fn pool() -> Arc<Http1Pool> {
        Arc::new(Http1Pool::new(PoolLimits::default()))
    }

    #[tokio::test]
    async fn an_empty_pool_has_nothing_to_hand_out() {
        let pool = pool();

        assert_eq!(pool.idle_count(), 0);
        assert!(pool.checkout(&key("example.com", 80)).is_none());
    }

    #[tokio::test]
    async fn a_released_connection_is_handed_to_the_next_request() {
        let server = Quiet::start().await;
        let pool = pool();
        let key = key("example.com", 80);

        pool.release(key.clone(), connection(&server).await);

        assert_eq!(pool.idle_count(), 1);
        assert!(pool.checkout(&key).is_some());
        // Checking out takes it: it is one request's, not everyone's.
        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn a_connection_is_never_handed_to_a_different_origin() {
        let server = Quiet::start().await;
        let pool = pool();

        pool.release(key("example.com", 80), connection(&server).await);

        assert!(pool.checkout(&key("other.example", 80)).is_none());
        assert!(pool.checkout(&key("example.com", 8080)).is_none());
        assert_eq!(pool.idle_count(), 1);
    }

    #[tokio::test]
    async fn a_closed_connection_is_not_kept() {
        let server = Quiet::start().await;
        let pool = pool();
        let sender = connection(&server).await;

        // Dropping the server's side ends the conversation, which is what the
        // HTTP library reports as a closed connection.
        drop(server);
        for _ in 0..200 {
            if sender.is_closed() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        assert!(
            sender.is_closed(),
            "the connection never noticed the server had gone"
        );

        pool.release(key("example.com", 80), sender);

        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn an_idle_connection_expires() {
        let server = Quiet::start().await;
        let pool = Arc::new(Http1Pool::new(PoolLimits {
            idle_timeout: Duration::from_millis(20),
            ..PoolLimits::default()
        }));
        let key = key("example.com", 80);

        pool.release(key.clone(), connection(&server).await);
        sleep(Duration::from_millis(60)).await;

        assert!(
            pool.checkout(&key).is_none(),
            "an expired connection was handed out"
        );
        assert_eq!(
            pool.idle_count(),
            0,
            "an expired connection was still counted"
        );
    }

    #[tokio::test]
    async fn an_expired_connection_is_swept_even_for_another_origin() {
        let server = Quiet::start().await;
        let pool = Arc::new(Http1Pool::new(PoolLimits {
            idle_timeout: Duration::from_millis(20),
            ..PoolLimits::default()
        }));

        pool.release(key("stale.example", 80), connection(&server).await);
        sleep(Duration::from_millis(60)).await;
        // Touching the pool for a different origin is what sweeps it.
        pool.release(key("fresh.example", 80), connection(&server).await);

        assert_eq!(pool.idle_count(), 1);
        assert_eq!(pool.idle_count_for(&key("stale.example", 80)), 0);
    }

    #[tokio::test]
    async fn one_origin_keeps_no_more_than_its_share() {
        let server = Quiet::start().await;
        let pool = Arc::new(Http1Pool::new(PoolLimits {
            max_idle_per_key: 2,
            ..PoolLimits::default()
        }));
        let key = key("example.com", 80);

        for _ in 0..5 {
            pool.release(key.clone(), connection(&server).await);
        }

        assert_eq!(pool.idle_count_for(&key), 2);
        assert_eq!(pool.idle_count(), 2);
    }

    #[tokio::test]
    async fn the_pool_as_a_whole_is_bounded() {
        let server = Quiet::start().await;
        let pool = Arc::new(Http1Pool::new(PoolLimits {
            max_idle_per_key: 4,
            max_idle_total: 3,
            ..PoolLimits::default()
        }));

        for port in 0..6_u16 {
            pool.release(key("example.com", 8000 + port), connection(&server).await);
        }

        assert_eq!(pool.idle_count(), 3);
    }

    #[tokio::test]
    async fn closing_the_pool_lets_go_of_everything() {
        let server = Quiet::start().await;
        let pool = pool();
        let key = key("example.com", 80);
        pool.release(key.clone(), connection(&server).await);

        pool.close();

        assert!(pool.is_closed());
        assert_eq!(pool.idle_count(), 0);
        assert!(pool.checkout(&key).is_none());
    }

    #[tokio::test]
    async fn a_closed_pool_keeps_nothing_it_is_given() {
        let server = Quiet::start().await;
        let pool = pool();
        pool.close();

        pool.release(key("example.com", 80), connection(&server).await);

        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn a_reservation_is_how_a_connection_gets_back() {
        let server = Quiet::start().await;
        let pool = pool();
        let key = key("example.com", 80);

        let reservation =
            Reservation::new(Arc::clone(&pool), key.clone(), connection(&server).await);
        assert_eq!(
            pool.idle_count(),
            0,
            "a reservation is not yet a pooled connection"
        );

        reservation.release();

        assert_eq!(pool.idle_count_for(&key), 1);
    }

    #[tokio::test]
    async fn a_reservation_that_is_dropped_gives_the_connection_up() {
        let server = Quiet::start().await;
        let pool = pool();
        let key = key("example.com", 80);

        drop(Reservation::new(
            Arc::clone(&pool),
            key.clone(),
            connection(&server).await,
        ));

        assert_eq!(pool.idle_count(), 0);
    }

    #[tokio::test]
    async fn the_debug_form_counts_connections_and_names_nothing_else() {
        let server = Quiet::start().await;
        let pool = pool();
        let key = key("example.com", 80);
        pool.release(key.clone(), connection(&server).await);

        let described = format!("{pool:?}");
        assert!(described.contains("idle: 1"), "{described}");

        let reservation = Reservation::new(Arc::clone(&pool), key, connection(&server).await);
        assert!(format!("{reservation:?}").contains("http://example.com:80"));
    }
}
