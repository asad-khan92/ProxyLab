//! Writing captured traffic out as a HAR file.
//!
//! ```text
//! create  ──▶  {"log":{"version":"1.2","creator":{…},"entries":[
//! write   ──▶      {…entry…}, {…entry…}, …
//! finish  ──▶  ]}}   ──▶ rename into place
//! ```
//!
//! One entry at a time, straight to the file. Exporting everything a
//! debugging session captured is thousands of exchanges with their bodies,
//! and building that document in memory before writing a byte of it would
//! make the size of an export the size of the process. So [`HarWriter`] holds
//! a file and a count, and each exchange is serialized and written as it is
//! handed over.
//!
//! # Nothing is written under the real name until it is whole
//!
//! The bytes go to a temporary file beside the destination, and the
//! destination is only created — by a rename — once the document has been
//! closed and flushed. A failed export therefore leaves either nothing or
//! whatever was already at that path, and never a half a HAR file with the
//! name of a whole one. A writer dropped without [`finish`](HarWriter::finish)
//! removes what it wrote.
//!
//! # A path a client chose
//!
//! The path comes from a save panel on the other side of the IPC boundary, so
//! it is checked here before anything is opened: absolute, not a directory,
//! and inside a directory that exists. Nothing is created but the file
//! itself — an export does not make folders — and nothing is appended to a
//! path the caller gave, so there is no joining for a traversal to happen in.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use proxy_storage::TrafficExchange;

use crate::convert;
use crate::error::{HarError, PathProblem};
use crate::model::{Creator, HAR_VERSION};

/// The name this crate writes into a HAR file's `creator`.
pub const CREATOR_NAME: &str = "ProxyLab";

/// The suffix a HAR file wears while it is being written.
const PARTIAL_SUFFIX: &str = "tmp";

/// Makes every temporary name in this process different from the last.
static ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// What an export came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// How many exchanges were written.
    pub entries: u64,
    /// How many bytes the file is.
    pub bytes: u64,
}

/// A HAR file being written, one exchange at a time.
#[derive(Debug)]
pub struct HarWriter {
    file: Option<BufWriter<File>>,
    partial: PathBuf,
    destination: PathBuf,
    entries: u64,
}

impl HarWriter {
    /// Opens a HAR file at `path` and writes the head of the document.
    ///
    /// The file at `path` is not touched until [`finish`](Self::finish)
    /// renames the finished document over it.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, HarError> {
        let destination = checked_destination(path.as_ref())?;
        let partial = partial_path(&destination);

        let file = File::create(&partial).map_err(|cause| HarError::file(&partial, cause))?;
        let mut writer = Self {
            file: Some(BufWriter::new(file)),
            partial,
            destination,
            entries: 0,
        };

        // Written by hand rather than by serializing a `Log` with no entries
        // and then splicing into it: the document is opened here and closed
        // in `finish`, and the entries go between them.
        let creator = Creator {
            name: CREATOR_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let head = format!(
            "{{\"log\":{{\"version\":\"{HAR_VERSION}\",\"creator\":{},\"entries\":[",
            serde_json::to_string(&creator).expect("a creator serializes"),
        );
        writer.put(head.as_bytes())?;

        Ok(writer)
    }

    /// Writes one exchange as the entry it is.
    pub fn write(&mut self, exchange: &TrafficExchange) -> Result<(), HarError> {
        let entry = convert::entry(exchange);
        // An entry is built from a captured exchange, whose every field is a
        // string, a number, or bytes: there is nothing in one that JSON
        // cannot hold, so a failure here would be a bug rather than a
        // condition to report.
        let text = serde_json::to_vec(&entry).expect("an entry serializes");

        if self.entries > 0 {
            self.put(b",")?;
        }
        self.put(&text)?;
        self.entries += 1;

        Ok(())
    }

    /// How many exchanges have been written so far.
    pub fn entries(&self) -> u64 {
        self.entries
    }

    /// Closes the document and puts it at the path it was created for.
    ///
    /// Flushed and synced before the rename, so the file that appears under
    /// the destination's name is the whole document.
    pub fn finish(mut self) -> Result<ExportSummary, HarError> {
        self.put(b"]}}\n")?;

        let mut file = self
            .file
            .take()
            .expect("a writer is only finished once")
            .into_inner()
            .map_err(|error| HarError::file(&self.partial, error.into_error()))?;
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|cause| HarError::file(&self.partial, cause))?;

        let bytes = file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        drop(file);

        fs::rename(&self.partial, &self.destination)
            .map_err(|cause| HarError::file(&self.destination, cause))?;

        Ok(ExportSummary {
            entries: self.entries,
            bytes,
        })
    }

    /// Writes bytes into the document.
    fn put(&mut self, bytes: &[u8]) -> Result<(), HarError> {
        let partial = self.partial.clone();
        let file = self.file.as_mut().expect("a writer is only finished once");

        file.write_all(bytes)
            .map_err(|cause| HarError::file(&partial, cause))
    }
}

/// A writer that was never finished leaves nothing behind.
impl Drop for HarWriter {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

/// Writes `exchanges` to `path` as one HAR file.
///
/// What an export of a handful of selected exchanges is; an export of
/// everything a filter matches drives a [`HarWriter`] page by page instead,
/// so no caller ever holds the whole list.
pub fn write_all<'a>(
    path: impl AsRef<Path>,
    exchanges: impl IntoIterator<Item = &'a TrafficExchange>,
) -> Result<ExportSummary, HarError> {
    let mut writer = HarWriter::create(path)?;

    for exchange in exchanges {
        writer.write(exchange)?;
    }

    writer.finish()
}

/// Checks that `path` is one this crate will write a file at.
fn checked_destination(path: &Path) -> Result<PathBuf, HarError> {
    if path.as_os_str().is_empty() {
        return Err(HarError::path(path, PathProblem::Empty));
    }
    if !path.is_absolute() {
        return Err(HarError::path(path, PathProblem::NotAbsolute));
    }
    if path.is_dir() {
        return Err(HarError::path(path, PathProblem::IsDirectory));
    }

    let Some(directory) = path.parent() else {
        return Err(HarError::path(path, PathProblem::NoDirectory));
    };
    if !directory.is_dir() {
        return Err(HarError::path(path, PathProblem::NoDirectory));
    }

    Ok(path.to_path_buf())
}

/// The name the document wears while it is being written.
///
/// Beside the destination, so the rename that publishes it is within one
/// file system and cannot fail halfway. Unique per attempt, so two exports to
/// one path cannot write over each other's partial file.
fn partial_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.har".to_owned());
    let attempt = ATTEMPT.fetch_add(1, Ordering::Relaxed);

    destination.with_file_name(format!(
        ".{name}.{}.{attempt}.{PARTIAL_SUFFIX}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::{
        BodyCapture, CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId,
        HttpVersion,
    };
    use tempfile::TempDir;

    use crate::model::Har;

    fn exchange(url: &str) -> TrafficExchange {
        let mut exchange = TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::https(
                "127.0.0.1:1".parse().expect("an address"),
                Some(Destination::new("example.com", 443)),
            ),
            CapturedRequest::new("GET", url, HttpVersion::Http11),
        );
        let mut response = CapturedResponse::new(200, HttpVersion::Http11);
        response.body = BodyCapture::complete(b"hello".to_vec());
        exchange.response = Some(response);

        exchange
    }

    fn read(path: &Path) -> Har {
        let text = fs::read_to_string(path).expect("the file is there");

        serde_json::from_str(&text).expect("it is a HAR document")
    }

    #[test]
    fn an_export_is_a_har_document_with_one_entry_per_exchange() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");

        let summary = write_all(
            &path,
            [
                &exchange("https://example.com/one"),
                &exchange("https://example.com/two"),
            ],
        )
        .expect("exports");

        assert_eq!(summary.entries, 2);
        assert!(summary.bytes > 0);

        let har = read(&path);
        assert_eq!(har.log.version, "1.2");
        assert_eq!(har.log.creator.name, "ProxyLab");
        assert!(!har.log.creator.version.is_empty());
        assert_eq!(har.log.entries.len(), 2);
        assert_eq!(har.log.entries[0].request.url, "https://example.com/one");
        assert_eq!(har.log.entries[1].request.url, "https://example.com/two");
    }

    #[test]
    fn an_export_of_nothing_is_a_har_document_with_no_entries() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("empty.har");

        let summary = write_all(&path, []).expect("exports");

        assert_eq!(summary.entries, 0);
        assert!(read(&path).log.entries.is_empty());
    }

    #[test]
    fn an_export_replaces_what_was_at_the_path() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");
        fs::write(&path, b"whatever was here before").expect("writes");

        write_all(&path, [&exchange("https://example.com/")]).expect("exports");

        assert_eq!(read(&path).log.entries.len(), 1);
    }

    #[test]
    fn a_writer_that_is_never_finished_leaves_nothing_behind() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");

        {
            let mut writer = HarWriter::create(&path).expect("creates");
            writer
                .write(&exchange("https://example.com/"))
                .expect("writes");
            assert_eq!(writer.entries(), 1);
        }

        assert!(!path.exists(), "the destination was created");
        let left: Vec<_> = fs::read_dir(directory.path())
            .expect("readable")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_path_that_is_not_absolute_is_refused_before_anything_is_opened() {
        let error = HarWriter::create("traffic.har").expect_err("refused");

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
    fn a_directory_is_refused() {
        let directory = TempDir::new().expect("a temporary directory");

        let error = HarWriter::create(directory.path()).expect_err("refused");

        assert!(
            matches!(
                error,
                HarError::Path {
                    reason: PathProblem::IsDirectory,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_path_in_a_folder_that_is_not_there_is_refused_rather_than_created() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("nowhere").join("traffic.har");

        let error = HarWriter::create(&path).expect_err("refused");

        assert!(
            matches!(
                error,
                HarError::Path {
                    reason: PathProblem::NoDirectory,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!directory.path().join("nowhere").exists());
    }

    #[test]
    fn two_writers_for_one_path_do_not_disturb_each_other() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("traffic.har");

        let mut first = HarWriter::create(&path).expect("creates");
        let mut second = HarWriter::create(&path).expect("creates");
        first
            .write(&exchange("https://example.com/one"))
            .expect("writes");
        second
            .write(&exchange("https://example.com/two"))
            .expect("writes");
        second
            .write(&exchange("https://example.com/three"))
            .expect("writes");

        assert_eq!(first.finish().expect("finishes").entries, 1);
        assert_eq!(second.finish().expect("finishes").entries, 2);
        assert_eq!(read(&path).log.entries.len(), 2);
    }
}
