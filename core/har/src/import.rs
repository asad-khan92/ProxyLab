//! Reading a HAR file back as captured traffic.
//!
//! ```text
//! a .har file ──▶ Har ──▶ entries ──▶ TrafficExchange, one per entry
//! ```
//!
//! What comes out is the traffic model ProxyLab already has, so an imported
//! exchange is inspected, searched, filtered, copied as cURL and replayed by
//! the same code as a captured one. There is no second traffic model and no
//! importer-shaped view of an exchange.
//!
//! # Nothing is sent
//!
//! Importing produces records. It opens no connection, consults no rule, and
//! hands nothing to the proxy: a HAR file somebody sent you may describe
//! requests that should never be made again, and reading it must not be a way
//! to make them. Replaying an imported request is a separate, deliberate act,
//! and is the same replay every other exchange offers.
//!
//! # A file from outside is bounded before it is read
//!
//! A HAR file is written by something else and is as large as that something
//! else decided. So the file's length is checked before it is opened, the
//! number of entries taken from it is capped, and each body is held to the
//! proxy's own capture limit. What is left out is counted and reported rather
//! than passed over in silence.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use proxy_storage::{TrafficExchange, DEFAULT_MAX_CAPTURE_BODY_SIZE};

use crate::convert;
use crate::error::{HarError, PathProblem};
use crate::model::Har;

/// The largest HAR file this crate will read, in bytes.
///
/// A HAR file is text with base64 bodies in it, so this is also the bound on
/// everything an import puts in memory at once. Sixty-four mebibytes is a
/// long debugging session's worth of traffic and is small enough that reading
/// one is not a way to exhaust this machine.
pub const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;

/// The most entries one import will take.
///
/// Far more than a person reads, and a bound on how many rows one file can
/// add to the traffic store.
pub const MAX_IMPORT_ENTRIES: usize = 10_000;

/// What an import is allowed to take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportLimits {
    /// The longest file that will be read, in bytes.
    pub max_file_bytes: u64,
    /// The most entries that will be turned into exchanges.
    pub max_entries: usize,
    /// The most of any one body that is kept, in bytes.
    ///
    /// The proxy's own capture limit, so an imported body is bounded exactly
    /// as a captured one is — and a body cut short by it is marked truncated,
    /// which is what a capture cut short has always been.
    pub max_body_bytes: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: MAX_IMPORT_BYTES,
            max_entries: MAX_IMPORT_ENTRIES,
            max_body_bytes: DEFAULT_MAX_CAPTURE_BODY_SIZE,
        }
    }
}

/// What a HAR file came to.
#[derive(Clone, Debug, Default)]
pub struct Import {
    /// The exchanges the file described.
    pub exchanges: Vec<TrafficExchange>,
    /// How many entries were not turned into one.
    ///
    /// An entry that names no method or no URL is not an exchange, and an
    /// entry past [`ImportLimits::max_entries`] is one this import did not
    /// take. Both are counted here, so a client can say what did not come in
    /// rather than showing a shorter list than the file had.
    pub skipped: u64,
}

/// Reads the HAR file at `path`.
pub fn read(path: impl AsRef<Path>, limits: &ImportLimits) -> Result<Import, HarError> {
    let path = path.as_ref();
    check(path)?;

    let file = File::open(path).map_err(|cause| HarError::file(path, cause))?;
    let length = file
        .metadata()
        .map_err(|cause| HarError::file(path, cause))?
        .len();
    if length > limits.max_file_bytes {
        return Err(HarError::TooLarge {
            path: path.to_path_buf(),
            limit: limits.max_file_bytes,
        });
    }

    // The parser's own message quotes the text it choked on, which for this
    // file is captured traffic. So it is dropped and the file is reported as
    // what it is: not a HAR document.
    let har: Har = serde_json::from_reader(BufReader::new(file)).map_err(|_| HarError::NotHar {
        path: path.to_path_buf(),
    })?;

    Ok(entries(har, limits))
}

/// Reads a HAR document that is already in memory.
///
/// What [`read`] does once it has the file, and what a test uses when the
/// point is the entries rather than the file they came from.
pub fn entries(har: Har, limits: &ImportLimits) -> Import {
    let total = har.log.entries.len();
    let taken = total.min(limits.max_entries);
    let mut import = Import {
        exchanges: Vec::with_capacity(taken),
        skipped: (total - taken) as u64,
    };

    for entry in har.log.entries.iter().take(taken) {
        match convert::exchange(entry, limits.max_body_bytes) {
            Some(exchange) => import.exchanges.push(exchange),
            None => import.skipped += 1,
        }
    }

    import
}

/// Checks that `path` is one this crate will read a file from.
fn check(path: &Path) -> Result<(), HarError> {
    if path.as_os_str().is_empty() {
        return Err(HarError::path(path, PathProblem::Empty));
    }
    if !path.is_absolute() {
        return Err(HarError::path(path, PathProblem::NotAbsolute));
    }
    if path.is_dir() {
        return Err(HarError::path(path, PathProblem::IsDirectory));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use proxy_storage::{
        CapturedRequest, ConnectionInfo, Destination, ExchangeId, HttpVersion, Scheme,
    };
    use tempfile::TempDir;

    use crate::export;

    fn exchange(url: &str) -> TrafficExchange {
        TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::https(
                "127.0.0.1:1".parse().expect("an address"),
                Some(Destination::new("example.com", 443)),
            ),
            CapturedRequest::new("GET", url, HttpVersion::Http11),
        )
    }

    #[test]
    fn what_was_exported_is_what_is_imported() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");
        export::write_all(
            &path,
            [
                &exchange("https://example.com/one"),
                &exchange("http://other.example:8080/two"),
            ],
        )
        .expect("exports");

        let import = read(&path, &ImportLimits::default()).expect("imports");

        assert_eq!(import.skipped, 0);
        assert_eq!(import.exchanges.len(), 2);
        assert_eq!(import.exchanges[0].request.url, "https://example.com/one");
        assert_eq!(import.exchanges[0].connection.scheme, Scheme::Https);
        assert_eq!(
            import.exchanges[1].connection.destination,
            Some(Destination::new("other.example", 8080))
        );
    }

    #[test]
    fn every_imported_exchange_has_an_id_of_its_own() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");
        let one = exchange("https://example.com/one");
        export::write_all(&path, [&one, &one]).expect("exports");

        let import = read(&path, &ImportLimits::default()).expect("imports");

        assert_ne!(import.exchanges[0].id, import.exchanges[1].id);
        assert_ne!(import.exchanges[0].id, one.id);
    }

    #[test]
    fn an_entry_that_is_not_an_exchange_is_counted_and_not_imported() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");
        fs::write(
            &path,
            br#"{"log":{"version":"1.2","creator":{"name":"x","version":"1"},"entries":[
                {"startedDateTime":"2026-09-20T10:11:12.345Z","time":1,
                 "request":{"method":"GET","url":"https://example.com/"},
                 "response":{"status":200},"cache":{},"timings":{"send":0,"wait":1,"receive":0}},
                {"startedDateTime":"2026-09-20T10:11:13.345Z","time":1,
                 "request":{"method":"","url":""},
                 "response":{"status":200},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}
            ]}}"#,
        )
        .expect("writes");

        let import = read(&path, &ImportLimits::default()).expect("imports");

        assert_eq!(import.exchanges.len(), 1);
        assert_eq!(import.skipped, 1);
    }

    #[test]
    fn entries_past_the_limit_are_counted_and_not_imported() {
        let har: Har = serde_json::from_str(
            r#"{"log":{"version":"1.2","creator":{"name":"x","version":"1"},"entries":[
                {"request":{"method":"GET","url":"https://example.com/1"},"response":{"status":200}},
                {"request":{"method":"GET","url":"https://example.com/2"},"response":{"status":200}},
                {"request":{"method":"GET","url":"https://example.com/3"},"response":{"status":200}}
            ]}}"#,
        )
        .expect("a document");

        let import = entries(
            har,
            &ImportLimits {
                max_entries: 2,
                ..ImportLimits::default()
            },
        );

        assert_eq!(import.exchanges.len(), 2);
        assert_eq!(import.skipped, 1);
    }

    #[test]
    fn a_file_that_is_not_json_is_reported_without_quoting_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("notes.har");
        fs::write(&path, b"Authorization: Bearer sk-secret").expect("writes");

        let error = read(&path, &ImportLimits::default()).expect_err("refused");

        assert!(matches!(error, HarError::NotHar { .. }), "{error:?}");
        assert!(!error.to_string().contains("sk-secret"), "{error}");
    }

    #[test]
    fn json_that_is_not_a_har_document_is_refused() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("other.har");
        fs::write(&path, br#"{"requests": []}"#).expect("writes");

        let error = read(&path, &ImportLimits::default()).expect_err("refused");

        assert!(matches!(error, HarError::NotHar { .. }), "{error:?}");
    }

    #[test]
    fn a_file_longer_than_the_limit_is_refused_before_it_is_parsed() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("huge.har");
        fs::write(&path, vec![b'x'; 4_096]).expect("writes");

        let error = read(
            &path,
            &ImportLimits {
                max_file_bytes: 1_024,
                ..ImportLimits::default()
            },
        )
        .expect_err("refused");

        assert!(
            matches!(error, HarError::TooLarge { limit: 1_024, .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_there_says_so() {
        let directory = TempDir::new().expect("a temporary directory");

        let error = read(
            directory.path().join("missing.har"),
            &ImportLimits::default(),
        )
        .expect_err("refused");

        assert!(matches!(error, HarError::File { .. }), "{error:?}");
    }

    #[test]
    fn a_relative_path_is_refused() {
        let error = read("traffic.har", &ImportLimits::default()).expect_err("refused");

        assert!(
            matches!(
                error,
                HarError::Path {
                    reason: PathProblem::NotAbsolute,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn an_empty_har_document_imports_nothing_and_is_not_an_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("empty.har");
        export::write_all(&path, []).expect("exports");

        let import = read(&path, &ImportLimits::default()).expect("imports");

        assert!(import.exchanges.is_empty());
        assert_eq!(import.skipped, 0);
    }
}
