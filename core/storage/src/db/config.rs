//! Where the traffic database and its body files live.
//!
//! Nothing here is hard-coded to a machine. A [`StorageConfig`] names the
//! database file and the directory the body files go in, so the proxy stores
//! traffic under the developer's home directory and a test stores it in a
//! temporary directory that is deleted with the test.
//!
//! ```text
//! Production   ~/.proxylab/traffic/traffic.db
//!              ~/.proxylab/traffic/bodies/
//!
//! Tests        <temporary directory>/traffic.db
//!              <temporary directory>/bodies/
//! ```
//!
//! `PROXYLAB_TRAFFIC_DIR` overrides the production location, which is how a
//! second instance — or a developer who keeps captured traffic somewhere
//! else — is pointed elsewhere without a rebuild. It is the same shape as
//! `PROXYLAB_CA_DIR`, which the TLS crate reads for the development CA.

use std::env;
use std::path::{Path, PathBuf};

use super::error::StorageError;

/// The environment variable that overrides where traffic is stored.
const TRAFFIC_DIRECTORY_VARIABLE: &str = "PROXYLAB_TRAFFIC_DIR";

/// ProxyLab's directory under the home directory.
const STATE_DIRECTORY: &str = ".proxylab";

/// The captured-traffic directory under that.
const TRAFFIC_DIRECTORY: &str = "traffic";

/// The database file's name inside a traffic directory.
pub const DATABASE_FILE: &str = "traffic.db";

/// The body-file directory's name inside a traffic directory.
pub const BODIES_DIRECTORY: &str = "bodies";

/// How large a captured body may be and still be kept in the database.
///
/// SQLite is happy to hold a blob this size and reading one back is a single
/// row read, which is what makes a small body cheap to show. Past this the
/// bytes go to a file of their own: a database that holds every captured
/// image and archive grows into something that has to be vacuumed, and the
/// row it is attached to becomes expensive to scan past. 64 KiB covers the
/// API responses a debugging session is mostly made of.
pub const DEFAULT_INLINE_BODY_MAX_BYTES: usize = 64 * 1024;

/// Where a [`Storage`](super::Storage) keeps what it is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageConfig {
    /// The SQLite database file. Created if it is not there.
    pub database: PathBuf,
    /// The directory captured bodies too large to inline are written to.
    /// Created if it is not there.
    pub bodies: PathBuf,
    /// The largest captured body kept in the database rather than in a file.
    ///
    /// Zero puts every non-empty captured body in a file.
    pub inline_body_max_bytes: usize,
}

impl StorageConfig {
    /// A database and a body directory of ProxyLab's own naming, under
    /// `directory`.
    pub fn in_directory(directory: impl AsRef<Path>) -> Self {
        let directory = directory.as_ref();

        Self {
            database: directory.join(DATABASE_FILE),
            bodies: directory.join(BODIES_DIRECTORY),
            inline_body_max_bytes: DEFAULT_INLINE_BODY_MAX_BYTES,
        }
    }

    /// A database at `database` and body files in `bodies`.
    ///
    /// The two are named separately because they are two different things: a
    /// database file and a directory of body files. Most callers want
    /// [`in_directory`](Self::in_directory), which puts both in one place.
    pub fn new(database: impl Into<PathBuf>, bodies: impl Into<PathBuf>) -> Self {
        Self {
            database: database.into(),
            bodies: bodies.into(),
            inline_body_max_bytes: DEFAULT_INLINE_BODY_MAX_BYTES,
        }
    }

    /// The production location: `~/.proxylab/traffic`, or wherever
    /// `PROXYLAB_TRAFFIC_DIR` points.
    pub fn default_location() -> Result<Self, StorageError> {
        Ok(Self::in_directory(Self::default_directory()?))
    }

    /// The directory the production location is inside.
    pub fn default_directory() -> Result<PathBuf, StorageError> {
        if let Some(directory) = env::var_os(TRAFFIC_DIRECTORY_VARIABLE) {
            if !directory.is_empty() {
                return Ok(PathBuf::from(directory));
            }
        }

        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .filter(|home| !home.is_empty())
            .ok_or(StorageError::NoHomeDirectory)?;

        Ok(PathBuf::from(home)
            .join(STATE_DIRECTORY)
            .join(TRAFFIC_DIRECTORY))
    }

    /// The same configuration, keeping bodies up to `max` bytes in the
    /// database.
    pub fn with_inline_body_max_bytes(mut self, max: usize) -> Self {
        self.inline_body_max_bytes = max;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_holds_the_database_and_the_bodies_beside_it() {
        let config = StorageConfig::in_directory("/tmp/proxylab-test");

        assert_eq!(
            config.database,
            Path::new("/tmp/proxylab-test").join("traffic.db")
        );
        assert_eq!(
            config.bodies,
            Path::new("/tmp/proxylab-test").join("bodies")
        );
    }

    #[test]
    fn the_database_and_the_bodies_can_be_named_separately() {
        let config = StorageConfig::new("/tmp/one/db.sqlite", "/tmp/two");

        assert_eq!(config.database, Path::new("/tmp/one/db.sqlite"));
        assert_eq!(config.bodies, Path::new("/tmp/two"));
    }

    #[test]
    fn the_inline_limit_defaults_to_sixty_four_kibibytes() {
        assert_eq!(DEFAULT_INLINE_BODY_MAX_BYTES, 65_536);
        assert_eq!(
            StorageConfig::in_directory("/tmp").inline_body_max_bytes,
            DEFAULT_INLINE_BODY_MAX_BYTES
        );
    }

    #[test]
    fn the_inline_limit_can_be_set() {
        let config = StorageConfig::in_directory("/tmp").with_inline_body_max_bytes(0);

        assert_eq!(config.inline_body_max_bytes, 0);
    }

    #[test]
    fn the_default_directory_is_under_the_home_directory() {
        // The variable is read, not written: writing it would race with every
        // other test in this binary. What is asserted is the shape of the
        // path when it is not set, which is what a launching proxy uses.
        if env::var_os(TRAFFIC_DIRECTORY_VARIABLE).is_some() {
            return;
        }

        let directory = StorageConfig::default_directory().expect("a home directory");

        assert!(directory.ends_with(Path::new(STATE_DIRECTORY).join(TRAFFIC_DIRECTORY)));
        assert!(directory.is_absolute(), "{directory:?}");
    }

    #[test]
    fn the_default_location_puts_both_in_the_default_directory() {
        if env::var_os(TRAFFIC_DIRECTORY_VARIABLE).is_some() {
            return;
        }

        let config = StorageConfig::default_location().expect("a home directory");

        assert!(config.database.ends_with(DATABASE_FILE));
        assert!(config.bodies.ends_with(BODIES_DIRECTORY));
        assert_eq!(config.database.parent(), config.bodies.parent());
    }
}
