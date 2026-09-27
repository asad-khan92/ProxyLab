//! Why a TLS operation did not succeed.
//!
//! One error type for the whole crate, because the caller's decision is the
//! same shape whatever failed: it cannot intercept this connection. What
//! differs is what the caller may say about it, so every variant is written
//! for a log — a developer needs to know which file, which host, which
//! handshake — and never carries key material. Neither
//! [`rcgen::Error`](rcgen::Error) nor [`rustls::Error`] echoes the input it
//! was given, so wrapping them is safe; what a client is told is the caller's
//! own decision.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why a TLS operation did not succeed.
#[derive(Debug)]
#[non_exhaustive]
pub enum TlsError {
    /// The directory the CA lives in could not be created.
    CaDirectory {
        /// The directory that was being created.
        path: PathBuf,
        /// The underlying filesystem error.
        source: io::Error,
    },
    /// One of the CA's files could not be read.
    CaRead {
        /// The file that was being read.
        path: PathBuf,
        /// The underlying filesystem error.
        source: io::Error,
    },
    /// One of the CA's files could not be written.
    CaWrite {
        /// The file that was being written.
        path: PathBuf,
        /// The underlying filesystem error.
        source: io::Error,
    },
    /// The stored CA has one of its two files and not the other.
    ///
    /// Reported rather than repaired: half a CA cannot sign anything, and
    /// generating a replacement would silently retire the CA the developer
    /// installed and trusted.
    CaIncomplete {
        /// The file that is there.
        present: PathBuf,
        /// The file that is not.
        missing: PathBuf,
    },
    /// The stored CA could not be read as a certificate and a key.
    CaMalformed {
        /// The file that could not be parsed.
        path: PathBuf,
        /// What the certificate library made of it.
        source: rcgen::Error,
    },
    /// There is no home directory to put the default CA directory in.
    NoHomeDirectory,
    /// A certificate could not be generated.
    Certificate(rcgen::Error),
    /// A TLS configuration could not be built from a certificate and key.
    Configuration(rustls::Error),
    /// A certificate could not be used as a trust anchor.
    TrustAnchor(rustls::Error),
    /// The TLS handshake with the client failed.
    ClientHandshake(io::Error),
    /// The TLS handshake with the origin server failed.
    UpstreamHandshake(io::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CaDirectory { path, source } => {
                write!(f, "could not create the CA directory {path:?}: {source}")
            }
            Self::CaRead { path, source } => write!(f, "could not read {path:?}: {source}"),
            Self::CaWrite { path, source } => write!(f, "could not write {path:?}: {source}"),
            Self::CaIncomplete { present, missing } => write!(
                f,
                "the stored CA is incomplete: {present:?} is there but {missing:?} is not"
            ),
            Self::CaMalformed { path, source } => {
                write!(f, "could not use the CA in {path:?}: {source}")
            }
            Self::NoHomeDirectory => {
                f.write_str("no home directory to store the development CA in")
            }
            Self::Certificate(source) => write!(f, "could not generate a certificate: {source}"),
            Self::Configuration(source) => write!(f, "could not configure TLS: {source}"),
            Self::TrustAnchor(source) => write!(f, "could not trust a certificate: {source}"),
            Self::ClientHandshake(source) => {
                write!(f, "the client's TLS handshake failed: {source}")
            }
            Self::UpstreamHandshake(source) => {
                write!(f, "the upstream TLS handshake failed: {source}")
            }
        }
    }
}

impl StdError for TlsError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::CaDirectory { source, .. }
            | Self::CaRead { source, .. }
            | Self::CaWrite { source, .. } => Some(source),
            Self::CaMalformed { source, .. } => Some(source),
            Self::Certificate(source) => Some(source),
            Self::Configuration(source) | Self::TrustAnchor(source) => Some(source),
            Self::ClientHandshake(source) | Self::UpstreamHandshake(source) => Some(source),
            Self::CaIncomplete { .. } | Self::NoHomeDirectory => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    fn every_error() -> Vec<TlsError> {
        vec![
            TlsError::CaDirectory {
                path: PathBuf::from("/ca"),
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            },
            TlsError::CaRead {
                path: PathBuf::from("/ca/ca-cert.pem"),
                source: io::Error::from(io::ErrorKind::NotFound),
            },
            TlsError::CaWrite {
                path: PathBuf::from("/ca/ca-key.pem"),
                source: io::Error::from(io::ErrorKind::AlreadyExists),
            },
            TlsError::CaIncomplete {
                present: PathBuf::from("/ca/ca-cert.pem"),
                missing: PathBuf::from("/ca/ca-key.pem"),
            },
            TlsError::CaMalformed {
                path: PathBuf::from("/ca/ca-key.pem"),
                source: rcgen::Error::CouldNotParseKeyPair,
            },
            TlsError::NoHomeDirectory,
            TlsError::Certificate(rcgen::Error::UnsupportedSignatureAlgorithm),
            TlsError::Configuration(rustls::Error::NoCertificatesPresented),
            TlsError::TrustAnchor(rustls::Error::NoCertificatesPresented),
            TlsError::ClientHandshake(io::Error::from(io::ErrorKind::UnexpectedEof)),
            TlsError::UpstreamHandshake(io::Error::from(io::ErrorKind::ConnectionReset)),
        ]
    }

    #[test]
    fn every_error_says_what_failed() {
        for error in every_error() {
            assert!(!error.to_string().is_empty(), "{error:?} explains nothing");
        }
    }

    #[test]
    fn a_file_error_names_the_file_and_keeps_its_cause() {
        let error = TlsError::CaRead {
            path: Path::new("/ca/ca-cert.pem").to_owned(),
            source: io::Error::from(io::ErrorKind::NotFound),
        };

        assert!(error.to_string().contains("ca-cert.pem"));
        assert!(error.source().is_some());
    }

    #[test]
    fn an_incomplete_ca_names_both_halves_and_has_no_cause() {
        let error = TlsError::CaIncomplete {
            present: PathBuf::from("/ca/ca-cert.pem"),
            missing: PathBuf::from("/ca/ca-key.pem"),
        };
        let message = error.to_string();

        assert!(message.contains("ca-cert.pem"), "{message}");
        assert!(message.contains("ca-key.pem"), "{message}");
        assert!(error.source().is_none());
    }
}
