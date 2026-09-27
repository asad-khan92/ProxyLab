//! Where an upstream request finds a connection to go out on.
//!
//! One [`UpstreamManager`] is built when the proxy starts and shared by
//! everything that sends a request to an origin: a forwarded plaintext
//! request, a request read out of an intercepted TLS session, a request a
//! Map Remote rule sent somewhere else, and a replay. They all arrive with
//! the proxy's canonical outgoing request — origin form, HTTP/1.1 shape — and
//! this is where it becomes bytes on a connection.
//!
//! ```text
//!                        UpstreamManager
//!                              │
//!                ┌─────────────┴─────────────┐
//!                ▼                           ▼
//!        HTTP/1.1 pool                HTTP/2 connections
//!   one exchange per connection    many streams per connection
//!   several connections per origin    one connection per origin
//! ```
//!
//! # Which connection a request gets
//!
//! In this order, and the first that works wins:
//!
//! 1. **An HTTP/2 connection to this origin**, if there is one. Streams are
//!    what HTTP/2 is for, so a second connection would be a waste and is not
//!    opened; the request takes a clone of the sender and goes, alongside
//!    however many others are already in flight.
//! 2. **An idle HTTP/1.1 connection to this origin**, if there is one. It is
//!    taken *out* of the pool for the duration, so it is one request's and
//!    nobody else's until its response body has been read to the end.
//! 3. **A new connection.** Which protocol it speaks is ALPN's answer, not a
//!    setting: a TLS session that agreed `h2` becomes an HTTP/2 connection
//!    and is registered as one, and anything else is HTTP/1.1.
//!
//! This is what stops concurrent requests to one HTTP/1.1 origin from queuing
//! behind each other. Four streams of one intercepted HTTP/2 connection whose
//! origin only speaks HTTP/1.1 find an empty pool, open four connections, and
//! run side by side; when they finish, four connections are pooled and the
//! next four requests reuse them.
//!
//! # Retrying, and when not to
//!
//! A pooled connection can be dead without anything local knowing: the origin
//! closed it while it was idle and the news arrives only when something is
//! written. So the request is offered with
//! [`try_send_request`](hyper::client::conn::http1::SendRequest::try_send_request),
//! which hands the request *back* when it was never written to the socket.
//!
//! That hand-back is the whole of the retry rule:
//!
//! ```text
//! request came back  ──▶ the origin never saw it ──▶ safe: try once more
//! request did not    ──▶ it may have been acted on ──▶ fail the exchange
//! ```
//!
//! A request that may have reached the origin is never sent again, whatever
//! its method: this proxy does not decide that somebody's `POST` was
//! idempotent. There is one retry, onto a connection opened for it, and a
//! failure there is the exchange's failure.
//!
//! # What pooling does not change
//!
//! Rules are evaluated before anything here is asked for a connection, so a
//! blocked request and a request answered from a file never reach this module
//! at all, and a mapped request arrives with the destination the rule chose —
//! see [`crate::http1::ruled`]. Every request is still its own
//! [`TrafficExchange`](proxy_storage::TrafficExchange) with its own bodies and
//! its own timing; a connection carrying four of them in turn is four
//! exchanges, and nothing about the connection appears in any of them.
//!
//! TLS is not weakened either. A session is opened by the same
//! [`Interceptor`] the rest of the proxy uses, verifying the origin's
//! certificate against the same trust anchors and sending the same SNI, and
//! the name it was verified for is part of the pool key — so a session
//! established for one host is never handed to a request for another.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hyper::body::Incoming;
use hyper::client::conn::{http1 as http1_client, http2 as http2_client};
use hyper::header::CONNECTION;
use hyper::{Request, Response, Version};
use proxy_tls::{Interceptor, Protocol};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use super::body::{PooledBody, UpstreamBody};
use super::key::{UpstreamDestination, UpstreamKey};
use super::pool::{Http1Pool, PoolLimits, Reservation};
use crate::http1::upstream::UpstreamError;
use crate::{http2, origin};

/// The upstream connections the proxy is holding, and how to get one.
pub struct UpstreamManager {
    /// The one thing that opens TLS sessions here, which is why the pool key
    /// carries a verified host name and not a TLS configuration: there is
    /// only ever one configuration in a manager's life.
    interceptor: Arc<Interceptor>,
    http1: Arc<Http1Pool>,
    /// At most one HTTP/2 connection per origin, because one is enough.
    http2: Mutex<HashMap<UpstreamKey, http2_client::SendRequest<UpstreamBody>>>,
    stopping: AtomicBool,
}

impl UpstreamManager {
    /// A manager that opens TLS sessions through `interceptor`, with the
    /// default [`PoolLimits`].
    pub fn new(interceptor: Arc<Interceptor>) -> Self {
        Self::with_limits(interceptor, PoolLimits::default())
    }

    /// The same, keeping to `limits`.
    pub fn with_limits(interceptor: Arc<Interceptor>, limits: PoolLimits) -> Self {
        Self {
            interceptor,
            http1: Arc::new(Http1Pool::new(limits)),
            http2: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        }
    }

    /// The idle HTTP/1.1 connections this manager is holding.
    pub fn pool(&self) -> &Arc<Http1Pool> {
        &self.http1
    }

    /// How many origins have an HTTP/2 connection registered.
    pub fn http2_connections(&self) -> usize {
        self.lock_http2().len()
    }

    /// Sends `request` to `destination` and waits for the response head.
    ///
    /// The response comes back with its body still arriving, so the caller
    /// can forward it as it streams. Reading that body to its end is also
    /// what returns an HTTP/1.1 connection to the pool — see
    /// [`PooledBody`] — so a caller that drops the response drops the
    /// connection with it rather than leaving it half-read in the pool.
    pub async fn send(
        &self,
        destination: &UpstreamDestination,
        request: Request<UpstreamBody>,
    ) -> Result<Response<PooledBody>, UpstreamError> {
        let key = destination.key();
        let mut request = request;

        // An HTTP/2 connection is the one to use if there is one: another
        // connection to the same origin would buy nothing a stream does not.
        if let Some(sender) = self.registered_http2(key) {
            let canonical = origin::http1_shape(&request);
            let rewritten =
                origin::into_http2(request, destination.scheme(), destination.authority());

            match attempt_http2(sender, rewritten).await {
                Attempt::Answered(response) => return Ok(response.map(PooledBody::plain)),
                Attempt::Failed(error) => return Err(error),
                // The connection would not take it — a `GOAWAY`, or a
                // connection closed since it was registered. Nothing reached
                // the origin, so the request is put back in its canonical
                // shape and tried on a connection of its own.
                Attempt::NotSent(recovered, _) => {
                    self.forget_http2(key);
                    request = canonical.restore(recovered);
                }
            }
        }

        // One pooled connection is tried, once. A second stale one is
        // possible but unlikely, and opening a connection is the cheaper way
        // to be sure than walking the pool.
        if let Some(sender) = self.checked_out(key).await {
            let (sender, outcome) = attempt_http1(sender, request).await;

            match outcome {
                Attempt::Answered(response) => return Ok(self.parked(key, sender, response)),
                Attempt::Failed(error) => return Err(error),
                Attempt::NotSent(recovered, _) => request = recovered,
            }
        }

        self.send_on_a_new_connection(destination, request).await
    }

    /// Opens a connection to `destination` and sends `request` on it.
    ///
    /// The end of the line: a request that is not written here has run out of
    /// connections to be tried on, so the failure is the exchange's.
    async fn send_on_a_new_connection(
        &self,
        destination: &UpstreamDestination,
        request: Request<UpstreamBody>,
    ) -> Result<Response<PooledBody>, UpstreamError> {
        let key = destination.key();

        match self.establish(destination).await? {
            Connected::Http1(sender) => {
                let (sender, outcome) = attempt_http1(sender, request).await;

                match outcome {
                    Attempt::Answered(response) => Ok(self.parked(key, sender, response)),
                    Attempt::NotSent(_, error) | Attempt::Failed(error) => Err(error),
                }
            }
            Connected::Http2(sender) => {
                self.register_http2(key.clone(), sender.clone());
                let rewritten =
                    origin::into_http2(request, destination.scheme(), destination.authority());

                match attempt_http2(sender, rewritten).await {
                    Attempt::Answered(response) => Ok(response.map(PooledBody::plain)),
                    Attempt::NotSent(_, error) | Attempt::Failed(error) => Err(error),
                }
            }
        }
    }

    /// Opens a connection for one upgrade, which nothing else will ever use.
    ///
    /// A WebSocket connection is not a request the manager carries: it is a
    /// handshake, and then a socket that stops being HTTP. So this deliberately
    /// does none of what [`Self::send`] does — no pooled connection is checked
    /// out, no HTTP/2 connection is reused, and what comes back is registered
    /// nowhere. When the tunnel ends, the connection ends with it.
    ///
    /// A TLS destination is reached over a session that offers `http/1.1`
    /// alone. Verification is unchanged — the same trust anchors, the same
    /// host name check, the same SNI — and only the protocol list differs,
    /// because there is no form of the WebSocket handshake this proxy carries
    /// over HTTP/2.
    pub async fn open_upgradable(
        &self,
        destination: &UpstreamDestination,
    ) -> Result<http1_client::SendRequest<UpstreamBody>, UpstreamError> {
        if self.is_stopping() {
            return Err(UpstreamError::Stopping);
        }

        let stream = TcpStream::connect((destination.host(), destination.port()))
            .await
            .map_err(UpstreamError::Connect)?;

        let Some(host) = destination.verified() else {
            return crate::http1::upstream::upgradable_connection(stream).await;
        };

        let session = self
            .interceptor
            .connect_upstream_http11(host, stream)
            .await
            .map_err(UpstreamError::Tls)?;

        crate::http1::upstream::upgradable_connection(session).await
    }

    /// Takes an already-established connection into the manager's keeping.
    ///
    /// For the one connection ProxyLab opens before it has a request to send:
    /// a `CONNECT` proves the origin is reachable, and its TLS session is
    /// handshaken, before the client is told the connection is established —
    /// see [`crate::intercept`]. That session is not thrown away and opened
    /// again; it is handed here, and the first request inside the intercepted
    /// session picks it up.
    ///
    /// `protocol` is what that session's ALPN settled on. An HTTP/2 session
    /// is registered as the connection to this origin, so every stream in the
    /// intercepted connection multiplexes over it.
    pub async fn adopt<S>(
        &self,
        destination: &UpstreamDestination,
        stream: S,
        protocol: Protocol,
    ) -> Result<(), UpstreamError>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        match protocol {
            // One HTTP/2 connection per origin is all HTTP/2 needs, so a
            // session arriving where there is already a usable one is let go
            // of here rather than handshaken and then dropped. The `CONNECT`
            // still opened a socket and verified the origin's certificate,
            // because that is what it promises the client before it answers.
            Protocol::Http2 if self.registered_http2(destination.key()).is_some() => {}
            Protocol::Http2 => {
                let sender = http2::upstream::connection(stream).await?;
                self.register_http2(destination.key().clone(), sender);
            }
            Protocol::Http11 => {
                let sender = crate::http1::upstream::connection(stream).await?;
                self.http1.release(destination.key().clone(), sender);
            }
        }

        Ok(())
    }

    /// Closes every idle connection and opens no more.
    ///
    /// What a proxy that is stopping does. Connections a request is still
    /// using are not touched: they end when that request does, which is the
    /// shutdown behaviour the rest of the proxy already has.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.http1.close();
        self.lock_http2().clear();
    }

    /// Whether this manager has stopped opening connections.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Opens a connection to `destination`, of whichever kind it needs.
    async fn establish(
        &self,
        destination: &UpstreamDestination,
    ) -> Result<Connected, UpstreamError> {
        if self.is_stopping() {
            return Err(UpstreamError::Stopping);
        }

        let stream = TcpStream::connect((destination.host(), destination.port()))
            .await
            .map_err(UpstreamError::Connect)?;

        let Some(host) = destination.verified() else {
            return Ok(Connected::Http1(
                crate::http1::upstream::connection(stream).await?,
            ));
        };

        let session = self
            .interceptor
            .connect_upstream(host, stream)
            .await
            .map_err(UpstreamError::Tls)?;

        match proxy_tls::upstream_protocol(&session) {
            Protocol::Http2 => Ok(Connected::Http2(
                http2::upstream::connection(session).await?,
            )),
            Protocol::Http11 => Ok(Connected::Http1(
                crate::http1::upstream::connection(session).await?,
            )),
        }
    }

    /// An idle connection for `key` that is ready to carry a request.
    ///
    /// The readiness wait is deliberately outside the pool's lock: it is an
    /// `await`, and nothing in this crate holds a lock across one.
    async fn checked_out(
        &self,
        key: &UpstreamKey,
    ) -> Option<http1_client::SendRequest<UpstreamBody>> {
        while let Some(mut sender) = self.http1.checkout(key) {
            // The previous exchange is over — that is what put it back — but
            // the connection task may not have been polled since. This waits
            // for it to say so, and fails for a connection that has ended.
            if sender.ready().await.is_ok() {
                return Some(sender);
            }
        }

        None
    }

    /// Wraps `response` so that its connection goes back to the pool when its
    /// body has been read to the end, if it may be reused at all.
    fn parked(
        &self,
        key: &UpstreamKey,
        sender: http1_client::SendRequest<UpstreamBody>,
        response: Response<Incoming>,
    ) -> Response<PooledBody> {
        let reusable = !sender.is_closed() && keeps_the_connection(&response);
        let (parts, body) = response.into_parts();

        let body = if reusable {
            PooledBody::reserving(
                body,
                Reservation::new(Arc::clone(&self.http1), key.clone(), sender),
            )
        } else {
            PooledBody::plain(body)
        };

        Response::from_parts(parts, body)
    }

    /// The HTTP/2 connection to `key`, if there is a usable one.
    ///
    /// A connection that has closed is forgotten on the way past, so the next
    /// request opens a new one rather than finding the same dead entry.
    fn registered_http2(
        &self,
        key: &UpstreamKey,
    ) -> Option<http2_client::SendRequest<UpstreamBody>> {
        let mut registered = self.lock_http2();
        let sender = registered.get(key)?;

        if sender.is_closed() {
            registered.remove(key);

            return None;
        }

        Some(sender.clone())
    }

    /// Records `sender` as the HTTP/2 connection to `key`.
    ///
    /// A connection already registered and still usable is left alone: two
    /// requests that raced to open one both get an answer, and only the first
    /// connection is kept. The other is used for the request that opened it
    /// and closes when that request is finished with it.
    fn register_http2(&self, key: UpstreamKey, sender: http2_client::SendRequest<UpstreamBody>) {
        let mut registered = self.lock_http2();

        match registered.get(&key) {
            Some(existing) if !existing.is_closed() => {}
            _ => {
                registered.insert(key, sender);
            }
        }
    }

    /// Forgets the HTTP/2 connection to `key`, if it is the closed one.
    ///
    /// Only if it is closed: another request may have opened a healthy
    /// connection in the meantime, and this one has no business removing it.
    fn forget_http2(&self, key: &UpstreamKey) {
        let mut registered = self.lock_http2();

        if registered
            .get(key)
            .is_some_and(http2_client::SendRequest::is_closed)
        {
            registered.remove(key);
        }
    }

    /// The registry lock, recovered from a panic elsewhere rather than
    /// propagated: an entry is inserted or it is not, so there is no
    /// half-written state a panic could have left behind.
    fn lock_http2(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<UpstreamKey, http2_client::SendRequest<UpstreamBody>>>
    {
        self.http2
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// How much is held, and nothing about what any of it carried.
impl fmt::Debug for UpstreamManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpstreamManager")
            .field("idle_http1", &self.http1.idle_count())
            .field("http2_connections", &self.http2_connections())
            .field("stopping", &self.is_stopping())
            .finish()
    }
}

/// A connection that has just been opened, and what it turned out to speak.
enum Connected {
    Http1(http1_client::SendRequest<UpstreamBody>),
    Http2(http2_client::SendRequest<UpstreamBody>),
}

/// What came of offering a request to a connection.
enum Attempt {
    /// The origin answered.
    Answered(Response<Incoming>),
    /// The request was never written to the socket, so the origin cannot have
    /// acted on it, and it is safe to send somewhere else.
    NotSent(Request<UpstreamBody>, UpstreamError),
    /// It may have reached the origin, or the answer was unusable. Either
    /// way this exchange is over.
    Failed(UpstreamError),
}

/// Offers `request` to an HTTP/1.1 connection, handing the connection back.
///
/// The connection is taken by value and returned, rather than borrowed,
/// because whether it may be pooled afterwards is a question about the
/// connection and not about the request.
async fn attempt_http1(
    mut sender: http1_client::SendRequest<UpstreamBody>,
    request: Request<UpstreamBody>,
) -> (http1_client::SendRequest<UpstreamBody>, Attempt) {
    let outcome = match sender.try_send_request(request).await {
        Ok(response) => Attempt::Answered(response),
        Err(mut refusal) => match refusal.take_message() {
            Some(request) => Attempt::NotSent(request, UpstreamError::Send(refusal.into_error())),
            None => Attempt::Failed(UpstreamError::Send(refusal.into_error())),
        },
    };

    (sender, outcome)
}

/// Offers `request` to an HTTP/2 connection.
///
/// The connection is not handed back: it was never taken out of anywhere, and
/// the clone this used is dropped when the stream is finished with it.
async fn attempt_http2(
    mut sender: http2_client::SendRequest<UpstreamBody>,
    request: Request<UpstreamBody>,
) -> Attempt {
    match sender.try_send_request(request).await {
        Ok(response) => Attempt::Answered(response),
        Err(mut refusal) => match refusal.take_message() {
            Some(request) => Attempt::NotSent(request, UpstreamError::Send(refusal.into_error())),
            None => Attempt::Failed(UpstreamError::Send(refusal.into_error())),
        },
    }
}

/// Whether a response leaves its connection fit for another request.
///
/// The HTTP/1.1 rules, and nothing invented here: a response that says
/// `Connection: close` ends the connection, and an HTTP/1.0 response ends it
/// unless it asked to be kept. Nothing is removed from or added to the
/// message to make it poolable — this only reads what the origin said.
///
/// A response whose body is framed by the connection closing is not caught
/// here and does not need to be: the HTTP library closes the connection when
/// that body ends, and a closed connection is not pooled.
fn keeps_the_connection<B>(response: &Response<B>) -> bool {
    let mut closes = false;
    let mut keeps_alive = false;

    for value in response.headers().get_all(CONNECTION) {
        let Ok(value) = value.to_str() else {
            // A `Connection` header that is not text is one this proxy cannot
            // read, and a connection whose terms are unknown is not reused.
            return false;
        };

        for token in value.split(',') {
            let token = token.trim();
            if token.eq_ignore_ascii_case("close") {
                closes = true;
            }
            if token.eq_ignore_ascii_case("keep-alive") {
                keeps_alive = true;
            }
        }
    }

    if closes {
        return false;
    }

    match response.version() {
        Version::HTTP_10 => keeps_alive,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::header::HeaderValue;
    use hyper::StatusCode;
    use proxy_tls::{CertificateAuthority, UpstreamTrust};
    use tokio::net::TcpListener;

    use super::super::body::boxed;
    use super::super::key::UpstreamDestination;

    fn manager() -> UpstreamManager {
        UpstreamManager::new(Arc::new(
            Interceptor::new(
                CertificateAuthority::generate().expect("generate a CA"),
                &UpstreamTrust::PublicRoots,
            )
            .expect("build an interceptor"),
        ))
    }

    fn request() -> Request<UpstreamBody> {
        Request::builder()
            .uri("/")
            .header("host", "example.com")
            .body(boxed(Empty::<Bytes>::new()))
            .expect("valid request")
    }

    /// An address nothing is listening on: a port is bound to learn a free one
    /// and then released, so connecting to it is refused rather than ignored.
    async fn unused() -> UpstreamDestination {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        drop(listener);

        UpstreamDestination::plaintext(&addr.ip().to_string(), addr.port())
            .expect("a usable destination")
    }

    #[tokio::test]
    async fn a_manager_starts_holding_nothing() {
        let manager = manager();

        assert_eq!(manager.pool().idle_count(), 0);
        assert_eq!(manager.http2_connections(), 0);
        assert!(!manager.is_stopping());
    }

    #[tokio::test]
    async fn a_destination_that_refuses_the_connection_is_a_connect_failure() {
        let manager = manager();

        let error = manager
            .send(&unused().await, request())
            .await
            .expect_err("nothing is listening");

        assert!(matches!(error, UpstreamError::Connect(_)), "{error}");
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(manager.pool().idle_count(), 0);
    }

    #[tokio::test]
    async fn a_manager_that_has_stopped_opens_no_more_connections() {
        let manager = manager();
        let destination = unused().await;
        manager.shutdown();

        let error = manager
            .send(&destination, request())
            .await
            .expect_err("the manager is stopping");

        assert!(matches!(error, UpstreamError::Stopping), "{error}");
        assert!(manager.is_stopping());
        assert!(manager.pool().is_closed());
    }

    #[tokio::test]
    async fn the_debug_form_counts_connections_and_names_nothing_else() {
        let described = format!("{:?}", manager());

        assert!(described.contains("idle_http1: 0"), "{described}");
        assert!(described.contains("http2_connections: 0"), "{described}");
    }

    fn response(version: Version, connection: Option<&str>) -> Response<()> {
        let mut response = Response::new(());

        *response.version_mut() = version;
        *response.status_mut() = StatusCode::OK;
        if let Some(value) = connection {
            response.headers_mut().insert(
                CONNECTION,
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }

        response
    }

    #[test]
    fn an_ordinary_http_1_1_response_leaves_the_connection_usable() {
        assert!(keeps_the_connection(&response(Version::HTTP_11, None)));
    }

    #[test]
    fn a_response_that_says_close_ends_the_connection() {
        assert!(!keeps_the_connection(&response(
            Version::HTTP_11,
            Some("close")
        )));
    }

    #[test]
    fn close_is_recognised_however_it_is_written() {
        for value in ["Close", "CLOSE", "keep-alive, close", "close , foo"] {
            assert!(
                !keeps_the_connection(&response(Version::HTTP_11, Some(value))),
                "{value} should have ended the connection"
            );
        }
    }

    #[test]
    fn an_http_1_0_response_ends_the_connection_unless_it_asks_not_to() {
        assert!(!keeps_the_connection(&response(Version::HTTP_10, None)));
        assert!(keeps_the_connection(&response(
            Version::HTTP_10,
            Some("keep-alive")
        )));
    }

    #[test]
    fn a_connection_header_that_cannot_be_read_ends_the_connection() {
        let mut built = response(Version::HTTP_11, None);
        built.headers_mut().insert(
            CONNECTION,
            HeaderValue::from_bytes(&[0xff, 0xfe]).expect("a header value"),
        );

        assert!(!keeps_the_connection(&built));
    }
}
