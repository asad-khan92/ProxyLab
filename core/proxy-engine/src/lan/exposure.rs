//! The second listener, and the switch that turns it on.
//!
//! ```text
//!   127.0.0.1:9090 ─────────┐
//!                           ├──▶ the one pipeline:
//!   192.168.1.42:9090 ──────┘    rules, TLS, capture, storage, events
//!   (only while LAN mode is on)
//! ```
//!
//! Turning LAN exposure on binds a second [`Listener`] and runs it with the
//! same interceptor, the same upstream connections, the same capture, and the
//! same rule engine the loopback listener is using. That is the whole of the
//! feature: an iPhone's request is a request like any other by the time it
//! reaches [`crate::http1`], so there is no iPhone-shaped code anywhere past
//! this module, no second capture, and no second way for traffic to be
//! stored.
//!
//! # Off, until a person says otherwise
//!
//! A [`LanExposure`] that has just been built is not listening on anything.
//! Nothing constructs one enabled, nothing reads a setting that would, and
//! nothing writes one down: a restarted ProxyLab is a ProxyLab only this Mac
//! can reach, every time, whatever was on before.
//!
//! # One address, chosen, never `0.0.0.0`
//!
//! The listener is bound to a single address that a person picked from what
//! [`discovery`](super::discovery) offers. Binding the unspecified address
//! would put the proxy on every interface this Mac has, including ones the
//! developer was not thinking about, and it is never done here — there is no
//! parameter that would express it and an address that is not one of this
//! machine's current usable ones is refused.
//!
//! # The loopback listener is untouched
//!
//! It is bound before any of this exists, it is never rebound, and it is
//! never stopped by anything here. Turning LAN exposure on adds a listener;
//! turning it off removes that one and no other.

use std::net::Ipv4Addr;
use std::sync::Arc;

use proxy_ipc::{LanAddress, LanControl, LanFuture, LanRefusal, LanStatus};
use proxy_tls::Interceptor;
use rules::RuleEngine;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use super::discovery;
use crate::capture::Capture;
use crate::controls::Controls;
use crate::listener::Listener;
use crate::upstream::UpstreamManager;

/// The proxy's optional presence on the local network.
///
/// One of these exists per running proxy. It holds everything a second
/// listener would need and, until it is asked, uses none of it.
#[derive(Debug)]
pub struct LanExposure {
    port: u16,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    /// The breakpoints and the throttling the proxy is running with.
    ///
    /// Shared with the loopback listener, not copied: a breakpoint a
    /// developer set applies to their phone's traffic exactly as it applies
    /// to their browser's, and so does a bandwidth ceiling.
    controls: Controls,
    /// The signal that stops the whole proxy.
    ///
    /// The LAN listener waits on this as well as on its own, so stopping
    /// ProxyLab stops it too — without the process having to remember to.
    stopping: watch::Receiver<bool>,
    /// The listener there is, if there is one.
    ///
    /// An async mutex because enabling holds it across a bind and disabling
    /// holds it across a join, and both are awaits. Held only by these
    /// operations, which a person triggers one at a time.
    running: Mutex<Option<Running>>,
}

/// A LAN listener that is up.
#[derive(Debug)]
struct Running {
    address: Ipv4Addr,
    /// Closes this listener and nothing else.
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl LanExposure {
    /// Something that could put the proxy on the local network, and has not.
    ///
    /// `port` is the port the proxy is already listening on, so the LAN
    /// listener is reachable at the port the developer has been told about
    /// rather than at one of its own.
    ///
    /// Everything else is what the loopback listener was given: the same
    /// values, shared, not copies. That is what makes an iPhone's traffic
    /// the same traffic.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        port: u16,
        interceptor: Arc<Interceptor>,
        upstream: Arc<UpstreamManager>,
        capture: Capture,
        rules: Option<Arc<RuleEngine>>,
        controls: Controls,
        stopping: watch::Receiver<bool>,
    ) -> Self {
        Self {
            port,
            interceptor,
            upstream,
            capture,
            rules,
            controls,
            stopping,
            running: Mutex::new(None),
        }
    }

    /// Stops the LAN listener, if there is one, and says nothing about it.
    ///
    /// For the process shutting down, where the listener going away is not
    /// news. [`LanControl::disable`] is the same thing said out loud.
    pub async fn shutdown(&self) {
        let running = self.running.lock().await.take();

        if let Some(running) = running {
            stop(running).await;
        }
    }

    /// The status, given what is running.
    fn describe(&self, running: Option<&Running>) -> LanStatus {
        LanStatus {
            enabled: running.is_some(),
            address: running.map(|running| running.address.to_string()),
            port: self.port,
            available: discovery::usable(),
        }
    }

    /// Binds a listener on `address` and runs it until it is told to stop.
    async fn start(&self, address: Ipv4Addr) -> Result<Running, LanRefusal> {
        let listener = Listener::bind(&format!("{address}:{}", self.port))
            .await
            .map_err(|error| {
                // The address is the developer's own network position and is
                // already on their screen; what is worth writing down is
                // that the bind failed and what the operating system said.
                eprintln!("Failed to listen on the local network: {error}");

                LanRefusal::BindFailed
            })?
            // The pool belongs to the loopback listener, which outlives this
            // one. See `Listener::sharing_upstream`.
            .sharing_upstream();

        let (stop, stopped) = watch::channel(false);
        let interceptor = Arc::clone(&self.interceptor);
        let upstream = Arc::clone(&self.upstream);
        let capture = self.capture.clone();
        let rules = self.rules.clone();
        let controls = self.controls.clone();
        let process = self.stopping.clone();

        let task = tokio::spawn(async move {
            let outcome = listener
                .run(
                    interceptor,
                    upstream,
                    capture,
                    rules,
                    controls,
                    async move {
                        // Either signal ends it: the switch, or the process.
                        tokio::select! {
                            () = signalled(stopped) => {}
                            () = signalled(process) => {}
                        }
                    },
                )
                .await;

            if let Err(error) = outcome {
                eprintln!("The local network listener stopped: {error}");
            }
        });

        Ok(Running {
            address,
            stop,
            task,
        })
    }
}

impl LanControl for LanExposure {
    fn status(&self) -> LanFuture<'_, LanStatus> {
        Box::pin(async move {
            let running = self.running.lock().await;

            self.describe(running.as_ref())
        })
    }

    fn enable(&self, address: String) -> LanFuture<'_, Result<LanStatus, LanRefusal>> {
        Box::pin(async move {
            let address = discovery::parse(&address).ok_or(LanRefusal::NotAnAddress)?;

            // Asked of the machine now rather than taken on the client's
            // word: this is what keeps `enable_lan` from being a way to bind
            // the unspecified address, a loopback one, or a routable one.
            if !discovery::is_usable(address) {
                return Err(LanRefusal::NotAvailable);
            }

            let mut running = self.running.lock().await;

            // Already there. Enabling twice is not an error and does not
            // rebind: a socket closed and reopened would drop whatever was
            // going through it for no reason.
            if running.as_ref().is_some_and(|it| it.address == address) {
                return Ok(self.describe(running.as_ref()));
            }

            // Moving to another address: the old listener goes first, so the
            // proxy is never on two LAN addresses at once — which is what
            // makes "the proxy is on 192.168.1.42" a true sentence.
            if let Some(previous) = running.take() {
                stop(previous).await;
            }

            *running = Some(self.start(address).await?);

            Ok(self.describe(running.as_ref()))
        })
    }

    fn disable(&self) -> LanFuture<'_, LanStatus> {
        Box::pin(async move {
            let mut running = self.running.lock().await;

            // Awaited, not signalled and forgotten: a client that has been
            // told the proxy is off the local network can rely on the
            // listening socket being closed by the time it is told.
            if let Some(previous) = running.take() {
                stop(previous).await;
            }

            self.describe(running.as_ref())
        })
    }
}

/// Stops one LAN listener and waits for it to be gone.
async fn stop(running: Running) {
    // A receiver that is gone is a listener that has already ended, which is
    // the outcome being asked for.
    let _ = running.stop.send(true);

    // A panic in the listener task is a bug in this process, and waiting for
    // a task that panicked is not a reason to refuse to turn LAN exposure
    // off: either way, the socket is closed.
    let _ = running.task.await;
}

/// Resolves once `signal` says to stop.
async fn signalled(mut signal: watch::Receiver<bool>) {
    while !*signal.borrow() {
        // A sender that is gone is a proxy that is ending, which is a reason
        // to stop rather than to keep waiting.
        if signal.changed().await.is_err() {
            return;
        }
    }
}

/// The line reporting where the proxy can be reached from.
///
/// Printed when the proxy starts, so a run says for itself that it is on
/// loopback and nothing else — which is the thing a person would otherwise
/// have to take on trust.
pub fn startup_report(available: &[LanAddress]) -> String {
    match available.len() {
        0 => "Local network: off (no usable address on this machine)".to_owned(),
        1 => "Local network: off (1 address available)".to_owned(),
        count => format!("Local network: off ({count} addresses available)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;
    use std::net::SocketAddr;
    use std::time::Duration;

    use proxy_tls::{CertificateAuthority, UpstreamTrust};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    /// Long enough to absorb a loaded machine, short enough that something
    /// that never happens fails the test instead of hanging it.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// A request the proxy answers by itself, without reaching any origin.
    const SELF_ANSWERED_REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: proxy.test\r\n\r\n";

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

    /// An exposure over a port nothing is using, and the sender that would
    /// stop the whole proxy.
    fn exposure(port: u16) -> (LanExposure, watch::Sender<bool>) {
        let interceptor = interceptor();
        let upstream = Arc::new(UpstreamManager::new(Arc::clone(&interceptor)));
        let (stopping, signal) = watch::channel(false);

        (
            LanExposure::new(
                port,
                interceptor,
                upstream,
                Capture::with_defaults(),
                None,
                Controls::new(),
                signal,
            ),
            stopping,
        )
    }

    /// A port the operating system just handed out and has now released.
    ///
    /// Not a guarantee — another process could take it in between — but it
    /// is a port nothing in this suite is using, which is what these tests
    /// need. Binding a fixed port would fail whenever a developer's own
    /// ProxyLab was running.
    async fn free_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");

        listener.local_addr().expect("a bound address").port()
    }

    /// One address of this machine a LAN listener could use, if there is one.
    fn an_address() -> Option<Ipv4Addr> {
        discovery::usable()
            .first()
            .and_then(|address| discovery::parse(&address.address))
    }

    /// Sends one request to `addr` and reads the whole answer.
    async fn exchange(addr: SocketAddr) -> io::Result<String> {
        timeout(TEST_TIMEOUT, async {
            let mut stream = TcpStream::connect(addr).await?;
            stream.write_all(SELF_ANSWERED_REQUEST).await?;

            let mut response = Vec::new();
            stream.read_to_end(&mut response).await?;

            io::Result::Ok(String::from_utf8_lossy(&response).into_owned())
        })
        .await
        .expect("the proxy answers and closes the connection")
    }

    #[tokio::test]
    async fn a_new_exposure_is_not_listening_on_anything() {
        let (exposure, _stopping) = exposure(free_port().await);

        let status = exposure.status().await;

        assert!(!status.enabled);
        assert_eq!(status.address, None);
    }

    #[tokio::test]
    async fn the_status_names_the_port_the_proxy_is_on_whether_or_not_it_is_enabled() {
        let port = free_port().await;
        let (exposure, _stopping) = exposure(port);

        assert_eq!(exposure.status().await.port, port);
    }

    #[tokio::test]
    async fn something_that_is_not_an_address_is_refused() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert_eq!(
            exposure.enable("not an address".to_owned()).await,
            Err(LanRefusal::NotAnAddress)
        );
        assert!(!exposure.status().await.enabled);
    }

    #[tokio::test]
    async fn loopback_is_not_an_address_lan_mode_will_bind() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert_eq!(
            exposure.enable("127.0.0.1".to_owned()).await,
            Err(LanRefusal::NotAvailable)
        );
    }

    #[tokio::test]
    async fn the_unspecified_address_is_not_one_either() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert_eq!(
            exposure.enable("0.0.0.0".to_owned()).await,
            Err(LanRefusal::NotAvailable)
        );
    }

    #[tokio::test]
    async fn an_address_this_machine_does_not_have_is_refused() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert_eq!(
            exposure.enable("10.255.255.254".to_owned()).await,
            Err(LanRefusal::NotAvailable)
        );
    }

    #[tokio::test]
    async fn a_routable_address_is_refused() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert_eq!(
            exposure.enable("93.184.216.34".to_owned()).await,
            Err(LanRefusal::NotAvailable)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_chosen_address_is_the_one_that_is_bound() {
        let Some(address) = an_address() else { return };
        let port = free_port().await;
        let (exposure, _stopping) = exposure(port);

        let status = exposure
            .enable(address.to_string())
            .await
            .expect("this machine's own address binds");

        assert!(status.enabled);
        assert_eq!(status.address, Some(address.to_string()));
        assert_eq!(status.port, port);

        // And it is the proxy that is there, answering as the proxy does.
        let response = exchange(SocketAddr::from((address, port)))
            .await
            .expect("the LAN listener answers");
        assert!(response.starts_with("HTTP/1.1 400 "), "{response}");

        exposure.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn enabling_twice_on_the_same_address_changes_nothing() {
        let Some(address) = an_address() else { return };
        let port = free_port().await;
        let (exposure, _stopping) = exposure(port);

        exposure
            .enable(address.to_string())
            .await
            .expect("this machine's own address binds");
        let again = exposure
            .enable(address.to_string())
            .await
            .expect("enabling an address it is already on is not an error");

        assert!(again.enabled);
        assert_eq!(again.address, Some(address.to_string()));

        let response = exchange(SocketAddr::from((address, port)))
            .await
            .expect("the LAN listener is still there");
        assert!(response.starts_with("HTTP/1.1 400 "), "{response}");

        exposure.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn disabling_closes_the_listening_socket() {
        let Some(address) = an_address() else { return };
        let port = free_port().await;
        let (exposure, _stopping) = exposure(port);

        exposure
            .enable(address.to_string())
            .await
            .expect("this machine's own address binds");
        let status = exposure.disable().await;

        assert!(!status.enabled);
        assert_eq!(status.address, None);
        assert!(
            exchange(SocketAddr::from((address, port))).await.is_err(),
            "the LAN listener was still reachable after being disabled"
        );
    }

    #[tokio::test]
    async fn disabling_something_that_was_never_enabled_is_not_an_error() {
        let (exposure, _stopping) = exposure(free_port().await);

        assert!(!exposure.disable().await.enabled);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_port_already_taken_fails_cleanly() {
        let Some(address) = an_address() else { return };

        // Held for the whole test, so the bind the exposure attempts is
        // against a port that is genuinely in use.
        let taken = tokio::net::TcpListener::bind((address, 0))
            .await
            .expect("bind an ephemeral port on this machine's own address");
        let port = taken.local_addr().expect("a bound address").port();
        let (exposure, _stopping) = exposure(port);

        assert_eq!(
            exposure.enable(address.to_string()).await,
            Err(LanRefusal::BindFailed)
        );
        assert!(!exposure.status().await.enabled);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stopping_the_proxy_stops_the_lan_listener() {
        let Some(address) = an_address() else { return };
        let port = free_port().await;
        let (exposure, stopping) = exposure(port);

        exposure
            .enable(address.to_string())
            .await
            .expect("this machine's own address binds");
        let _ = stopping.send(true);
        exposure.shutdown().await;

        assert!(
            exchange(SocketAddr::from((address, port))).await.is_err(),
            "the LAN listener outlived the proxy"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn turning_lan_mode_off_leaves_the_shared_upstream_connections_alone() {
        let Some(address) = an_address() else { return };
        let interceptor = interceptor();
        let upstream = Arc::new(UpstreamManager::new(Arc::clone(&interceptor)));
        let (_stopping, signal) = watch::channel(false);
        let exposure = LanExposure::new(
            free_port().await,
            interceptor,
            Arc::clone(&upstream),
            Capture::with_defaults(),
            None,
            Controls::new(),
            signal,
        );

        exposure
            .enable(address.to_string())
            .await
            .expect("this machine's own address binds");
        exposure.disable().await;

        // The loopback listener is still running and still needs the pool.
        assert!(
            !upstream.is_stopping(),
            "disabling LAN mode shut down the proxy's upstream connections"
        );
    }

    #[test]
    fn the_startup_report_says_lan_mode_is_off() {
        assert!(startup_report(&[]).contains("off"));
        assert!(startup_report(&[LanAddress {
            interface: "en0".to_owned(),
            address: "192.168.1.42".to_owned(),
        }])
        .contains("off"));
    }

    #[test]
    fn the_startup_report_never_names_an_address() {
        let report = startup_report(&[LanAddress {
            interface: "en0".to_owned(),
            address: "192.168.1.42".to_owned(),
        }]);

        assert!(!report.contains("192.168.1.42"), "{report}");
        assert!(!report.contains("en0"), "{report}");
    }
}
