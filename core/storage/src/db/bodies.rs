//! Where a captured body's bytes go.
//!
//! ```text
//! captured body
//!      |
//!      +-- nothing captured  -> no bytes stored at all
//!      |
//!      +-- small             -> a BLOB in the database
//!      |
//!      +-- large             -> a file, and the database keeps its name
//! ```
//!
//! Only what was *captured* is stored. A 20 MB response that was captured up
//! to a 1 MB limit stores 1 MB, records that the body was 20 MB, and records
//! that the capture was cut short; nothing here tries to reconstruct the rest,
//! because the rest was forwarded and never kept.
//!
//! # The file layout
//!
//! One file per message: `<exchange id>.request` and
//! `<exchange id>.response`, in the configured body directory. Deterministic,
//! so a body's file is known from the row that references it, and so deleting
//! an exchange knows what to delete without a search.
//!
//! A saved request's body goes in the same directory under
//! `<saved request id>.saved`, written and published the same way. It is a
//! third name rather than a third mechanism, and the name is what keeps the
//! two apart: clearing the captured traffic walks this directory and removes
//! the message files, and a saved request's body is not one of them.
//!
//! # Writing without leaving a dangling reference
//!
//! A file is written under a temporary name, and only renamed into the name
//! the database references once the rows are in an open transaction:
//!
//! ```text
//! write <id>.request.<unique>.tmp
//! insert the rows
//! rename it to <id>.request
//! commit
//! ```
//!
//! So a file that exists under its real name is a complete file, a
//! transaction that fails leaves no file behind (the temporary names are
//! removed, and so is anything already renamed), and a committed row always
//! points at a file that is there.
//!
//! The one thing this ordering cannot survive is the process dying between
//! the rename and the commit, which leaves a body file that no row
//! references. It wastes the space of one body and is otherwise harmless;
//! nothing reads a body file except through the row that names it.

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::body::BodyCapture;
use crate::saved::SavedRequestId;
use crate::stream::StreamMessageId;
use crate::traffic::ExchangeId;

use super::config::StorageConfig;
use super::encoding::MessageRole;
use super::error::StorageError;

/// The suffix a body file wears while it is being written.
const PARTIAL_SUFFIX: &str = "tmp";

/// Where a captured body's bytes are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Placement {
    /// Nothing was captured, so there is nothing to keep.
    Nothing,
    /// The bytes are in the database.
    Inline(Vec<u8>),
    /// The bytes are in this file, in the body directory.
    File(String),
}

impl Placement {
    /// The `body_inline` column's value.
    pub(super) fn inline(&self) -> Option<&[u8]> {
        match self {
            Self::Inline(data) => Some(data),
            Self::Nothing | Self::File(_) => None,
        }
    }

    /// The `body_file` column's value.
    pub(super) fn file(&self) -> Option<&str> {
        match self {
            Self::File(name) => Some(name),
            Self::Nothing | Self::Inline(_) => None,
        }
    }
}

/// The name of the file a message's body would be stored in.
fn file_name(id: ExchangeId, role: MessageRole) -> String {
    format!("{id}.{}", role.as_column())
}

/// The suffix a saved request's body file wears.
const SAVED_SUFFIX: &str = "saved";

/// The name of the file a saved request's body would be stored in.
fn saved_file_name(id: SavedRequestId) -> String {
    format!("{id}.{SAVED_SUFFIX}")
}

/// A body file written under a temporary name, waiting to be renamed.
#[derive(Debug)]
struct Staged {
    partial: PathBuf,
    final_path: PathBuf,
    renamed: Cell<bool>,
}

/// The files one staging wrote, and the two things that can happen to them.
///
/// Held between staging and the commit that makes the rows real. Everything
/// it wrote can be taken back, which is what stops a failed insert from
/// leaving a file nothing points at.
#[derive(Debug, Default)]
struct StagedFiles {
    files: Vec<Staged>,
}

impl StagedFiles {
    /// Renames every staged file to the name the rows reference.
    ///
    /// Called with the rows inserted and the transaction still open, so a
    /// failure here can still take the whole insert back.
    fn promote(&self) -> Result<(), StorageError> {
        for file in &self.files {
            fs::rename(&file.partial, &file.final_path)
                .map_err(|cause| StorageError::body(&file.final_path, cause))?;
            file.renamed.set(true);
        }

        Ok(())
    }

    /// Removes everything this staging wrote.
    ///
    /// Best effort, and deliberately so: it runs when the insert has already
    /// failed, and a file that cannot be removed is not a reason to lose the
    /// error that explains why. A file that was never renamed is removed
    /// under its temporary name; one that was is removed under its real name.
    /// Nothing else is touched, so a failed insert of an exchange whose id is
    /// already stored cannot remove the stored exchange's bodies.
    fn abandon(&self) {
        for file in &self.files {
            let _ = fs::remove_file(&file.partial);

            if file.renamed.get() {
                let _ = fs::remove_file(&file.final_path);
            }
        }
    }
}

/// The body files of one exchange, written but not yet referenced.
#[derive(Debug)]
pub(super) struct StagedBodies {
    request: Placement,
    response: Placement,
    staged: StagedFiles,
}

impl StagedBodies {
    /// Where the request's captured bytes are to be recorded as being.
    pub(super) fn request(&self) -> &Placement {
        &self.request
    }

    /// Where the response's captured bytes are to be recorded as being.
    pub(super) fn response(&self) -> &Placement {
        &self.response
    }

    /// Renames every staged file to the name the rows reference.
    pub(super) fn promote(&self) -> Result<(), StorageError> {
        self.staged.promote()
    }

    /// Removes everything this staging wrote.
    pub(super) fn abandon(&self) {
        self.staged.abandon();
    }
}

/// The body file of one saved request, written but not yet referenced.
///
/// The same two moves as [`StagedBodies`], for a row in another table: a
/// saved request is written the same way an exchange is, so a failed save
/// leaves neither a row pointing at a file that is not there nor a file
/// nothing points at.
#[derive(Debug)]
pub(super) struct StagedSavedBody {
    placement: Placement,
    staged: StagedFiles,
}

impl StagedSavedBody {
    /// Where the saved request's bytes are to be recorded as being.
    pub(super) fn placement(&self) -> &Placement {
        &self.placement
    }

    /// Renames the staged file to the name the row references.
    pub(super) fn promote(&self) -> Result<(), StorageError> {
        self.staged.promote()
    }

    /// Removes whatever this staging wrote.
    pub(super) fn abandon(&self) {
        self.staged.abandon();
    }
}

/// Writes whatever of `request` and `response` needs a file of its own, and
/// says where each of them will be.
///
/// Runs before the database is touched: writing a body is filesystem work,
/// and holding the database while doing it would stall every other exchange.
pub(super) fn stage(
    config: &StorageConfig,
    id: ExchangeId,
    request: &BodyCapture,
    response: Option<&BodyCapture>,
) -> Result<StagedBodies, StorageError> {
    let mut staged = StagedBodies {
        request: Placement::Nothing,
        response: Placement::Nothing,
        staged: StagedFiles::default(),
    };

    match place(
        config,
        file_name(id, MessageRole::Request),
        request,
        &mut staged.staged,
    ) {
        Ok(placement) => staged.request = placement,
        Err(error) => {
            staged.abandon();
            return Err(error);
        }
    }

    if let Some(response) = response {
        match place(
            config,
            file_name(id, MessageRole::Response),
            response,
            &mut staged.staged,
        ) {
            Ok(placement) => staged.response = placement,
            Err(error) => {
                staged.abandon();
                return Err(error);
            }
        }
    }

    Ok(staged)
}

/// Writes a saved request's body if it needs a file of its own, and says
/// where it will be.
///
/// The same three answers a captured body gets — nowhere, in the database, or
/// in a file — decided by the same limit.
pub(super) fn stage_saved(
    config: &StorageConfig,
    id: SavedRequestId,
    body: &BodyCapture,
) -> Result<StagedSavedBody, StorageError> {
    let mut staged = StagedSavedBody {
        placement: Placement::Nothing,
        staged: StagedFiles::default(),
    };

    match place(config, saved_file_name(id), body, &mut staged.staged) {
        Ok(placement) => staged.placement = placement,
        Err(error) => {
            staged.abandon();
            return Err(error);
        }
    }

    Ok(staged)
}

/// Writes a stream message's payload if it needs a file of its own.
///
/// The same three answers a captured body gets, decided by the same limit.
/// The file is named `<message id>.stream`, so clearing traffic can tell it
/// from a saved request's body.
pub(super) fn stage_stream(
    config: &StorageConfig,
    id: StreamMessageId,
    body: &BodyCapture,
) -> Result<StagedSavedBody, StorageError> {
    let mut staged = StagedSavedBody {
        placement: Placement::Nothing,
        staged: StagedFiles::default(),
    };

    match place(config, format!("{id}.stream"), body, &mut staged.staged) {
        Ok(placement) => staged.placement = placement,
        Err(error) => {
            staged.abandon();
            return Err(error);
        }
    }

    Ok(staged)
}

/// Decides where one captured body goes, and writes the file if it needs one.
fn place(
    config: &StorageConfig,
    name: String,
    body: &BodyCapture,
    staged: &mut StagedFiles,
) -> Result<Placement, StorageError> {
    let data = body.data();

    if data.is_empty() {
        return Ok(Placement::Nothing);
    }

    if data.len() <= config.inline_body_max_bytes {
        return Ok(Placement::Inline(data.to_vec()));
    }

    let final_path = config.bodies.join(&name);
    // Unique per attempt, so a second attempt at an exchange that is already
    // stored writes its own temporary file and cannot disturb the stored
    // exchange's.
    let partial = config
        .bodies
        .join(format!("{name}.{}.{PARTIAL_SUFFIX}", Uuid::now_v7()));

    write_file(&partial, data)?;
    staged.files.push(Staged {
        partial,
        final_path,
        renamed: Cell::new(false),
    });

    Ok(Placement::File(name))
}

/// Writes `data` to `path`, and makes sure it is all there.
///
/// Flushed and synced before the name is handed on, so the file the database
/// comes to reference is the whole body and not as much of it as the
/// operating system had got round to.
fn write_file(path: &Path, data: &[u8]) -> Result<(), StorageError> {
    let mut file = fs::File::create(path).map_err(|cause| StorageError::body(path, cause))?;

    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|cause| StorageError::body(path, cause))
}

/// The bytes of a stored body file.
pub(super) fn read(config: &StorageConfig, name: &str) -> Result<Vec<u8>, StorageError> {
    let path = config.bodies.join(name);

    fs::read(&path).map_err(|cause| StorageError::body(&path, cause))
}

/// Removes the body files an exchange had, if it had any.
///
/// Best effort: it runs after the rows that referenced them are gone, so a
/// file that will not go away is wasted space and not a broken database.
pub(super) fn remove(config: &StorageConfig, names: &[String]) {
    for name in names {
        let _ = fs::remove_file(config.bodies.join(name));
    }
}

/// Removes every captured body file in the body directory.
///
/// Only the files a captured exchange wrote: a name is a stored exchange's
/// id followed by `.request` or `.response`, and anything else in the
/// directory is left alone. Files still being written — which wear a
/// temporary name — are left alone too, because they belong to an insert that
/// is still in progress, and so are saved requests' bodies, which are not
/// traffic and do not go away when the traffic does.
pub(super) fn clear(config: &StorageConfig) -> Result<(), StorageError> {
    let entries = match fs::read_dir(&config.bodies) {
        Ok(entries) => entries,
        // Nothing has been stored yet, so there is nothing to clear.
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(cause) => return Err(StorageError::body(&config.bodies, cause)),
    };

    for entry in entries {
        let entry = entry.map_err(|cause| StorageError::body(&config.bodies, cause))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };

        if is_body_file(name) {
            let _ = fs::remove_file(entry.path());
        }
    }

    Ok(())
}

/// Whether `name` is a body file this crate wrote.
fn is_body_file(name: &str) -> bool {
    let Some((id, role)) = name.rsplit_once('.') else {
        return false;
    };

    if MessageRole::from_column(role).is_ok() && id.parse::<ExchangeId>().is_ok() {
        return true;
    }

    role == "stream" && id.parse::<StreamMessageId>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// A configuration in a directory of its own, with an inline limit of
    /// `inline` bytes.
    fn config(directory: &TempDir, inline: usize) -> StorageConfig {
        let config =
            StorageConfig::in_directory(directory.path()).with_inline_body_max_bytes(inline);
        fs::create_dir_all(&config.bodies).expect("a body directory");

        config
    }

    fn id() -> ExchangeId {
        ExchangeId::next()
    }

    fn body_files(config: &StorageConfig) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&config.bodies)
            .expect("a readable body directory")
            .map(|entry| {
                entry
                    .expect("a readable entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();

        names
    }

    #[test]
    fn a_body_with_nothing_captured_is_stored_nowhere() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 1_024);

        let staged = stage(&config, id(), &BodyCapture::empty(), None).expect("stages");

        assert_eq!(staged.request(), &Placement::Nothing);
        assert_eq!(staged.response(), &Placement::Nothing);
        assert!(body_files(&config).is_empty());
    }

    #[test]
    fn a_body_within_the_inline_limit_is_stored_in_the_database() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 1_024);

        let staged = stage(
            &config,
            id(),
            &BodyCapture::complete(b"hello".to_vec()),
            None,
        )
        .expect("stages");

        assert_eq!(staged.request(), &Placement::Inline(b"hello".to_vec()));
        assert_eq!(staged.request().file(), None);
        assert!(body_files(&config).is_empty());
    }

    #[test]
    fn a_body_exactly_at_the_inline_limit_is_still_stored_in_the_database() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 5);

        let staged = stage(
            &config,
            id(),
            &BodyCapture::complete(b"hello".to_vec()),
            None,
        )
        .expect("stages");

        assert!(matches!(staged.request(), Placement::Inline(_)));
    }

    #[test]
    fn a_body_over_the_inline_limit_is_stored_in_a_file_of_its_own() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 4);
        let id = id();

        let staged = stage(
            &config,
            id,
            &BodyCapture::complete(b"hello".to_vec()),
            Some(&BodyCapture::complete(b"answer".to_vec())),
        )
        .expect("stages");

        assert_eq!(staged.request(), &Placement::File(format!("{id}.request")));
        assert_eq!(
            staged.response(),
            &Placement::File(format!("{id}.response"))
        );
        // Written, but not yet under the names the rows will reference.
        assert!(
            body_files(&config)
                .iter()
                .all(|name| name.ends_with(".tmp")),
            "{:?}",
            body_files(&config)
        );

        staged.promote().expect("renames");

        assert_eq!(
            body_files(&config),
            [format!("{id}.request"), format!("{id}.response")]
        );
        assert_eq!(
            read(&config, &format!("{id}.request")).expect("reads"),
            b"hello"
        );
        assert_eq!(
            read(&config, &format!("{id}.response")).expect("reads"),
            b"answer"
        );
    }

    #[test]
    fn a_file_backed_body_keeps_its_bytes_exactly() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);
        let id = id();
        let bytes: Vec<u8> = (0..=255).cycle().take(4_096).collect();

        let staged =
            stage(&config, id, &BodyCapture::complete(bytes.clone()), None).expect("stages");
        staged.promote().expect("renames");

        assert_eq!(
            read(&config, &format!("{id}.request")).expect("reads"),
            bytes
        );
    }

    #[test]
    fn an_abandoned_staging_leaves_nothing_behind() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);

        let staged = stage(
            &config,
            id(),
            &BodyCapture::complete(b"hello".to_vec()),
            Some(&BodyCapture::complete(b"answer".to_vec())),
        )
        .expect("stages");
        staged.abandon();

        assert!(body_files(&config).is_empty());
    }

    #[test]
    fn an_abandoned_staging_that_was_already_renamed_leaves_nothing_behind() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);

        let staged = stage(
            &config,
            id(),
            &BodyCapture::complete(b"hello".to_vec()),
            None,
        )
        .expect("stages");
        staged.promote().expect("renames");
        staged.abandon();

        assert!(body_files(&config).is_empty());
    }

    #[test]
    fn staging_into_a_body_directory_that_is_not_there_fails_without_panicking() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = StorageConfig::in_directory(directory.path().join("missing"))
            .with_inline_body_max_bytes(0);

        let error = stage(
            &config,
            id(),
            &BodyCapture::complete(b"hello".to_vec()),
            None,
        )
        .expect_err("there is nowhere to write the body");

        assert!(matches!(error, StorageError::Body { .. }), "{error:?}");
    }

    #[test]
    fn reading_a_body_file_that_is_not_there_fails_without_panicking() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);

        let error = read(&config, "0199c0de-0000-7000-8000-00000000002a.request")
            .expect_err("no such body file");

        assert!(matches!(error, StorageError::Body { .. }), "{error:?}");
    }

    #[test]
    fn removing_a_body_file_that_is_not_there_is_not_a_failure() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);

        remove(&config, &["nothing.request".to_owned()]);
    }

    #[test]
    fn clearing_removes_body_files_and_leaves_everything_else() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = config(&directory, 0);
        let stored = id();
        let staged = stage(
            &config,
            stored,
            &BodyCapture::complete(b"hello".to_vec()),
            Some(&BodyCapture::complete(b"answer".to_vec())),
        )
        .expect("stages");
        staged.promote().expect("renames");
        fs::write(config.bodies.join("notes.txt"), b"not ours").expect("a stray file");
        let in_progress = stage(
            &config,
            id(),
            &BodyCapture::complete(b"still going".to_vec()),
            None,
        )
        .expect("stages");

        clear(&config).expect("clears");

        let left = body_files(&config);
        assert!(left.contains(&"notes.txt".to_owned()), "{left:?}");
        assert!(
            left.iter().any(|name| name.ends_with(".tmp")),
            "an insert in progress lost its body file: {left:?}"
        );
        assert!(
            !left.contains(&format!("{stored}.request")),
            "a stored body file survived the clear: {left:?}"
        );
        drop(in_progress);
    }

    #[test]
    fn clearing_a_body_directory_that_is_not_there_is_not_a_failure() {
        let directory = TempDir::new().expect("a temporary directory");
        let config = StorageConfig::in_directory(directory.path().join("missing"));

        clear(&config).expect("nothing to clear");
    }

    #[test]
    fn only_this_crates_own_body_files_are_recognised() {
        let stored = ExchangeId::next().to_string();

        assert!(is_body_file(&format!("{stored}.request")));
        assert!(is_body_file(&format!("{stored}.response")));
        assert!(is_body_file(&format!("{}.stream", StreamMessageId::next())));
        assert!(!is_body_file(&format!("{stored}.request.abc.tmp")));
        assert!(!is_body_file(&format!("{stored}.trailer")));
        assert!(!is_body_file("notes.txt"));
        assert!(!is_body_file("request"));
    }
}
