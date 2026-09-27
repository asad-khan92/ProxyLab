//! Where the socket lives, and making sure it is ours.
//!
//! ```text
//! ~/.proxylab/run/proxylab.sock     the socket, 0600
//! ~/.proxylab/run/                  its directory, 0700
//! ```
//!
//! `PROXYLAB_IPC_SOCKET` names a different path outright, which is what lets
//! a second instance — or a test — have a socket of its own. It is the same
//! shape as `PROXYLAB_CA_DIR` and `PROXYLAB_TRAFFIC_DIR`.
//!
//! # Binding onto a path that is already there
//!
//! A Unix socket is a file, and a file outlives the process that made it. So
//! a path that already exists is one of three things, and they are not
//! treated alike:
//!
//! ```text
//! not a socket          →  refused; something else owns that name
//! a socket, connectable →  refused; ProxyLab is already running
//! a socket, refused     →  stale; removed, and bound over
//! ```
//!
//! The middle case is why the path is connected to rather than simply
//! deleted: unlinking a live socket would leave the running instance serving
//! a socket no client can find, and would hand its clients to whichever
//! process bound the name next.
//!
//! # Permissions
//!
//! The socket is the way to every request and response ProxyLab has ever
//! captured, so it is for one user: the directory is created `0700` and the
//! socket is set to `0600`. The directory is the boundary that matters —
//! it is made private before the socket inside it exists, so there is no
//! moment when the socket is reachable and not yet restricted.

use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::IpcError;

/// The environment variable that names a different socket.
const SOCKET_VARIABLE: &str = "PROXYLAB_IPC_SOCKET";

/// ProxyLab's directory under the home directory.
const STATE_DIRECTORY: &str = ".proxylab";

/// The directory the socket lives in, under that.
const RUNTIME_DIRECTORY: &str = "run";

/// The socket's name inside it.
pub const SOCKET_FILE: &str = "proxylab.sock";

/// The mode the runtime directory is created with.
const DIRECTORY_MODE: u32 = 0o700;

/// The mode the socket is set to once it exists.
const SOCKET_MODE: u32 = 0o600;

/// The longest socket path the operating system will accept.
///
/// A Unix socket address is a fixed-size buffer — 104 bytes on macOS — and a
/// path that does not fit produces an error from `bind` that says nothing
/// about why. Checking first turns that into something a developer can act
/// on.
pub const MAX_SOCKET_PATH_LENGTH: usize = 103;

/// Where the IPC server listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpcConfig {
    /// The socket to bind.
    pub socket: PathBuf,
}

impl IpcConfig {
    /// A server listening on `socket`.
    pub fn at(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// The production location: `~/.proxylab/run/proxylab.sock`, or wherever
    /// `PROXYLAB_IPC_SOCKET` points.
    pub fn default_location() -> Result<Self, IpcError> {
        Ok(Self::at(Self::default_socket()?))
    }

    /// The path the production location resolves to.
    pub fn default_socket() -> Result<PathBuf, IpcError> {
        if let Some(socket) = env::var_os(SOCKET_VARIABLE) {
            if !socket.is_empty() {
                return Ok(PathBuf::from(socket));
            }
        }

        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .filter(|home| !home.is_empty())
            .ok_or(IpcError::NoHomeDirectory)?;

        Ok(PathBuf::from(home)
            .join(STATE_DIRECTORY)
            .join(RUNTIME_DIRECTORY)
            .join(SOCKET_FILE))
    }
}

/// Makes the path ready to be bound, or says why it cannot be.
pub(crate) fn prepare(socket: &Path) -> Result<(), IpcError> {
    if socket.as_os_str().len() > MAX_SOCKET_PATH_LENGTH {
        return Err(IpcError::SocketPathTooLong {
            path: socket.to_path_buf(),
            limit: MAX_SOCKET_PATH_LENGTH,
        });
    }

    if let Some(directory) = socket.parent() {
        if !directory.as_os_str().is_empty() {
            fs::create_dir_all(directory).map_err(|cause| IpcError::directory(directory, cause))?;
            // Set every time rather than only on creation: a directory that
            // was made loose by something else is one this would otherwise
            // quietly keep using.
            fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE))
                .map_err(|cause| IpcError::permissions(directory, cause))?;
        }
    }

    clear_stale(socket)
}

/// Removes a socket left behind by a process that is gone.
///
/// Refuses to remove anything else: a live socket, or a path that is not a
/// socket at all.
fn clear_stale(socket: &Path) -> Result<(), IpcError> {
    let metadata = match fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(cause) => return Err(IpcError::bind(socket, cause)),
    };

    if !metadata.file_type().is_socket() {
        return Err(IpcError::PathOccupied {
            path: socket.to_path_buf(),
        });
    }

    // The only way to tell a stale socket from a live one is to try it. A
    // connection that is refused means nothing is listening; one that
    // succeeds means something is, and that something is not to be unlinked
    // out from under its clients.
    match std::os::unix::net::UnixStream::connect(socket) {
        Ok(_) => Err(IpcError::AlreadyRunning {
            path: socket.to_path_buf(),
        }),
        Err(cause) if is_unattended(&cause) => {
            fs::remove_file(socket).map_err(|cause| IpcError::bind(socket, cause))
        }
        Err(cause) => Err(IpcError::bind(socket, cause)),
    }
}

/// Whether a failure to connect means nothing is listening on the socket.
fn is_unattended(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

/// Restricts a bound socket to the user that made it.
pub(crate) fn restrict(socket: &Path) -> Result<(), IpcError> {
    fs::set_permissions(socket, fs::Permissions::from_mode(SOCKET_MODE))
        .map_err(|cause| IpcError::permissions(socket, cause))
}

/// A bound socket's file, removed when the server is done with it.
///
/// Cleanup on the way out is what keeps the next run from finding a stale
/// socket, and it happens on drop so that it also covers a server that is
/// dropped without ever being run.
#[derive(Debug)]
pub(crate) struct SocketFile {
    path: PathBuf,
}

impl SocketFile {
    /// Takes responsibility for the socket at `path`.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where it is.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        // Best effort, and only if it is still a socket: a path that has
        // become something else in the meantime belongs to whatever put it
        // there.
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_socket() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn socket_in(directory: &TempDir) -> PathBuf {
        directory.path().join(SOCKET_FILE)
    }

    #[test]
    fn the_default_socket_is_under_the_home_directory() {
        // The variable is read, not written: writing it would race with
        // every other test in this binary.
        if env::var_os(SOCKET_VARIABLE).is_some() {
            return;
        }

        let socket = IpcConfig::default_socket().expect("a home directory");

        assert!(socket.ends_with(
            Path::new(STATE_DIRECTORY)
                .join(RUNTIME_DIRECTORY)
                .join(SOCKET_FILE)
        ));
        assert!(socket.is_absolute(), "{socket:?}");
    }

    #[test]
    fn a_socket_can_be_named_outright() {
        let config = IpcConfig::at("/tmp/proxylab-test.sock");

        assert_eq!(config.socket, Path::new("/tmp/proxylab-test.sock"));
    }

    #[test]
    fn preparing_makes_the_directory_private() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = directory.path().join("run").join(SOCKET_FILE);

        prepare(&socket).expect("prepares");

        let mode = fs::metadata(socket.parent().expect("a parent"))
            .expect("the directory is there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, DIRECTORY_MODE);
    }

    #[test]
    fn preparing_a_path_that_is_free_does_nothing_to_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);

        prepare(&socket).expect("prepares");

        assert!(!socket.exists(), "something was created at the socket path");
    }

    #[test]
    fn a_path_held_by_something_that_is_not_a_socket_is_refused() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        fs::write(&socket, b"not a socket").expect("a file in the way");

        let error = prepare(&socket).expect_err("that path is taken");

        assert!(matches!(error, IpcError::PathOccupied { .. }), "{error:?}");
        // Refused, not removed: whatever is there belongs to someone else.
        assert!(socket.is_file());
    }

    #[test]
    fn a_path_too_long_for_the_operating_system_is_refused_with_a_reason() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = directory
            .path()
            .join("x".repeat(MAX_SOCKET_PATH_LENGTH + 1));

        let error = prepare(&socket).expect_err("that path cannot be bound");

        assert!(
            matches!(error, IpcError::SocketPathTooLong { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_stale_socket_is_removed() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        // A socket whose listener is gone, which is what a crash leaves.
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");
        drop(listener);
        assert!(socket.exists());

        prepare(&socket).expect("prepares");

        assert!(!socket.exists(), "a stale socket was left behind");
    }

    #[test]
    fn a_live_socket_is_refused_rather_than_removed() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");

        let error = prepare(&socket).expect_err("something is already listening");

        assert!(
            matches!(error, IpcError::AlreadyRunning { .. }),
            "{error:?}"
        );
        assert!(socket.exists(), "a live socket was unlinked");
    }

    #[test]
    fn a_bound_socket_is_restricted_to_its_owner() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");

        restrict(&socket).expect("restricts");

        let mode = fs::metadata(&socket)
            .expect("the socket is there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, SOCKET_MODE);
    }

    #[test]
    fn a_socket_file_removes_its_socket_when_it_is_dropped() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("binds");

        let file = SocketFile::new(socket.clone());
        assert_eq!(file.path(), socket);
        drop(file);

        assert!(!socket.exists(), "the socket outlived the server");
    }

    #[test]
    fn a_socket_file_leaves_alone_a_path_that_has_become_something_else() {
        let directory = TempDir::new().expect("a temporary directory");
        let socket = socket_in(&directory);
        let file = SocketFile::new(socket.clone());
        fs::write(&socket, b"someone else's file").expect("a file in its place");

        drop(file);

        assert!(socket.is_file(), "another process's file was removed");
    }

    #[test]
    fn a_socket_file_for_a_path_that_is_gone_is_not_a_problem() {
        let directory = TempDir::new().expect("a temporary directory");

        drop(SocketFile::new(socket_in(&directory)));
    }
}
