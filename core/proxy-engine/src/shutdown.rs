//! Telling a long-lived connection that the proxy is stopping.
//!
//! Most of what the proxy holds open is short: a plaintext request is
//! answered and the connection closes, and so is a request inside an
//! intercepted HTTP/1.1 session. The accept loop simply waits for those.
//!
//! An HTTP/2 connection is not like that. It is opened once and kept, and the
//! client decides when it is finished with it — which may be never. Waiting
//! for one would be waiting for the client, so instead it is told: when the
//! listener stops accepting, every connection holding one of these is asked
//! to wind down, and an HTTP/2 connection answers by finishing the streams it
//! has and refusing new ones.
//!
//! ```text
//! Ctrl+C ──▶ listener stops accepting
//!                   │
//!                   └──▶ Shutdown ──▶ HTTP/2 connections wind down
//! ```
//!
//! [`Shutdown`] is the receiving half and is cheap to clone: one is made when
//! the listener starts and a clone goes to each connection. Nothing here
//! decides what winding down means — that belongs to whatever is holding the
//! connection.

use std::fmt;
use std::future::pending;

use tokio::sync::watch;

/// Tells the connections holding it that the proxy is stopping.
///
/// A handle from [`Shutdown::never`] is never signalled, which is what a
/// caller with nothing to signal it hands over — a single exchange run
/// directly, or a test that is not about shutting down.
#[derive(Clone, Debug)]
pub struct Shutdown(Option<watch::Receiver<bool>>);

/// The half that signals. Dropping it signals too: a sender that is gone is a
/// proxy that is ending.
pub struct ShutdownSignal(watch::Sender<bool>);

impl Shutdown {
    /// A signal and the handle that hears it.
    pub fn channel() -> (ShutdownSignal, Self) {
        let (sender, receiver) = watch::channel(false);

        (ShutdownSignal(sender), Self(Some(receiver)))
    }

    /// A handle that is never signalled.
    pub fn never() -> Self {
        Self(None)
    }

    /// Resolves once the proxy is stopping.
    ///
    /// A handle whose signal has been dropped resolves too, because the only
    /// thing that drops one is the accept loop returning.
    pub async fn signalled(&self) {
        let Some(receiver) = &self.0 else {
            return pending().await;
        };

        let mut receiver = receiver.clone();
        let _ = receiver.wait_for(|stopping| *stopping).await;
    }
}

impl ShutdownSignal {
    /// Tells every handle that the proxy is stopping.
    pub fn stop(&self) {
        let _ = self.0.send(true);
    }
}

/// Says whether it has been signalled, and nothing else: there is nothing
/// else in it.
impl fmt::Debug for ShutdownSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownSignal")
            .field("stopping", &*self.0.borrow())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::time::{timeout, Duration};

    /// Long enough that a handle which should resolve gets the chance, short
    /// enough that one which should not does not hold the suite up.
    const MOMENT: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn a_handle_waits_until_it_is_signalled() {
        let (signal, shutdown) = Shutdown::channel();

        assert!(timeout(MOMENT, shutdown.signalled()).await.is_err());

        signal.stop();

        timeout(MOMENT, shutdown.signalled())
            .await
            .expect("a signalled handle resolves");
    }

    #[tokio::test]
    async fn a_handle_signalled_before_it_is_waited_on_resolves_at_once() {
        let (signal, shutdown) = Shutdown::channel();
        signal.stop();

        timeout(MOMENT, shutdown.signalled())
            .await
            .expect("the signal is not missed");
    }

    #[tokio::test]
    async fn every_clone_hears_the_same_signal() {
        let (signal, shutdown) = Shutdown::channel();
        let clones: Vec<Shutdown> = (0..3).map(|_| shutdown.clone()).collect();

        signal.stop();

        for clone in &clones {
            timeout(MOMENT, clone.signalled())
                .await
                .expect("every clone resolves");
        }
    }

    #[tokio::test]
    async fn a_signal_that_is_dropped_is_a_proxy_that_has_ended() {
        let (signal, shutdown) = Shutdown::channel();

        drop(signal);

        timeout(MOMENT, shutdown.signalled())
            .await
            .expect("a dropped signal resolves the handle");
    }

    #[tokio::test]
    async fn a_handle_that_is_never_signalled_never_resolves() {
        let shutdown = Shutdown::never();

        assert!(timeout(MOMENT, shutdown.signalled()).await.is_err());
    }
}
