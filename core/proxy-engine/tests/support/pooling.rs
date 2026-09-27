//! An instrumented origin server for the connection-pooling tests.
//!
//! The rest of the harness has origins that answer one request and close,
//! which is all a test about *what the proxy wrote* needs. Pooling is about
//! something else: how many connections the proxy opened, which requests went
//! down which one, and whether two of them were ever in flight at the same
//! time. So this origin keeps its connections open and counts.
//!
//! ```text
//!   connection 0 ──▶ GET /a ──▶ GET /b      reuse: one connection, two requests
//!   connection 1 ──▶ GET /c ┐
//!   connection 2 ──▶ GET /d ┴──────────     concurrency: two at once, two connections
//! ```
//!
//! # Proving concurrency without a stopwatch
//!
//! [`OriginPlan::concurrency`] puts a [`Barrier`] in front of the response:
//! no request is answered until that many are waiting at it. A proxy that
//! serialised them would leave the first request waiting for a second that
//! cannot arrive, and the test fails on its timeout rather than on a
//! measurement that happened to be fast enough. Nothing here is timing-based.
//!
//! Every server is in-process, on an OS-chosen port, and every certificate is
//! generated for the test that needs it.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use proxy_tls::{Interceptor, UpstreamTrust};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Barrier;
use tokio::task::JoinHandle;

use super::{generated_ca, read_request, tls_host, trusting, HTTP_1_1_ONLY};

/// How an instrumented origin behaves.
#[derive(Clone, Copy, Debug)]
pub struct OriginPlan {
    /// How many requests must be in flight at once before any is answered.
    ///
    /// `1` answers each request as it arrives. Anything higher is a gate that
    /// only opens when that many requests are waiting at it, which is what
    /// proves they were concurrent.
    pub concurrency: usize,
    /// Whether every response says `Connection: close`.
    pub closes: bool,
    /// Whether the socket is closed after each response without the response
    /// saying so, which is what a keep-alive connection reaped at the far end
    /// looks like to a proxy holding it.
    pub vanishes: bool,
}

impl Default for OriginPlan {
    fn default() -> Self {
        Self {
            concurrency: 1,
            closes: false,
            vanishes: false,
        }
    }
}

impl OriginPlan {
    /// A plan that answers nothing until `requests` are in flight at once.
    pub fn gated(requests: usize) -> Self {
        Self {
            concurrency: requests,
            ..Self::default()
        }
    }

    /// A plan whose responses end their connection, and say so.
    pub fn closing() -> Self {
        Self {
            closes: true,
            ..Self::default()
        }
    }

    /// A plan whose connections go away after a response without saying so.
    pub fn vanishing() -> Self {
        Self {
            vanishes: true,
            ..Self::default()
        }
    }
}

/// What the origin saw.
#[derive(Default)]
struct Instrument {
    /// The request targets served, in order, per connection.
    connections: Mutex<Vec<Vec<String>>>,
    /// How many connections have ended.
    finished: Mutex<usize>,
    /// How many requests are being answered right now.
    active: Mutex<usize>,
    /// The most that were ever being answered at once.
    peak: Mutex<usize>,
}

impl Instrument {
    /// Registers a new connection and returns its number.
    fn opened(&self) -> usize {
        let mut connections = self.connections.lock().expect("connections are writable");
        connections.push(Vec::new());

        connections.len() - 1
    }

    /// Registers a request arriving on connection `id`.
    fn began(&self, id: usize, target: &str) {
        self.connections.lock().expect("connections are writable")[id].push(target.to_owned());

        let mut active = self.active.lock().expect("the count is writable");
        *active += 1;

        let mut peak = self.peak.lock().expect("the peak is writable");
        *peak = (*peak).max(*active);
    }

    /// Registers a request having been answered.
    fn ended(&self) {
        *self.active.lock().expect("the count is writable") -= 1;
    }

    /// Registers a connection closing.
    fn closed(&self) {
        *self.finished.lock().expect("the count is writable") += 1;
    }
}

/// An origin server that keeps its connections open and counts what it saw.
pub struct KeepAliveOrigin {
    addr: SocketAddr,
    host: String,
    trust: Option<UpstreamTrust>,
    instrument: Arc<Instrument>,
    task: JoinHandle<()>,
}

impl KeepAliveOrigin {
    /// A plaintext origin on an OS-chosen loopback port.
    pub async fn plaintext(plan: OriginPlan) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let instrument = Arc::new(Instrument::default());
        let serving = Arc::clone(&instrument);
        let gate = Arc::new(Barrier::new(plan.concurrency));

        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let serving = Arc::clone(&serving);
                let gate = Arc::clone(&gate);

                tokio::spawn(async move {
                    serve(stream, plan, gate, serving).await;
                });
            }
        });

        Self {
            addr,
            host: addr.ip().to_string(),
            trust: None,
            instrument,
            task,
        }
    }

    /// A TLS origin listening as `host`, speaking HTTP/1.1 and only HTTP/1.1.
    ///
    /// `http/1.1` is all it offers over ALPN, which is what makes it the
    /// origin for the case Step 16 exists for: an HTTP/2 client whose origin
    /// cannot multiplex.
    pub async fn tls(host: &str, plan: OriginPlan) -> Self {
        let interceptor =
            Interceptor::offering(generated_ca(), &UpstreamTrust::PublicRoots, HTTP_1_1_ONLY)
                .expect("build an interceptor");
        let trust = trusting(interceptor.authority());
        let interceptor = Arc::new(interceptor);
        let listener = TcpListener::bind((host, 0))
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let instrument = Arc::new(Instrument::default());
        let serving = Arc::clone(&instrument);
        let gate = Arc::new(Barrier::new(plan.concurrency));
        let name = tls_host(host);

        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let serving = Arc::clone(&serving);
                let gate = Arc::clone(&gate);
                let accepting = Arc::clone(&interceptor);
                let name = name.clone();

                tokio::spawn(async move {
                    let Ok(session) = accepting.accept_client(&name, stream).await else {
                        return;
                    };

                    serve(session, plan, gate, serving).await;
                });
            }
        });

        Self {
            addr,
            host: host.to_owned(),
            trust: Some(trust),
            instrument,
            task,
        }
    }

    /// The address the origin is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A `http://host:port` prefix for building absolute request targets.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The `host:port` a `CONNECT` request should name to reach it.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.addr.port())
    }

    /// The host name its certificate is issued for.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// What a proxy has to trust to accept this origin's certificate.
    pub fn trust(&self) -> UpstreamTrust {
        self.trust
            .clone()
            .expect("a plaintext origin has no certificate")
    }

    /// How many connections the origin has accepted.
    pub fn connections(&self) -> usize {
        self.instrument
            .connections
            .lock()
            .expect("connections are readable")
            .len()
    }

    /// How many connections the origin has seen closed.
    pub fn closed_connections(&self) -> usize {
        *self
            .instrument
            .finished
            .lock()
            .expect("the count is readable")
    }

    /// The request targets served on connection `id`, in order.
    pub fn requests_on(&self, id: usize) -> Vec<String> {
        self.instrument
            .connections
            .lock()
            .expect("connections are readable")
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// Every request target the origin served, across every connection.
    pub fn requests(&self) -> Vec<String> {
        self.instrument
            .connections
            .lock()
            .expect("connections are readable")
            .iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// The most requests the origin was answering at any one moment.
    pub fn peak_concurrent(&self) -> usize {
        *self.instrument.peak.lock().expect("the peak is readable")
    }

    /// Waits until the origin has seen every connection it accepted close.
    ///
    /// Bounded, so a proxy that never lets go fails the test instead of
    /// hanging it.
    pub async fn wait_for_connections_to_close(&self) {
        for _ in 0..200 {
            if self.closed_connections() == self.connections() {
                return;
            }

            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        panic!(
            "{} of {} connections were still open",
            self.connections() - self.closed_connections(),
            self.connections()
        );
    }
}

impl Drop for KeepAliveOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves one connection for as long as the other end keeps asking.
///
/// The response body is the request target, so a test can tell which answer
/// belongs to which request, and the connection it was served on is named in
/// a header, so a test can tell which answers shared one.
async fn serve<S>(stream: S, plan: OriginPlan, gate: Arc<Barrier>, instrument: Arc<Instrument>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let id = instrument.opened();
    let mut reader = BufReader::new(stream);

    loop {
        let request = match read_request(&mut reader).await {
            Ok(Some(request)) => request,
            _ => break,
        };
        instrument.began(id, &request.target);

        // Nothing is answered until the plan's worth of requests are waiting
        // here, which is what makes concurrency a fact rather than a timing.
        gate.wait().await;

        let body = request.target.clone();
        let connection = if plan.closes { "close" } else { "keep-alive" };
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/plain\r\n\
             Content-Length: {}\r\n\
             X-Origin-Connection: {id}\r\n\
             Connection: {connection}\r\n\
             \r\n{body}",
            body.len()
        );

        let stream = reader.get_mut();
        if stream.write_all(response.as_bytes()).await.is_err() || stream.flush().await.is_err() {
            break;
        }
        instrument.ended();

        if plan.closes || plan.vanishes {
            let _ = stream.shutdown().await;
            break;
        }
    }

    instrument.closed();
}
