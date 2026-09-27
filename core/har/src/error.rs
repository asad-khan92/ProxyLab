//! Why a HAR file could not be written or read.
//!
//! # What an error may say
//!
//! The path it was working on, and what the operating system said about it.
//! The path is one a person chose in a save or open panel, so it is theirs to
//! be told about; what an error never carries is anything out of the file —
//! no URL, no header, no body — because a HAR file is a record of traffic and
//! an error goes into a log.
//!
//! A file that is not a HAR document is reported as exactly that, and not as
//! the parser's own complaint: `serde_json`'s message quotes the text it
//! choked on, which for this file is captured traffic.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Why an export or an import did not happen.
#[derive(Debug)]
pub enum HarError {
    /// The path was not one this crate will write to or read from.
    Path {
        /// The path.
        path: PathBuf,
        /// What is wrong with it.
        reason: PathProblem,
    },
    /// The file could not be written or read.
    File {
        /// The file being worked on.
        path: PathBuf,
        /// Why it failed.
        cause: io::Error,
    },
    /// The file is not a HAR document.
    ///
    /// Deliberately without the parser's own message: see the module
    /// documentation.
    NotHar {
        /// The file that is not one.
        path: PathBuf,
    },
    /// The file is longer than this crate will read.
    TooLarge {
        /// The file that is.
        path: PathBuf,
        /// How long it is allowed to be, in bytes.
        limit: u64,
    },
}

/// What is wrong with a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathProblem {
    /// It is empty.
    Empty,
    /// It is relative, and a proxy has no meaningful working directory to
    /// resolve it against.
    NotAbsolute,
    /// There is a directory at it.
    IsDirectory,
    /// The directory it is in does not exist. Nothing here creates one: a
    /// path a person picked in a panel has a directory, and creating one for
    /// a path that was typed would be making a folder nobody asked for.
    NoDirectory,
}

impl PathProblem {
    /// What is wrong, as a phrase.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Empty => "no path was given",
            Self::NotAbsolute => "the path is not absolute",
            Self::IsDirectory => "there is a folder at that path",
            Self::NoDirectory => "the folder that path is in does not exist",
        }
    }
}

impl HarError {
    /// A path this crate will not use.
    pub(crate) fn path(path: impl AsRef<Path>, reason: PathProblem) -> Self {
        Self::Path {
            path: path.as_ref().to_path_buf(),
            reason,
        }
    }

    /// A file that could not be written or read.
    pub(crate) fn file(path: impl AsRef<Path>, cause: io::Error) -> Self {
        Self::File {
            path: path.as_ref().to_path_buf(),
            cause,
        }
    }
}

impl fmt::Display for HarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path { path, reason } => {
                write!(f, "{}: {}", path.display(), reason.describe())
            }
            Self::File { path, cause } => write!(f, "{}: {cause}", path.display()),
            Self::NotHar { path } => {
                write!(f, "{} is not a HAR 1.2 document", path.display())
            }
            Self::TooLarge { path, limit } => write!(
                f,
                "{} is longer than the {limit} bytes ProxyLab will read",
                path.display()
            ),
        }
    }
}

impl std::error::Error for HarError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File { cause, .. } => Some(cause),
            Self::Path { .. } | Self::NotHar { .. } | Self::TooLarge { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_says_something_and_names_its_path() {
        let errors = [
            HarError::path("relative.har", PathProblem::NotAbsolute),
            HarError::file(
                "/tmp/x.har",
                io::Error::from(io::ErrorKind::PermissionDenied),
            ),
            HarError::NotHar {
                path: PathBuf::from("/tmp/x.har"),
            },
            HarError::TooLarge {
                path: PathBuf::from("/tmp/x.har"),
                limit: 64,
            },
        ];

        for error in errors {
            let message = error.to_string();

            assert!(!message.is_empty(), "{error:?}");
            assert!(message.contains(".har"), "{message}");
        }
    }

    #[test]
    fn a_file_that_is_not_a_har_document_does_not_quote_it() {
        let message = HarError::NotHar {
            path: PathBuf::from("/tmp/captured.har"),
        }
        .to_string();

        assert_eq!(message, "/tmp/captured.har is not a HAR 1.2 document");
    }

    #[test]
    fn every_path_problem_has_a_phrase() {
        for problem in [
            PathProblem::Empty,
            PathProblem::NotAbsolute,
            PathProblem::IsDirectory,
            PathProblem::NoDirectory,
        ] {
            assert!(!problem.describe().is_empty(), "{problem:?}");
        }
    }
}
