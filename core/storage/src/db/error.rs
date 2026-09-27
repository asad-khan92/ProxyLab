//! Why storing or reading captured traffic failed.
//!
//! Storage is where a debugging proxy keeps what it saw; it is not part of
//! carrying a request. So nothing here is ever turned into an HTTP status or
//! sent to a client: a storage failure is reported to whoever asked, counted,
//! and logged, and the proxy carries on. See the crate documentation for the
//! whole of that rule.
//!
//! # What an error may say
//!
//! The path it was working on and what the underlying failure was. Paths
//! here are ProxyLab's own — a database file, a body file named after an
//! exchange id — so they name no traffic. What an error never carries is
//! anything out of the exchange: no URL, no header, no body. The one thing
//! from an exchange that reaches a message is its id, which is a UUID.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Why a storage operation did not happen.
#[derive(Debug)]
pub enum StorageError {
    /// There is no home directory to work out the default location from.
    NoHomeDirectory,
    /// A directory the database or its bodies live in could not be made.
    Directory {
        /// The directory that could not be made.
        path: PathBuf,
        /// Why not.
        cause: io::Error,
    },
    /// A body file could not be written, read, or replaced.
    Body {
        /// The file being worked on.
        path: PathBuf,
        /// Why it failed.
        cause: io::Error,
    },
    /// The database itself refused the work.
    Database(rusqlite::Error),
    /// The database was written by a later version of ProxyLab.
    ///
    /// Migrating forwards is this crate's job; migrating backwards is not, so
    /// a database from the future is left alone rather than opened and
    /// misread.
    UnsupportedSchema {
        /// The version the database is at.
        found: u32,
        /// The newest version this build knows how to read.
        supported: u32,
    },
    /// A stored row could not be read back as the model it came from.
    ///
    /// The database has a value this build cannot make sense of — a scheme
    /// that is neither `http` nor `https`, a client address that is not an
    /// address, headers that are not the JSON they were written as.
    Unreadable {
        /// Which column, in the form a developer can act on.
        column: &'static str,
    },
    /// A search was asked to be a regular expression and was not one.
    InvalidSearch,
}

impl StorageError {
    /// A directory failure for `path`.
    pub(super) fn directory(path: impl Into<PathBuf>, cause: io::Error) -> Self {
        Self::Directory {
            path: path.into(),
            cause,
        }
    }

    /// A body-file failure for `path`.
    pub(super) fn body(path: impl AsRef<Path>, cause: io::Error) -> Self {
        Self::Body {
            path: path.as_ref().to_path_buf(),
            cause,
        }
    }

    /// A stored value that could not be read back.
    pub(super) fn unreadable(column: &'static str) -> Self {
        Self::Unreadable { column }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHomeDirectory => f.write_str(
                "no home directory to keep captured traffic in: neither HOME nor USERPROFILE is set",
            ),
            Self::Directory { path, cause } => {
                write!(f, "could not prepare {}: {cause}", path.display())
            }
            Self::Body { path, cause } => {
                write!(f, "could not store the body file {}: {cause}", path.display())
            }
            Self::Database(cause) => write!(f, "the traffic database refused the work: {cause}"),
            Self::UnsupportedSchema { found, supported } => write!(
                f,
                "the traffic database is at schema version {found}, and this build reads up to {supported}",
            ),
            Self::Unreadable { column } => {
                write!(f, "a stored exchange has an unreadable {column}")
            }
            Self::InvalidSearch => {
                f.write_str("the search is not a regular expression")
            }
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory { cause, .. } | Self::Body { cause, .. } => Some(cause),
            Self::Database(cause) => Some(cause),
            Self::NoHomeDirectory
            | Self::UnsupportedSchema { .. }
            | Self::Unreadable { .. }
            | Self::InvalidSearch => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error as _;

    fn every_error() -> Vec<StorageError> {
        vec![
            StorageError::NoHomeDirectory,
            StorageError::directory(
                "/tmp/proxylab",
                io::Error::from(io::ErrorKind::PermissionDenied),
            ),
            StorageError::body(
                "/tmp/proxylab/bodies/x.request",
                io::Error::from(io::ErrorKind::NotFound),
            ),
            StorageError::Database(rusqlite::Error::QueryReturnedNoRows),
            StorageError::UnsupportedSchema {
                found: 9,
                supported: 1,
            },
            StorageError::unreadable("client address"),
            StorageError::InvalidSearch,
        ]
    }

    #[test]
    fn every_error_says_something() {
        for error in every_error() {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }

    #[test]
    fn a_directory_failure_names_the_directory_and_the_cause() {
        let error = StorageError::directory(
            "/tmp/proxylab",
            io::Error::from(io::ErrorKind::PermissionDenied),
        );

        let message = error.to_string();
        assert!(message.contains("/tmp/proxylab"), "{message}");
        assert!(error.source().is_some());
    }

    #[test]
    fn a_database_failure_carries_the_database_error() {
        let error = StorageError::from(rusqlite::Error::QueryReturnedNoRows);

        assert!(matches!(error, StorageError::Database(_)));
        assert!(error.source().is_some());
    }

    #[test]
    fn a_database_from_the_future_says_both_versions() {
        let message = StorageError::UnsupportedSchema {
            found: 9,
            supported: 1,
        }
        .to_string();

        assert!(message.contains('9'), "{message}");
        assert!(message.contains('1'), "{message}");
    }
}
