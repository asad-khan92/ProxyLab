//! The proxy's TCP listener and connection lifecycle.
//!
//! Accepts client connections and hands each one to its own task. What is
//! said on a connection is not this module's concern: that belongs to
//! [`crate::http1`] — or, on the listener a transparently captured flow
//! arrives at, to [`crate::transparent`]. The accept loop, the shutdown, and
//! the grace a stopping proxy gives its connections are the same for both.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use proxy_tls::Interceptor;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

use rules::RuleEngine;

use crate::capture::Capture;
use crate::controls::Controls;
use crate::shutdown::Shutdown;
use crate::transparent::{self, Own};
use crate::upstream::UpstreamManager;

/// Address the proxy listens on.
pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:9090";

/// How long a stopping proxy waits for the connections still in flight.
///
/// Long enough for a response being written to be finished, which is the
/// whole reason to wait at all, and short enough that a connection carrying
/// something that does not end — an event stream, a WebSocket whose tunnel
/// has not noticed yet — cannot keep a stopped proxy alive. Nothing is
/// retried afterwards: the connections are dropped, and dropping them is what
/// closes them.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// A bound TCP listener that accepts proxy connections.
#[derive(Debug)]
pub struct Listener {
    inner: TcpListener,
    local_addr: SocketAddr,
    owns_upstream: bool,
    entry: Entry,
}

/// What an accepted connection is expected to say.
#[derive(Debug, Clone)]
enum Entry {
    /// An HTTP proxy client: forwarded requests and `CONNECT` tunnels.
    Proxy,
    /// ProxyLab's transparent proxy provider, handing over captured flows.
    Transparent(Arc<Own>),
}

impl Listener {
    /// Binds `addr`.
    ///
    /// Passing port `0` (`"127.0.0.1:0"`) lets the operating system choose a
    /// free port, which [`local_addr`](Self::local_addr) then reports.
    pub async fn bind(addr: &str) -> io::Result<Self> {
        let inner = TcpListener::bind(addr).await?;
        let local_addr = inner.local_addr()?;

        Ok(Self {
            inner,
            local_addr,
            owns_upstream: true,
            entry: Entry::Proxy,
        })
    }

    /// The same listener, sharing its upstream connections with one that
    /// outlives it.
    ///
    /// A listener that stops normally closes the idle connections the proxy
    /// is holding open to origin servers, because a stopping listener is a
    /// stopping proxy. That is true of the listener the proxy runs on
    /// loopback and false of a second one beside it: the LAN listener
    /// ([`crate::lan`]) can be turned off while the proxy carries on, and
    /// closing the shared pool on the way out would leave the loopback
    /// listener unable to open anything at all.
    ///
    /// So this says whose pool it is. The listener that owns it is the one
    /// that closes it; this one leaves it exactly as it found it.
    pub fn sharing_upstream(mut self) -> Self {
        self.owns_upstream = false;
        self
    }

    /// The same listener, accepting flows from the transparent proxy
    /// provider rather than proxy clients.
    ///
    /// Everything past the accept loop is [`crate::transparent`]'s, and it
    /// hands each flow to the same interception, rules, controls, capture and
    /// upstream pool this listener is run with — so a captured flow is traffic
    /// exactly as a request through the explicit proxy is. `own` is what the
    /// bridge refuses as the proxy talking to itself.
    pub fn transparent(mut self, own: Own) -> Self {
        self.entry = Entry::Transparent(Arc::new(own));
        self
    }

    /// The address the listener is actually bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accepts connections until `shutdown` resolves.
    ///
    /// Each accepted connection is handled in its own task, so a slow client
    /// never holds up the accept loop. Once `shutdown` resolves, no further
    /// connections are accepted and the loop waits for the connections already
    /// in flight — for [`SHUTDOWN_GRACE`], and no longer. A connection
    /// carrying a stream that does not end, such as an event stream, would
    /// otherwise mean a proxy that has been stopped and has not stopped.
    ///
    /// `interceptor` is shared by every connection: it holds the development
    /// CA, the certificates minted from it, and the configuration used to
    /// verify origin servers. Nothing in it is per-connection, so one is
    /// built when the proxy starts and handed to each connection in turn.
    ///
    /// `upstream` is shared the same way, and is the point of sharing it: it
    /// holds the connections the proxy has open to origin servers, so a
    /// connection one client's request opened is there for the next client's
    /// request to the same origin. When the accept loop is finished, and only
    /// then, its idle connections are closed.
    ///
    /// `capture` is shared the same way: it holds how much of a body is kept
    /// and the publisher every finished exchange goes to. Subscribers attach
    /// to it, not to the listener, so nothing here knows who is listening.
    ///
    /// Errors that only concern a single connection are reported and the loop
    /// continues. An error that breaks the listening socket itself is
    /// returned, because retrying it would spin forever.
    ///
    /// `shutdown` is also passed on to each connection, as a handle it can
    /// wait on. Most connections never look at it: they are over after one
    /// exchange. The ones that are not — an intercepted HTTP/2 connection,
    /// which stays open for as long as its client wants it, and a WebSocket
    /// tunnel, which stays open for as long as both ends do — are told to
    /// wind down rather than waited for, which is what keeps stopping the
    /// proxy from meaning "stop when every client has finished".
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        self,
        interceptor: Arc<Interceptor>,
        upstream: Arc<UpstreamManager>,
        capture: Capture,
        rules: Option<Arc<RuleEngine>>,
        controls: Controls,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let mut connections = JoinSet::new();
        let (signal, stopping) = Shutdown::channel();
        let owns_upstream = self.owns_upstream;
        let entry = self.entry.clone();
        tokio::pin!(shutdown);

        let outcome = loop {
            tokio::select! {
                () = &mut shutdown => {
                    signal.stop();
                    break Ok(());
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                accepted = self.inner.accept() => match accepted {
                    Ok((stream, peer)) => {
                        connections.spawn(handle_connection(
                            entry.clone(),
                            stream,
                            peer,
                            Arc::clone(&interceptor),
                            Arc::clone(&upstream),
                            capture.clone(),
                            rules.clone(),
                            controls.clone(),
                            stopping.clone(),
                        ));
                    }
                    Err(error) if is_per_connection(&error) => {
                        proxy_diagnostics::warn(
                            "listener",
                            "accept",
                            "connection",
                            &format!("failed to accept a connection: {error}"),
                        );
                    }
                    Err(error) => break Err(error),
                },
            }
        };

        // The connections already in flight, for a bounded time. Waiting for
        // them is what lets a response that is half-written finish being
        // written; waiting without end is what a connection carrying a stream
        // that has no end would turn that into. An event stream is open for
        // as long as the origin keeps writing to it, and a proxy that has
        // been told to stop cannot answer "when the origin feels like it".
        // So the wait has an end, and a connection still going at it is
        // dropped — which closes its socket, on both sides.
        if timeout(SHUTDOWN_GRACE, async {
            while connections.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            // Not an error, and not silent either: what was cut short is
            // worth knowing, and the count is all that is said about it.
            println!(
                "Stopped with {} connection(s) still open after {:?}",
                connections.len(),
                SHUTDOWN_GRACE
            );
        }

        // Last, and only once nothing this loop started is still running: an
        // idle upstream connection belongs to nobody, and holding one open
        // would keep a socket and the task driving it alive for a proxy that
        // has stopped. Connections a request is still using are untouched.
        //
        // Unless this listener is not the one the pool belongs to — see
        // [`sharing_upstream`](Self::sharing_upstream). A LAN listener being
        // turned off is not the proxy stopping, and shutting the pool down
        // there would stop the proxy that is still running on loopback.
        if owns_upstream {
            upstream.shutdown();
        }

        outcome
    }
}

/// Handles one accepted connection.
///
/// The connection is served as HTTP/1.1 and then closed. Failures concern
/// only this connection, so they are reported there and never reach the
/// accept loop.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    entry: Entry,
    stream: TcpStream,
    peer: SocketAddr,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
) {
    match entry {
        Entry::Proxy => {
            crate::http1::serve(
                stream,
                peer,
                interceptor,
                upstream,
                capture,
                rules,
                controls,
                shutdown,
            )
            .await;
        }
        Entry::Transparent(own) => {
            transparent::serve(
                stream,
                peer,
                interceptor,
                upstream,
                capture,
                rules,
                controls,
                own,
                shutdown,
            )
            .await;
        }
    }
}

/// Whether an accept error concerns only the connection being accepted.
///
/// These are caused by the client (it went away between the handshake and the
/// accept, or a signal interrupted the call), so the listening socket is still
/// healthy and the loop can carry on.
fn is_per_connection(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_tls::{CertificateAuthority, UpstreamTrust};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;
    use tokio::time::{timeout, Duration};

    /// Port `0` asks the OS for a free port, so tests never depend on the
    /// production port being available.
    const TEST_ADDR: &str = "127.0.0.1:0";

    /// Long enough to absorb a loaded machine, short enough that a listener
    /// that never accepts fails the test instead of hanging it.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// A listener running on an OS-chosen port, its address, and the sender
    /// that shuts it down.
    struct RunningListener {
        addr: SocketAddr,
        shutdown: oneshot::Sender<()>,
        task: JoinHandle<io::Result<()>>,
    }

    /// An interceptor with a CA of its own, so nothing here reads or writes
    /// the CA a developer has installed.
    fn interceptor() -> Arc<Interceptor> {
        Arc::new(
            Interceptor::new(
                CertificateAuthority::generate().expect("generate a CA"),
                &UpstreamTrust::PublicRoots,
            )
            .expect("build an interceptor"),
        )
    }

    impl RunningListener {
        async fn start() -> Self {
            let listener = Listener::bind(TEST_ADDR)
                .await
                .expect("bind ephemeral port");
            let addr = listener.local_addr();
            let (shutdown, signal) = oneshot::channel();
            let interceptor = interceptor();
            let upstream = Arc::new(UpstreamManager::new(Arc::clone(&interceptor)));
            let task = tokio::spawn(async move {
                listener
                    .run(
                        interceptor,
                        upstream,
                        Capture::with_defaults(),
                        None,
                        Controls::new(),
                        async {
                            let _ = signal.await;
                        },
                    )
                    .await
            });

            Self {
                addr,
                shutdown,
                task,
            }
        }

        /// Signals shutdown and waits for the accept loop to finish.
        async fn stop(self) -> io::Result<()> {
            let _ = self.shutdown.send(());

            timeout(TEST_TIMEOUT, self.task)
                .await
                .expect("listener shuts down")
                .expect("listener task does not panic")
        }
    }

    /// A request the proxy answers by itself, without reaching out to any
    /// origin server: the target is not the absolute URL a proxy request must
    /// carry. It keeps these tests about accepting and closing connections.
    const SELF_ANSWERED_REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: proxy.test\r\n\r\n";

    /// Sends one request and reads the whole response, up to the close.
    ///
    /// A response proves the listener accepted this connection and handed it
    /// to the HTTP layer rather than leaving it queued in the kernel backlog,
    /// and reaching end-of-stream proves the connection was closed afterwards.
    async fn exchange(addr: SocketAddr) -> String {
        let mut stream = TcpStream::connect(addr).await.expect("connect to listener");

        timeout(TEST_TIMEOUT, async {
            stream.write_all(SELF_ANSWERED_REQUEST).await?;

            let mut response = Vec::new();
            stream.read_to_end(&mut response).await?;

            io::Result::Ok(response)
        })
        .await
        .map(|response| {
            String::from_utf8(response.expect("exchange with the proxy")).expect("text response")
        })
        .expect("proxy answers and closes the connection")
    }

    #[test]
    fn default_listen_address_is_loopback_port_9090() {
        let addr: SocketAddr = DEFAULT_LISTEN_ADDR.parse().expect("valid socket address");

        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 9090);
    }

    #[tokio::test]
    async fn bind_reports_the_port_the_os_assigned() {
        let listener = Listener::bind(TEST_ADDR)
            .await
            .expect("bind ephemeral port");

        assert!(listener.local_addr().ip().is_loopback());
        assert_ne!(listener.local_addr().port(), 0);
    }

    #[tokio::test]
    async fn bind_fails_when_the_address_is_already_in_use() {
        let first = Listener::bind(TEST_ADDR)
            .await
            .expect("bind ephemeral port");
        let taken = first.local_addr().to_string();

        let error = Listener::bind(&taken).await.expect_err("port is taken");

        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn accepts_a_client_connection() {
        let listener = RunningListener::start().await;

        assert!(exchange(listener.addr).await.starts_with("HTTP/1.1 400 "));

        listener.stop().await.expect("clean shutdown");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepts_multiple_concurrent_connections() {
        let listener = RunningListener::start().await;

        // Open every client first, so all eight connections are live at the
        // same time before any of them is serviced to completion.
        let mut clients = Vec::new();
        for _ in 0..8 {
            clients.push(
                TcpStream::connect(listener.addr)
                    .await
                    .expect("connect to listener"),
            );
        }

        let exchanges = clients.into_iter().map(|mut client| {
            tokio::spawn(async move {
                client.write_all(SELF_ANSWERED_REQUEST).await?;

                let mut response = Vec::new();
                client.read_to_end(&mut response).await?;

                io::Result::Ok(response)
            })
        });

        for exchange in exchanges {
            let response = timeout(TEST_TIMEOUT, exchange)
                .await
                .expect("server answers every connection")
                .expect("client task does not panic")
                .expect("exchange with the proxy");

            assert!(response.starts_with(b"HTTP/1.1 400 "));
        }

        listener.stop().await.expect("clean shutdown");
    }

    #[tokio::test]
    async fn keeps_accepting_after_earlier_connections_close() {
        let listener = RunningListener::start().await;

        for _ in 0..3 {
            assert!(exchange(listener.addr).await.starts_with("HTTP/1.1 400 "));
        }

        listener.stop().await.expect("clean shutdown");
    }

    #[tokio::test]
    async fn stops_accepting_once_shutdown_is_signalled() {
        let listener = RunningListener::start().await;
        let addr = listener.addr;

        assert!(exchange(addr).await.starts_with("HTTP/1.1 400 "));
        listener.stop().await.expect("clean shutdown");

        let refused = timeout(TEST_TIMEOUT, TcpStream::connect(addr))
            .await
            .expect("connecting to a closed port resolves");

        assert!(refused.is_err(), "listener still accepted a connection");
    }

    #[tokio::test]
    async fn shuts_down_when_no_client_ever_connects() {
        let listener = RunningListener::start().await;

        listener.stop().await.expect("clean shutdown");
    }

    #[test]
    fn client_side_errors_do_not_stop_the_accept_loop() {
        assert!(is_per_connection(&io::Error::from(
            io::ErrorKind::ConnectionAborted
        )));
        assert!(is_per_connection(&io::Error::from(
            io::ErrorKind::ConnectionReset
        )));
        assert!(is_per_connection(&io::Error::from(
            io::ErrorKind::Interrupted
        )));
    }

    #[test]
    fn listener_socket_errors_stop_the_accept_loop() {
        assert!(!is_per_connection(&io::Error::from(
            io::ErrorKind::InvalidInput
        )));
        assert!(!is_per_connection(&io::Error::from(
            io::ErrorKind::AddrInUse
        )));
    }
}
