//! Keeping saved requests, so they outlive the process that was told about
//! them.
//!
//! ```text
//! SavedRequest ──▶ one row in `saved_requests` ──▶ SavedRequest
//!                  + a body, inline or in a file   validated on the way back
//! ```
//!
//! The shape of `rules`, with the body handling of `messages`. A saved
//! request is written by a person and read back whenever the list is shown,
//! so everything that comes out of the database goes through
//! [`SavedRequest::validate`]: a row edited by hand cannot become a nameless
//! entry in somebody's list.
//!
//! # The body goes where a captured one goes
//!
//! Nothing, a `BLOB`, or a file in the body directory, decided by the same
//! `inline_body_max_bytes` and written by the same [`bodies`](super::bodies)
//! module — under `<saved request id>.saved`, so clearing the captured
//! traffic, which removes `<exchange id>.request` and `.response`, leaves a
//! saved request's body exactly where it is.
//!
//! # Nothing here is on the request path
//!
//! These are CRUD operations, called when a person saves, renames, edits, or
//! deletes one. A request the proxy is carrying never reaches this module,
//! and neither does a replay: replaying a saved request is the app reading it
//! and then asking for the request to be sent, which is the replay every
//! other request gets.

use rusqlite::{OptionalExtension, Row};

use crate::saved::{SavedRequest, SavedRequestId, SavedRequestUpdate};

use super::bodies::{self, Placement};
use super::encoding;
use super::error::StorageError;
use super::store::Storage;

/// The columns of a saved request's row, in the order [`read`] expects them.
const SAVED_COLUMNS: &str = "\
    id, name, method, url, headers, body_size, body_truncated, body_inline, body_file, \
    created_at_ms, updated_at_ms";

/// Stores one saved request.
const INSERT_SAVED: &str = "\
    INSERT INTO saved_requests (
        id, name, method, url, headers, body_size, body_truncated, body_inline, body_file,
        created_at_ms, updated_at_ms
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

/// Replaces the parts of one saved request that can change.
///
/// Neither the id nor `created_at_ms` is among them: a saved request keeps
/// its identity and the moment it was saved for as long as it exists.
const UPDATE_SAVED: &str = "\
    UPDATE saved_requests SET
        name = ?2, method = ?3, url = ?4, headers = ?5, body_size = ?6, body_truncated = ?7,
        body_inline = ?8, body_file = ?9, updated_at_ms = ?10
    WHERE id = ?1";

impl Storage {
    /// Stores a new saved request.
    ///
    /// The order is the one an exchange is stored in, and for the same
    /// reason: the body file is written under a temporary name, the row goes
    /// in, the file is renamed to what the row references, and only then is
    /// anything committed. A failure takes the whole thing back.
    ///
    /// Fails if a request with that id is already stored, and takes nothing
    /// away from the one that is there.
    pub fn insert_saved_request(&self, saved: &SavedRequest) -> Result<(), StorageError> {
        let config = self.config().clone();
        let staged = bodies::stage_saved(&config, saved.id, &saved.body)?;

        let stored = self.with_connection(|connection| {
            let transaction = connection.transaction()?;

            transaction.execute(
                INSERT_SAVED,
                rusqlite::params_from_iter(encode(saved, staged.placement())?),
            )?;
            staged.promote()?;
            transaction.commit()?;

            Ok(())
        });

        if stored.is_err() {
            staged.abandon();
        }

        stored
    }

    /// Reads one saved request back, or `None` if there is no such request.
    pub fn saved_request(&self, id: SavedRequestId) -> Result<Option<SavedRequest>, StorageError> {
        let sql = format!("SELECT {SAVED_COLUMNS} FROM saved_requests WHERE id = ?1");

        let stored = self.with_connection(|connection| {
            let stored = connection
                .query_row(&sql, [id.as_text()], read)
                .optional()?;

            Ok(stored)
        })?;

        stored.map(|row| self.decode(row)).transpose()
    }

    /// Every saved request, in the order they were saved.
    ///
    /// `created_at_ms ASC, id ASC`, which is what the `saved_requests_order`
    /// index is on: the order a list is shown in is one ordered scan, and two
    /// requests saved in the same millisecond keep the same order for ever.
    pub fn list_saved_requests(&self) -> Result<Vec<SavedRequest>, StorageError> {
        let sql = format!(
            "SELECT {SAVED_COLUMNS} FROM saved_requests ORDER BY created_at_ms ASC, id ASC"
        );

        let stored = self.with_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map([], read)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows)
        })?;

        stored.into_iter().map(|row| self.decode(row)).collect()
    }

    /// How many saved requests are stored.
    pub fn count_saved_requests(&self) -> Result<u64, StorageError> {
        self.with_connection(|connection| {
            let count: i64 =
                connection
                    .query_row("SELECT COUNT(*) FROM saved_requests", [], |row| row.get(0))?;

            Ok(count.max(0) as u64)
        })
    }

    /// Applies `update` to the saved request `id` names, and returns what it
    /// became.
    ///
    /// `None` if there is no such request. The body is staged before the
    /// transaction and promoted inside it, exactly as an insert does, so an
    /// edit that replaces a large body with another large one never leaves a
    /// row pointing at a file that is not there.
    ///
    /// The file it replaces is removed afterwards, and only when the new body
    /// went somewhere else: a body that stayed in a file of the same name has
    /// already been overwritten by the rename.
    pub fn update_saved_request(
        &self,
        id: SavedRequestId,
        update: SavedRequestUpdate,
    ) -> Result<Option<SavedRequest>, StorageError> {
        let Some(current) = self.saved_request(id)? else {
            return Ok(None);
        };
        let updated = current
            .updated(update)
            .map_err(|_| StorageError::unreadable("saved request"))?;

        let config = self.config().clone();
        let staged = bodies::stage_saved(&config, updated.id, &updated.body)?;
        let replaced = previous_file(self.stored_body_file(id)?, staged.placement());

        let stored = self.with_connection(|connection| {
            let transaction = connection.transaction()?;

            let rows = transaction.execute(
                UPDATE_SAVED,
                rusqlite::params_from_iter(encode_update(&updated, staged.placement())?),
            )?;
            if rows == 0 {
                // Removed between the read and the write. Nothing is written
                // back: a saved request somebody deleted should not reappear
                // because somebody else was renaming it.
                return Ok(false);
            }

            staged.promote()?;
            transaction.commit()?;

            Ok(true)
        });

        match stored {
            Ok(true) => {
                bodies::remove(&config, &replaced);

                Ok(Some(updated))
            }
            Ok(false) => {
                staged.abandon();

                Ok(None)
            }
            Err(error) => {
                staged.abandon();

                Err(error)
            }
        }
    }

    /// Removes one saved request, and says whether there was one to remove.
    ///
    /// The row goes first and its body file afterwards, so an interruption
    /// can leave a body file that nothing references — wasted space — and
    /// never a row that references a file which is gone.
    pub fn delete_saved_request(&self, id: SavedRequestId) -> Result<bool, StorageError> {
        let file: Vec<String> = self.stored_body_file(id)?.into_iter().collect();

        let removed = self.with_connection(|connection| {
            let removed =
                connection.execute("DELETE FROM saved_requests WHERE id = ?1", [id.as_text()])?;

            Ok(removed > 0)
        })?;

        if removed {
            let config = self.config().clone();
            bodies::remove(&config, &file);
        }

        Ok(removed)
    }

    /// The body file the stored row references, if it references one.
    ///
    /// Read from the column rather than worked out from the request: whether
    /// a body went to a file or into the row is a decision the size limit
    /// made when it was written, and the row is where that decision is
    /// recorded.
    fn stored_body_file(&self, id: SavedRequestId) -> Result<Option<String>, StorageError> {
        self.with_connection(|connection| {
            let file = connection
                .query_row(
                    "SELECT body_file FROM saved_requests WHERE id = ?1",
                    [id.as_text()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();

            Ok(file)
        })
    }

    /// Puts one row back together, reading its body file if it had one.
    fn decode(&self, stored: StoredSavedRequest) -> Result<SavedRequest, StorageError> {
        let config = self.config().clone();
        let data = match (stored.inline, &stored.file) {
            (Some(data), _) => data,
            (None, Some(name)) => bodies::read(&config, name)?,
            (None, None) => Vec::new(),
        };

        let saved = SavedRequest {
            id: stored
                .id
                .parse()
                .map_err(|_| StorageError::unreadable("saved request id"))?,
            name: stored.name,
            method: stored.method,
            url: stored.url,
            headers: encoding::headers_from_column(&stored.headers)?,
            body: encoding::body_from_columns(data, stored.body_size, stored.body_truncated != 0),
            created_at_ms: stored.created_at_ms.max(0) as u64,
            updated_at_ms: stored.updated_at_ms.max(0) as u64,
        };
        saved
            .validate()
            .map_err(|_| StorageError::unreadable("saved request"))?;

        Ok(saved)
    }
}

/// A saved request's row as the database holds it, before anything is made of
/// it.
///
/// The strings are still strings here on purpose: turning them into a model
/// inside a `rusqlite` row reader would report a request this build cannot
/// read as a database error, which is not what it is.
struct StoredSavedRequest {
    id: String,
    name: String,
    method: String,
    url: String,
    headers: String,
    body_size: i64,
    body_truncated: i64,
    inline: Option<Vec<u8>>,
    file: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

/// Reads one row into the strings and numbers it holds.
fn read(row: &Row<'_>) -> rusqlite::Result<StoredSavedRequest> {
    Ok(StoredSavedRequest {
        id: row.get(0)?,
        name: row.get(1)?,
        method: row.get(2)?,
        url: row.get(3)?,
        headers: row.get(4)?,
        body_size: row.get(5)?,
        body_truncated: row.get(6)?,
        inline: row.get(7)?,
        file: row.get(8)?,
        created_at_ms: row.get(9)?,
        updated_at_ms: row.get(10)?,
    })
}

/// The values one saved request is stored as.
fn encode(
    saved: &SavedRequest,
    placement: &Placement,
) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    use rusqlite::types::Value;

    Ok(vec![
        Value::Text(saved.id.as_text()),
        Value::Text(saved.name.clone()),
        Value::Text(saved.method.clone()),
        Value::Text(saved.url.clone()),
        Value::Text(encoding::headers_column(&saved.headers)?),
        Value::Integer(saved.body.size().min(i64::MAX as u64) as i64),
        Value::Integer(i64::from(saved.body.is_truncated())),
        placement
            .inline()
            .map_or(Value::Null, |data| Value::Blob(data.to_vec())),
        placement
            .file()
            .map_or(Value::Null, |name| Value::Text(name.to_owned())),
        Value::Integer(saved.created_at_ms as i64),
        Value::Integer(saved.updated_at_ms as i64),
    ])
}

/// The values an update writes, in the order [`UPDATE_SAVED`] binds them.
fn encode_update(
    saved: &SavedRequest,
    placement: &Placement,
) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    let mut values = encode(saved, placement)?;
    // The insert's order with `created_at_ms` taken out: an update never
    // writes the moment the request was saved.
    values.remove(9);

    Ok(values)
}

/// The body file an update leaves behind, if it leaves one behind.
///
/// Only when the new body did *not* go to a file: a body that is still in a
/// file wears the same name, and the rename that published it has already
/// replaced what was there.
fn previous_file(current: Option<String>, placement: &Placement) -> Vec<String> {
    match placement {
        Placement::File(_) => Vec::new(),
        Placement::Nothing | Placement::Inline(_) => current.into_iter().collect(),
    }
}
