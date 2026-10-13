//! The Unix socket the UI connects to.
//!
//! ```text
//! bind    prepare the path, listen, restrict it to this user
//! run     accept clients, one task each, until told to stop
//! drop    remove the socket
//! ```
//!
//! # Clients are independent
//!
//! Each accepted connection gets a task of its own, so a client that is slow,
//! silent, or sending nonsense holds up nothing but itself. A connection that
//! ends — cleanly or not — is one task finishing; the accept loop does not
//! notice and no other client is affected.
//!
//! # Stopping
//!
//! Shutdown stops the accept loop and tells every connection to stop, then
//! waits for them. The signal is what makes that bounded: a connection
//! selects on it while waiting for its client and while writing to it, so a
//! client that has stopped reading cannot hold shutdown open.
//!
//! # Not a network server
//!
//! There is no TCP listener here and there is not meant to be. A Unix socket
//! is a file, which means the operating system's own file permissions decide
//! who may connect — see [`socket`](crate::socket) — and a socket bound to a
//! path cannot be reached from another machine at all.

use std::future::Future;
use std::io;
use std::path::Path;

use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::connection;
use crate::error::IpcError;
use crate::service::TrafficService;
use crate::socket::{self, IpcConfig, SocketFile};

/// The IPC server, listening on its socket.
#[derive(Debug)]
pub struct IpcServer {
    listener: UnixListener,
    socket: SocketFile,
}

impl IpcServer {
    /// Listens on the socket `config` names.
    ///
    /// Fails rather than takes over if something else is there: a socket
    /// another ProxyLab is serving is left alone, and so is a path holding
    /// something that is not a socket. A socket left behind by a process that
    /// is gone is removed and bound over.
    ///
    /// # Panics
    ///
    /// If called outside a Tokio runtime, which is where the listener
    /// registers itself.
    pub fn bind(config: &IpcConfig) -> Result<Self, IpcError> {
        socket::prepare(&config.socket)?;

        let listener = UnixListener::bind(&config.socket)
            .map_err(|cause| IpcError::bind(&config.socket, cause))?;
        // Immediately after binding, and inside a directory that was already
        // private: the window where the socket exists and is not yet
        // restricted is not one another user can reach into.
        socket::restrict(&config.socket)?;

        Ok(Self {
            listener,
            socket: SocketFile::new(config.socket.clone()),
        })
    }

    /// The socket clients connect to.
    pub fn socket_path(&self) -> &Path {
        self.socket.path()
    }

    /// Serves clients until `shutdown` resolves.
    ///
    /// Each client is handled in its own task. An error that concerns only
    /// the connection being accepted is reported and the loop carries on; one
    /// that breaks the listening socket is returned, because retrying it
    /// would spin.
    ///
    /// The socket file is removed as the server is dropped, whether it
    /// stopped cleanly or not.
    pub async fn run(
        self,
        service: TrafficService,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let (stop, stopping) = watch::channel(false);
        let mut clients = JoinSet::new();
        tokio::pin!(shutdown);

        let outcome = loop {
            tokio::select! {
                () = &mut shutdown => break Ok(()),
                Some(_) = clients.join_next(), if !clients.is_empty() => {}
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        clients.spawn(connection::serve(
                            stream,
                            service.clone(),
                            stopping.clone(),
                        ));
                    }
                    Err(error) if is_per_connection(&error) => {
                        proxy_diagnostics::warn(
                            "ipc",
                            "accept",
                            "connection",
                            &format!("failed to accept a client: {error}"),
                        );
                    }
                    Err(error) => break Err(error),
                },
            }
        };

        // Told rather than dropped: a connection waiting on a client that
        // will never speak again needs to be woken to find out it is over.
        let _ = stop.send(true);
        while clients.join_next().await.is_some() {}

        outcome
    }
}

/// Whether an accept error concerns only the connection being accepted.
///
/// The client went away between its connect and this accept, or a signal
/// interrupted the call. The listening socket is still healthy either way.
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

    #[test]
    fn a_client_going_away_does_not_stop_the_accept_loop() {
        assert!(is_per_connection(&io::Error::from(
            io::ErrorKind::ConnectionAborted
        )));
        assert!(is_per_connection(&io::Error::from(
            io::ErrorKind::Interrupted
        )));
    }

    #[test]
    fn a_broken_listening_socket_stops_the_accept_loop() {
        assert!(!is_per_connection(&io::Error::from(
            io::ErrorKind::InvalidInput
        )));
    }
}
