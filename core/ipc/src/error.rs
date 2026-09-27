//! Why the IPC server could not start.
//!
//! These are the server's own failures — a socket that cannot be bound, a
//! path that is taken — and not a client's. A client's mistakes are answered
//! over the protocol, in [`ErrorBody`](crate::protocol::ErrorBody); nothing
//! here ever reaches a client, because a client that cannot connect has
//! nowhere to be told anything.
//!
//! Every message names a path and a cause, and nothing else. There is no
//! traffic at this point in the server's life, so there is nothing sensitive
//! for one of these to carry.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Why the IPC server is not listening.
#[derive(Debug)]
pub enum IpcError {
    /// There is no home directory to work out the default socket from.
    NoHomeDirectory,
    /// The socket path is longer than the operating system will accept.
    SocketPathTooLong {
        /// The path that is too long.
        path: PathBuf,
        /// The longest path that would have worked.
        limit: usize,
    },
    /// Something that is not a socket is at the socket's path.
    PathOccupied {
        /// The path that is taken.
        path: PathBuf,
    },
    /// A ProxyLab is already listening on that socket.
    AlreadyRunning {
        /// The socket it is listening on.
        path: PathBuf,
    },
    /// The directory the socket lives in could not be prepared.
    Directory {
        /// The directory.
        path: PathBuf,
        /// Why not.
        cause: io::Error,
    },
    /// The socket's permissions could not be set.
    Permissions {
        /// What was being restricted.
        path: PathBuf,
        /// Why it could not be.
        cause: io::Error,
    },
    /// The socket could not be bound.
    Bind {
        /// The socket.
        path: PathBuf,
        /// Why not.
        cause: io::Error,
    },
}

impl IpcError {
    /// A directory failure for `path`.
    pub(crate) fn directory(path: impl AsRef<Path>, cause: io::Error) -> Self {
        Self::Directory {
            path: path.as_ref().to_path_buf(),
            cause,
        }
    }

    /// A permissions failure for `path`.
    pub(crate) fn permissions(path: impl AsRef<Path>, cause: io::Error) -> Self {
        Self::Permissions {
            path: path.as_ref().to_path_buf(),
            cause,
        }
    }

    /// A bind failure for `path`.
    pub(crate) fn bind(path: impl AsRef<Path>, cause: io::Error) -> Self {
        Self::Bind {
            path: path.as_ref().to_path_buf(),
            cause,
        }
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHomeDirectory => f.write_str(
                "no home directory to put the IPC socket in: neither HOME nor USERPROFILE is set",
            ),
            Self::SocketPathTooLong { path, limit } => write!(
                f,
                "the socket path is {} characters and the operating system accepts at most {limit}: {}",
                path.as_os_str().len(),
                path.display(),
            ),
            Self::PathOccupied { path } => write!(
                f,
                "something that is not a socket is already at {}",
                path.display()
            ),
            Self::AlreadyRunning { path } => write!(
                f,
                "another ProxyLab is already listening on {}",
                path.display()
            ),
            Self::Directory { path, cause } => {
                write!(f, "could not prepare {}: {cause}", path.display())
            }
            Self::Permissions { path, cause } => write!(
                f,
                "could not restrict {} to this user: {cause}",
                path.display()
            ),
            Self::Bind { path, cause } => {
                write!(f, "could not listen on {}: {cause}", path.display())
            }
        }
    }
}

impl std::error::Error for IpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory { cause, .. }
            | Self::Permissions { cause, .. }
            | Self::Bind { cause, .. } => Some(cause),
            Self::NoHomeDirectory
            | Self::SocketPathTooLong { .. }
            | Self::PathOccupied { .. }
            | Self::AlreadyRunning { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error as _;

    fn every_error() -> Vec<IpcError> {
        vec![
            IpcError::NoHomeDirectory,
            IpcError::SocketPathTooLong {
                path: PathBuf::from("/tmp/a-very-long-path.sock"),
                limit: 103,
            },
            IpcError::PathOccupied {
                path: PathBuf::from("/tmp/proxylab.sock"),
            },
            IpcError::AlreadyRunning {
                path: PathBuf::from("/tmp/proxylab.sock"),
            },
            IpcError::directory("/tmp/run", io::Error::from(io::ErrorKind::PermissionDenied)),
            IpcError::permissions(
                "/tmp/proxylab.sock",
                io::Error::from(io::ErrorKind::PermissionDenied),
            ),
            IpcError::bind(
                "/tmp/proxylab.sock",
                io::Error::from(io::ErrorKind::AddrInUse),
            ),
        ]
    }

    #[test]
    fn every_failure_says_something() {
        for error in every_error() {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }

    #[test]
    fn a_failure_that_has_a_cause_carries_it() {
        assert!(
            IpcError::bind("/tmp/x.sock", io::Error::from(io::ErrorKind::AddrInUse))
                .source()
                .is_some()
        );
        assert!(IpcError::NoHomeDirectory.source().is_none());
    }

    #[test]
    fn an_already_running_proxylab_is_named_as_one() {
        let message = IpcError::AlreadyRunning {
            path: PathBuf::from("/tmp/proxylab.sock"),
        }
        .to_string();

        assert!(message.contains("already listening"), "{message}");
        assert!(message.contains("/tmp/proxylab.sock"), "{message}");
    }
}
