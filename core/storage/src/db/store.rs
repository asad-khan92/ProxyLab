//! The traffic store: a database file, a body directory, and the operations
//! on them.
//!
//! ```text
//! Storage
//! ├── open      make the database, or bring an existing one up to date
//! ├── insert    store one finished exchange
//! ├── get       read one back whole
//! ├── list      a page of the traffic list, newest first
//! ├── count     how many are stored
//! ├── delete    remove one, and its body files
//! └── clear     remove all of them, and their body files
//! ```
//!
//! # Synchronous, on purpose
//!
//! SQLite is a library that does its work on the calling thread, and these
//! operations are the same. Nothing here spawns, awaits, or needs a runtime;
//! a caller inside one uses [`StorageWorker`](super::StorageWorker), which
//! owns a blocking thread and a bounded queue so no proxy task ever waits on
//! a database.
//!
//! # Two connections, one writer
//!
//! A [`Storage`] holds two connections to the same file, and cloning one
//! shares them. Writes — inserts, updates, deletes — take the writer.
//! Reads — a page, one exchange, a search — take the reader. SQLite allows
//! one writer, so the writer is a mutex; the reader is another, so two
//! reads still take turns with each other. They do not take turns with the
//! writer. The write-ahead log, which this store turns on when it opens, is
//! what makes that true: a search can be reading a body file for the row it
//! is considering while an insert commits.
//!
//! That split is the difference between a slow search and a proxy that drops
//! captured traffic. The storage worker is the writer. If it waited on the
//! same connection a search was using, its queue would fill and exchanges
//! would be dropped for as long as the search ran.
//!
//! Filesystem work is kept outside the writer. A body file is written before
//! the connection is taken and read after it is given back, so an exchange
//! with a large body does not hold the writer while its bytes go to disk.
//! The one exception is the rename that publishes a body file, which happens
//! with the transaction open because that is the whole point of it — see
//! [`bodies`](super::bodies). A search still reads a body file while it holds
//! the *reader*, one file at a time, because that read is how the row is
//! matched. It does not hold the writer.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row};

use crate::body::BodyCapture;
use crate::headers::Headers;
use crate::timing::Timing;
use crate::traffic::{
    CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId, TrafficExchange,
};

use super::bodies::{self, Placement};
use super::config::StorageConfig;
use super::encoding::{self, MessageRole};
use super::error::StorageError;
use super::query::{
    Cursor, ExchangeSummary, TrafficFilter, TrafficOrder, TrafficPage, TrafficQuery,
};
use super::schema;
use super::search;

/// The columns of an exchange row, in the order the row readers expect them.
const EXCHANGE_COLUMNS: &str = "\
    id, started_at_ms, started_at_subms_nanos, response_at_ms, duration_micros, \
    upstream_connect_micros, upstream_response_micros, client_addr, scheme, host, port, \
    completed, error_kind, error_summary, request_method, request_url, request_version, \
    response_status, response_reason, response_version, request_content_type, \
    response_content_type";

/// Stores one exchange.
const INSERT_EXCHANGE: &str = "\
    INSERT INTO exchanges (
        id, started_at_ms, started_at_subms_nanos, response_at_ms, duration_micros,
        upstream_connect_micros, upstream_response_micros, client_addr, scheme, host, port,
        completed, error_kind, error_summary, request_method, request_url, request_version,
        response_status, response_reason, response_version, request_content_type,
        response_content_type
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
        ?21, ?22
    )";

/// Stores one half of an exchange: its headers and its captured body.
const INSERT_MESSAGE: &str = "\
    INSERT INTO messages (
        exchange_id, role, headers, body_size, body_captured_len, body_truncated,
        body_inline, body_file
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

/// The same insert, replacing an exchange that was announced before it finished.
///
/// A WebSocket handshake and an event stream are stored as soon as their
/// response head is known, so the UI can open them while they are still
/// carrying traffic. The finished snapshot is the same id written again:
/// `ON CONFLICT` updates the row rather than deleting it, so stream
/// messages hanging off the foreign key are left alone.
const UPSERT_EXCHANGE: &str = "\
    INSERT INTO exchanges (
        id, started_at_ms, started_at_subms_nanos, response_at_ms, duration_micros,
        upstream_connect_micros, upstream_response_micros, client_addr, scheme, host, port,
        completed, error_kind, error_summary, request_method, request_url, request_version,
        response_status, response_reason, response_version, request_content_type,
        response_content_type
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
        ?21, ?22
    )
    ON CONFLICT(id) DO UPDATE SET
        started_at_ms = excluded.started_at_ms,
        started_at_subms_nanos = excluded.started_at_subms_nanos,
        response_at_ms = excluded.response_at_ms,
        duration_micros = excluded.duration_micros,
        upstream_connect_micros = excluded.upstream_connect_micros,
        upstream_response_micros = excluded.upstream_response_micros,
        client_addr = excluded.client_addr,
        scheme = excluded.scheme,
        host = excluded.host,
        port = excluded.port,
        completed = excluded.completed,
        error_kind = excluded.error_kind,
        error_summary = excluded.error_summary,
        request_method = excluded.request_method,
        request_url = excluded.request_url,
        request_version = excluded.request_version,
        response_status = excluded.response_status,
        response_reason = excluded.response_reason,
        response_version = excluded.response_version,
        request_content_type = excluded.request_content_type,
        response_content_type = excluded.response_content_type";

/// The same insert, replacing a message half that was stored with the head.
const UPSERT_MESSAGE: &str = "\
    INSERT INTO messages (
        exchange_id, role, headers, body_size, body_captured_len, body_truncated,
        body_inline, body_file
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
    ON CONFLICT(exchange_id, role) DO UPDATE SET
        headers = excluded.headers,
        body_size = excluded.body_size,
        body_captured_len = excluded.body_captured_len,
        body_truncated = excluded.body_truncated,
        body_inline = excluded.body_inline,
        body_file = excluded.body_file";

/// Reads back the messages of one exchange.
const SELECT_MESSAGES: &str = "\
    SELECT role, headers, body_size, body_truncated, body_inline, body_file \
     FROM messages WHERE exchange_id = ?1";

/// The persisted traffic history.
///
/// Cheap to clone: a clone is another handle to the same database and the
/// same body directory.
#[derive(Clone, Debug)]
pub struct Storage {
    inner: Arc<Inner>,
}

/// What every handle to one store shares.
#[derive(Debug)]
struct Inner {
    config: StorageConfig,
    /// Inserts, updates, and deletes.
    write: Mutex<Connection>,
    /// Lists, gets, and searches.
    read: Mutex<Connection>,
}

impl Storage {
    /// Opens the store `config` describes, creating what is not there.
    ///
    /// The database file and the body directory are created if they do not
    /// exist, and an existing database is brought up to the current schema.
    /// Opening the same database again — from this process or another — is
    /// expected and safe.
    pub fn open(config: StorageConfig) -> Result<Self, StorageError> {
        if let Some(parent) = config.database.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|cause| StorageError::directory(parent, cause))?;
            }
        }
        std::fs::create_dir_all(&config.bodies)
            .map_err(|cause| StorageError::directory(&config.bodies, cause))?;

        let mut write = open_connection(&config.database)?;
        schema::migrate(&mut write)?;
        search::register(&write, config.bodies.clone())?;

        // After the migration, so the reader opens a database that is already
        // at this build's schema. `query_only` makes a write on this
        // connection fail instead of accidentally taking the writer lock.
        let read = open_connection(&config.database)?;
        search::register(&read, config.bodies.clone())?;
        read.execute_batch("PRAGMA query_only = ON")?;

        Ok(Self {
            inner: Arc::new(Inner {
                config,
                write: Mutex::new(write),
                read: Mutex::new(read),
            }),
        })
    }

    /// Opens a store of ProxyLab's own naming inside `directory`.
    pub fn open_in(directory: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::open(StorageConfig::in_directory(directory))
    }

    /// Where this store keeps things.
    pub fn config(&self) -> &StorageConfig {
        &self.inner.config
    }

    /// The schema version the database is at.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        self.with_read(|connection| schema::version(connection))
    }

    /// Stores one finished exchange.
    ///
    /// The order is what keeps the database and the body files agreeing with
    /// each other:
    ///
    /// 1. write the body files that need writing, under temporary names;
    /// 2. insert the rows in a transaction;
    /// 3. rename the body files to the names those rows reference;
    /// 4. commit.
    ///
    /// Anything that fails takes the whole insert back, body files included,
    /// so a failure leaves neither a row pointing at a file that is not
    /// there nor a file nothing points at.
    ///
    /// Storing an exchange whose id is already stored fails — an id
    /// identifies one exchange — and takes nothing away from the exchange
    /// that is already there.
    pub fn insert(&self, exchange: &TrafficExchange) -> Result<(), StorageError> {
        let response_body = exchange.response.as_ref().map(|response| &response.body);
        let staged = bodies::stage(
            &self.inner.config,
            exchange.id,
            &exchange.request.body,
            response_body,
        )?;

        let stored = self.with_write(|connection| {
            let transaction = connection.transaction()?;

            transaction.execute(
                INSERT_EXCHANGE,
                rusqlite::params_from_iter(exchange_params(exchange)),
            )?;
            transaction.execute(
                INSERT_MESSAGE,
                rusqlite::params_from_iter(message_params(
                    exchange.id,
                    MessageRole::Request,
                    &exchange.request.headers,
                    &exchange.request.body,
                    staged.request(),
                )?),
            )?;
            if let Some(response) = &exchange.response {
                transaction.execute(
                    INSERT_MESSAGE,
                    rusqlite::params_from_iter(message_params(
                        exchange.id,
                        MessageRole::Response,
                        &response.headers,
                        &response.body,
                        staged.response(),
                    )?),
                )?;
            }

            // With the rows in place and the transaction still open: a
            // failure here is still a failure of the whole insert.
            staged.promote()?;
            transaction.commit()?;

            Ok(())
        });

        if stored.is_err() {
            staged.abandon();
        }

        stored
    }

    /// Stores `exchange`, or replaces the one already stored under its id.
    ///
    /// What an announced handshake and a later finished snapshot share: the
    /// first write creates the row, the second updates it, and neither
    /// touches the stream messages that already belong to it.
    pub fn upsert(&self, exchange: &TrafficExchange) -> Result<(), StorageError> {
        let response_body = exchange.response.as_ref().map(|response| &response.body);
        let staged = bodies::stage(
            &self.inner.config,
            exchange.id,
            &exchange.request.body,
            response_body,
        )?;

        let stored = self.with_write(|connection| {
            let transaction = connection.transaction()?;

            transaction.execute(
                UPSERT_EXCHANGE,
                rusqlite::params_from_iter(exchange_params(exchange)),
            )?;
            transaction.execute(
                UPSERT_MESSAGE,
                rusqlite::params_from_iter(message_params(
                    exchange.id,
                    MessageRole::Request,
                    &exchange.request.headers,
                    &exchange.request.body,
                    staged.request(),
                )?),
            )?;
            if let Some(response) = &exchange.response {
                transaction.execute(
                    UPSERT_MESSAGE,
                    rusqlite::params_from_iter(message_params(
                        exchange.id,
                        MessageRole::Response,
                        &response.headers,
                        &response.body,
                        staged.response(),
                    )?),
                )?;
            }

            staged.promote()?;
            transaction.commit()?;

            Ok(())
        });

        if stored.is_err() {
            staged.abandon();
        }

        stored
    }

    /// Reads one exchange back whole.
    ///
    /// Everything that was stored: the request and its headers and captured
    /// body, the response and its own if there was one, the timing, and the
    /// error that explains a missing response.
    pub fn get(&self, id: ExchangeId) -> Result<Option<TrafficExchange>, StorageError> {
        let query = format!("SELECT {EXCHANGE_COLUMNS} FROM exchanges WHERE id = ?1");
        let stored = self.with_read(|connection| {
            let Some((summary, timing)) = connection
                .query_row(&query, [id.as_text()], read_exchange_row)
                .optional()?
            else {
                return Ok(None);
            };

            let mut statement = connection.prepare(SELECT_MESSAGES)?;
            let messages = statement
                .query_map([id.as_text()], read_message_row)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Some((summary, timing, messages)))
        })?;

        let Some((summary, timing, messages)) = stored else {
            return Ok(None);
        };

        // Body files are read with the connection given back: the bytes of a
        // large body have nothing to do with the database.
        self.assemble(summary?, timing, messages).map(Some)
    }

    /// A page of the traffic list, in the order the query asked for.
    pub fn list(&self, query: &TrafficQuery) -> Result<TrafficPage, StorageError> {
        check_search(&query.filter)?;
        let limit = query.effective_limit();
        let (clause, mut values) = conditions(&query.filter, query.after.as_ref(), query.order);
        // The direction is one of two fixed strings chosen by an enum, never
        // anything a caller supplies: every value in this statement is bound.
        let direction = query.order.direction();
        let sql = format!(
            "SELECT {EXCHANGE_COLUMNS} FROM exchanges{clause} \
             ORDER BY started_at_ms {direction}, id {direction} LIMIT ?"
        );
        values.push(Value::Integer(limit as i64));

        let exchanges = self.with_read(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(values), read_exchange_row)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows)
        })?;

        let exchanges: Vec<ExchangeSummary> = exchanges
            .into_iter()
            .map(|(summary, _)| summary)
            .collect::<Result<Vec<_>, _>>()?;
        // A page that was not filled is the last one there is; a page that
        // was may have another behind it.
        let next = (exchanges.len() == limit)
            .then(|| exchanges.last().map(Cursor::of))
            .flatten();

        Ok(TrafficPage { exchanges, next })
    }

    /// How many exchanges are stored.
    pub fn count(&self) -> Result<u64, StorageError> {
        self.count_matching(&TrafficFilter::all())
    }

    /// How many stored exchanges `filter` matches.
    pub fn count_matching(&self, filter: &TrafficFilter) -> Result<u64, StorageError> {
        check_search(filter)?;
        // No cursor, so the order decides nothing here: a count is the same
        // however the rows would have been read.
        let (clause, values) = conditions(filter, None, TrafficOrder::default());
        let sql = format!("SELECT COUNT(*) FROM exchanges{clause}");

        self.with_read(|connection| {
            let count: i64 =
                connection.query_row(&sql, rusqlite::params_from_iter(values), |row| row.get(0))?;

            Ok(count.max(0) as u64)
        })
    }

    /// Removes one exchange, and whatever body files it had.
    ///
    /// Says whether there was one to remove. The rows go first and the files
    /// afterwards, so an interruption can leave a body file that nothing
    /// references — wasted space — and never a row that references a file
    /// which is gone.
    pub fn delete(&self, id: ExchangeId) -> Result<bool, StorageError> {
        let removed = self.with_write(|connection| {
            let transaction = connection.transaction()?;
            let mut files = body_files(&transaction, id)?;
            files.extend(super::stream::stream_files(&transaction, id)?);
            let rows =
                transaction.execute("DELETE FROM exchanges WHERE id = ?1", [id.as_text()])?;

            transaction.commit()?;

            Ok((rows > 0, files))
        })?;

        let (deleted, files) = removed;
        bodies::remove(&self.inner.config, &files);

        Ok(deleted)
    }

    /// Removes every stored exchange, and every body file.
    ///
    /// Says how many exchanges were removed.
    pub fn clear(&self) -> Result<u64, StorageError> {
        let removed = self.with_write(|connection| {
            let transaction = connection.transaction()?;
            // The messages go with their exchanges, by the foreign key.
            let rows = transaction.execute("DELETE FROM exchanges", [])?;

            transaction.commit()?;

            Ok(rows as u64)
        })?;

        bodies::clear(&self.inner.config)?;

        Ok(removed)
    }

    /// Runs `work` on the writer.
    ///
    /// A poisoned lock is recovered from rather than panicked on: something
    /// else already panicked, and a second panic here would take a proxy
    /// connection down over a piece of history-keeping.
    pub(super) fn with_write<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.with_locked(&self.inner.write, work)
    }

    /// Runs `work` on the reader.
    ///
    /// Same poisoning rule as [`with_write`](Self::with_write). A search holds
    /// this lock while it reads one body file; the writer is a different lock.
    pub(super) fn with_read<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        self.with_locked(&self.inner.read, work)
    }

    fn with_locked<T>(
        &self,
        connection: &Mutex<Connection>,
        work: impl FnOnce(&mut Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut connection: MutexGuard<'_, Connection> = connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        work(&mut connection)
    }

    /// Holds the reader until the guard is dropped.
    ///
    /// What a test uses to prove an insert does not wait for a read. Nothing
    /// else should: a real read is [`with_read`](Self::with_read).
    #[cfg(test)]
    fn lease_reader(&self) -> MutexGuard<'_, Connection> {
        self.inner
            .read
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Puts an exchange back together, reading any body files it referenced.
    fn assemble(
        &self,
        summary: ExchangeSummary,
        timing: Timing,
        messages: Vec<StoredMessage>,
    ) -> Result<TrafficExchange, StorageError> {
        let mut request = None;
        let mut response = None;

        for message in messages {
            let role = MessageRole::from_column(&message.role)?;
            let headers = encoding::headers_from_column(&message.headers)?;
            let data = match (message.inline, &message.file) {
                (Some(data), _) => data,
                (None, Some(name)) => bodies::read(&self.inner.config, name)?,
                (None, None) => Vec::new(),
            };
            let body = encoding::body_from_columns(data, message.size, message.truncated);

            match role {
                MessageRole::Request => request = Some((headers, body)),
                MessageRole::Response => response = Some((headers, body)),
            }
        }

        let (request_headers, request_body) =
            request.ok_or_else(|| StorageError::unreadable("request message"))?;

        let response = match summary.status {
            Some(status) => {
                let (headers, body) =
                    response.ok_or_else(|| StorageError::unreadable("response message"))?;

                Some(CapturedResponse {
                    status,
                    reason: summary.reason,
                    version: summary.response_version.unwrap_or_default(),
                    headers,
                    body,
                })
            }
            None => None,
        };

        Ok(TrafficExchange {
            id: summary.id,
            connection: ConnectionInfo {
                client: summary.client,
                scheme: summary.scheme,
                destination: summary.destination,
            },
            request: CapturedRequest {
                method: summary.method,
                url: summary.url,
                version: summary.version,
                headers: request_headers,
                body: request_body,
            },
            response,
            error: summary.error,
            timing,
        })
    }
}

/// Opens `database` and applies the pragmas every connection needs.
fn open_connection(database: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open(database)?;
    prepare(&connection)?;

    Ok(connection)
}

/// Sets the connection up before anything is asked of it.
///
/// - foreign keys, which SQLite leaves off by default and which are what
///   make a deleted exchange take its messages with it;
/// - the write-ahead log, so a second process can read the database while
///   the proxy writes it. A filesystem that cannot do it leaves the journal
///   mode as it was, which costs concurrency and nothing else, so the mode
///   that comes back is not checked;
/// - `synchronous = NORMAL`, the setting the write-ahead log is meant to be
///   used with: durable across a crash of this process, and not paying for a
///   flush per transaction to also be durable across a power cut. Captured
///   traffic is a debugging record, not a ledger;
/// - a busy timeout, so a connection that finds the database locked by
///   another process waits for it instead of failing immediately.
fn prepare(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    connection.execute_batch("PRAGMA synchronous = NORMAL")?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;

    Ok(())
}

/// The values of one exchange row.
fn exchange_params(exchange: &TrafficExchange) -> Vec<Value> {
    let timing = &exchange.timing;
    let (started_at_ms, started_at_subms_nanos) = encoding::timestamp_columns(timing.started_at);
    let destination = exchange.connection.destination.as_ref();
    let response = exchange.response.as_ref();
    let error = exchange.error.as_ref();

    vec![
        Value::Text(exchange.id.as_text()),
        Value::Integer(started_at_ms),
        Value::Integer(started_at_subms_nanos),
        optional_integer(encoding::response_at_ms(
            timing.started_at,
            timing.upstream_response_micros,
        )),
        optional_integer(encoding::micros_column(timing.completed_micros)),
        optional_integer(encoding::micros_column(timing.upstream_connect_micros)),
        optional_integer(encoding::micros_column(timing.upstream_response_micros)),
        Value::Text(exchange.connection.client.to_string()),
        Value::Text(encoding::scheme_column(exchange.connection.scheme).to_owned()),
        optional_text(destination.map(|destination| destination.host.clone())),
        optional_integer(destination.map(|destination| i64::from(destination.port))),
        Value::Integer(i64::from(timing.completed_micros.is_some())),
        optional_text(error.map(|error| encoding::error_kind_column(error.kind()).to_owned())),
        optional_text(error.map(|error| error.summary().to_owned())),
        Value::Text(exchange.request.method.clone()),
        Value::Text(exchange.request.url.clone()),
        Value::Text(encoding::version_column(exchange.request.version).to_owned()),
        optional_integer(response.map(|response| i64::from(response.status))),
        optional_text(response.and_then(|response| response.reason.clone())),
        optional_text(
            response.map(|response| encoding::version_column(response.version).to_owned()),
        ),
        optional_text(
            exchange
                .request
                .headers
                .text("content-type")
                .map(str::to_owned),
        ),
        optional_text(
            response
                .and_then(|response| response.headers.text("content-type"))
                .map(str::to_owned),
        ),
    ]
}

/// The values of one message row.
fn message_params(
    id: ExchangeId,
    role: MessageRole,
    headers: &Headers,
    body: &BodyCapture,
    placement: &Placement,
) -> Result<Vec<Value>, StorageError> {
    Ok(vec![
        Value::Text(id.as_text()),
        Value::Text(role.as_column().to_owned()),
        Value::Text(encoding::headers_column(headers)?),
        Value::Integer(i64::try_from(body.size()).unwrap_or(i64::MAX)),
        Value::Integer(i64::try_from(body.captured_len()).unwrap_or(i64::MAX)),
        Value::Integer(i64::from(body.is_truncated())),
        match placement.inline() {
            Some(data) => Value::Blob(data.to_vec()),
            None => Value::Null,
        },
        optional_text(placement.file().map(str::to_owned)),
    ])
}

/// A value that may not be there.
fn optional_text(text: Option<String>) -> Value {
    text.map_or(Value::Null, Value::Text)
}

/// A number that may not be there.
fn optional_integer(number: Option<i64>) -> Value {
    number.map_or(Value::Null, Value::Integer)
}

/// One exchange row, as the list and the reader both want it.
///
/// The summary is what a traffic list shows; the timing is the rest of what
/// the row holds, which only a whole exchange needs.
///
/// The summary comes back unresolved because the two failures are different
/// kinds. A row that SQLite cannot produce is a database error, which is
/// rusqlite's to report; a row whose columns this build cannot make sense of
/// is a [`StorageError::Unreadable`], which is this crate's. Keeping them
/// apart is what stops the second from having to be dressed up as the
/// first.
type ReadRow = (Result<ExchangeSummary, StorageError>, Timing);

/// Reads one exchange row.
fn read_exchange_row(row: &Row<'_>) -> rusqlite::Result<ReadRow> {
    let started_at = encoding::timestamp_from_columns(row.get(1)?, row.get(2)?);
    let timing = Timing {
        started_at,
        upstream_connect_micros: encoding::micros_from_column(row.get(5)?),
        upstream_response_micros: encoding::micros_from_column(row.get(6)?),
        completed_micros: encoding::micros_from_column(row.get(4)?),
    };
    let response_at: Option<i64> = row.get(3)?;
    let id: String = row.get(0)?;
    let client: String = row.get(7)?;
    let scheme: String = row.get(8)?;
    let host: Option<String> = row.get(9)?;
    let port: Option<i64> = row.get(10)?;
    let completed: i64 = row.get(11)?;
    let error_kind: Option<String> = row.get(12)?;
    let error_summary: Option<String> = row.get(13)?;
    let method: String = row.get(14)?;
    let url: String = row.get(15)?;
    let version: String = row.get(16)?;
    let status: Option<i64> = row.get(17)?;
    let reason: Option<String> = row.get(18)?;
    let response_version: Option<String> = row.get(19)?;
    let request_content_type: Option<String> = row.get(20)?;
    let response_content_type: Option<String> = row.get(21)?;

    Ok((
        summary_of(SummaryColumns {
            id,
            started_at,
            response_at,
            duration_micros: timing.completed_micros,
            client,
            scheme,
            host,
            port,
            completed: completed != 0,
            error_kind,
            error_summary,
            method,
            url,
            version,
            status,
            reason,
            response_version,
            request_content_type,
            response_content_type,
        }),
        timing,
    ))
}

/// The columns a summary is built from, as they came out of the row.
struct SummaryColumns {
    id: String,
    started_at: crate::timing::Timestamp,
    response_at: Option<i64>,
    duration_micros: Option<u64>,
    client: String,
    scheme: String,
    host: Option<String>,
    port: Option<i64>,
    completed: bool,
    error_kind: Option<String>,
    error_summary: Option<String>,
    method: String,
    url: String,
    version: String,
    status: Option<i64>,
    reason: Option<String>,
    response_version: Option<String>,
    request_content_type: Option<String>,
    response_content_type: Option<String>,
}

/// The summary those columns describe, or why they could not be read.
fn summary_of(columns: SummaryColumns) -> Result<ExchangeSummary, StorageError> {
    let destination = match (columns.host, columns.port) {
        (Some(host), Some(port)) => Some(Destination::new(
            host,
            u16::try_from(port).map_err(|_| StorageError::unreadable("destination port"))?,
        )),
        _ => None,
    };

    Ok(ExchangeSummary {
        id: columns
            .id
            .parse()
            .map_err(|_| StorageError::unreadable("exchange id"))?,
        started_at: columns.started_at,
        response_at: columns
            .response_at
            .map(|millis| encoding::timestamp_from_columns(millis, 0)),
        duration_micros: columns.duration_micros,
        client: encoding::client_from_column(&columns.client)?,
        scheme: encoding::scheme_from_column(&columns.scheme)?,
        destination,
        method: columns.method,
        url: columns.url,
        version: encoding::version_from_column(&columns.version),
        status: columns
            .status
            .map(|status| {
                u16::try_from(status).map_err(|_| StorageError::unreadable("response status"))
            })
            .transpose()?,
        reason: columns.reason,
        response_version: columns
            .response_version
            .as_deref()
            .map(encoding::version_from_column),
        request_content_type: columns.request_content_type,
        response_content_type: columns.response_content_type,
        completed: columns.completed,
        error: encoding::error_from_columns(columns.error_kind, columns.error_summary)?,
    })
}

/// One message row, before its body file has been read.
struct StoredMessage {
    role: String,
    headers: String,
    size: i64,
    truncated: bool,
    inline: Option<Vec<u8>>,
    file: Option<String>,
}

/// Reads one message row.
fn read_message_row(row: &Row<'_>) -> rusqlite::Result<StoredMessage> {
    let truncated: i64 = row.get(3)?;

    Ok(StoredMessage {
        role: row.get(0)?,
        headers: row.get(1)?,
        size: row.get(2)?,
        truncated: truncated != 0,
        inline: row.get(4)?,
        file: row.get(5)?,
    })
}

/// The body files one exchange's messages reference.
fn body_files(connection: &Connection, id: ExchangeId) -> Result<Vec<String>, StorageError> {
    let mut statement =
        connection.prepare("SELECT body_file FROM messages WHERE exchange_id = ?1")?;
    let files = statement
        .query_map([id.as_text()], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<Option<String>>, _>>()?;

    Ok(files.into_iter().flatten().collect())
}

/// Refuses a search that claimed to be a regular expression and is not.
fn check_search(filter: &TrafficFilter) -> Result<(), StorageError> {
    if filter.search_regex {
        if let Some(text) = &filter.text {
            crate::Needle::compile(text, true, filter.search_case_sensitive)
                .map_err(|_| StorageError::InvalidSearch)?;
        }
    }

    Ok(())
}

/// The `WHERE` clause a filter and a cursor come to, and its values.
///
/// Every value is a bound parameter: nothing a caller supplies is written
/// into the statement.
fn conditions(
    filter: &TrafficFilter,
    after: Option<&Cursor>,
    order: TrafficOrder,
) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();

    if let Some(host) = &filter.host {
        // The column is declared `COLLATE NOCASE`, so this is the
        // case-insensitive comparison host names deserve, and it is the
        // comparison the host index is built on.
        clauses.push("host = ?".to_owned());
        values.push(Value::Text(host.clone()));
    }

    if let Some(method) = &filter.method {
        clauses.push("request_method = ?".to_owned());
        values.push(Value::Text(method.clone()));
    }

    if let Some(status) = filter.status {
        let (first, last) = status.range();
        clauses.push("response_status BETWEEN ? AND ?".to_owned());
        values.push(Value::Integer(i64::from(first)));
        values.push(Value::Integer(i64::from(last)));
    }

    if let Some(from) = filter.started_at_or_after {
        let (millis, _) = encoding::timestamp_columns(from);
        clauses.push("started_at_ms >= ?".to_owned());
        values.push(Value::Integer(millis));
    }

    if let Some(until) = filter.started_before {
        let (millis, _) = encoding::timestamp_columns(until);
        clauses.push("started_at_ms < ?".to_owned());
        values.push(Value::Integer(millis));
    }

    if let Some(content_type) = &filter.content_type {
        clauses.push(
            "(search_text(request_content_type, ?, ?) \
               OR search_text(response_content_type, ?, ?))"
                .to_owned(),
        );
        // Content-type filters are always a case-insensitive substring: they
        // are a menu, not the search box.
        let flags = Value::Integer(0);
        values.push(Value::Text(content_type.clone()));
        values.push(flags.clone());
        values.push(Value::Text(content_type.clone()));
        values.push(flags);
    }

    if let Some(version) = filter.http_version {
        let stored = encoding::version_column(version);
        clauses.push("(request_version = ? OR response_version = ?)".to_owned());
        values.push(Value::Text(stored.to_owned()));
        values.push(Value::Text(stored.to_owned()));
    }

    if filter.websocket {
        clauses.push("response_status = 101".to_owned());
    }

    if filter.sse {
        clauses.push(
            "(lower(ifnull(response_content_type, '')) LIKE 'text/event-stream%' \
               OR EXISTS (\
                    SELECT 1 FROM stream_messages s \
                    WHERE s.exchange_id = exchanges.id AND s.kind = 'sse'\
               ))"
            .to_owned(),
        );
    }

    if filter.failed {
        clauses.push("error_kind IS NOT NULL".to_owned());
    }

    if let Some((search, search_values)) = search::clause(filter) {
        clauses.push(search);
        values.extend(search_values);
    }

    if let Some(cursor) = after {
        // Strictly after the cursor in the order being read, which is the
        // same comparison that ordering is made of.
        clauses.push(order.after_cursor().to_owned());
        values.push(Value::Integer(cursor.started_at_ms()));
        values.push(Value::Integer(cursor.started_at_ms()));
        values.push(Value::Text(cursor.id().as_text()));
    }

    if clauses.is_empty() {
        return (String::new(), values);
    }

    (format!(" WHERE {}", clauses.join(" AND ")), values)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use tempfile::TempDir;

    use crate::traffic::{CapturedRequest, ConnectionInfo, Destination, ExchangeId, HttpVersion};

    use super::Storage;

    /// Long enough that a loaded machine still finishes an insert, short
    /// enough that an insert waiting on the reader fails the test.
    const INSERT_BOUND: Duration = Duration::from_secs(2);

    #[test]
    fn a_held_reader_does_not_block_an_insert() {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = Storage::open_in(directory.path()).expect("a store");
        let _reader = storage.lease_reader();

        let (sender, receiver) = mpsc::channel();
        let writing = storage.clone();
        thread::spawn(move || {
            let exchange = exchange();
            let _ = sender.send(writing.insert(&exchange));
        });

        let stored = receiver
            .recv_timeout(INSERT_BOUND)
            .expect("an insert waited on a reader");
        stored.expect("the insert is stored");
    }

    fn exchange() -> crate::traffic::TrafficExchange {
        let client: SocketAddr = "127.0.0.1:9".parse().expect("an address");

        crate::traffic::TrafficExchange::new(
            ExchangeId::next(),
            ConnectionInfo::http(client, Some(Destination::new("example.com", 80))),
            CapturedRequest::new("GET", "http://example.com/", HttpVersion::Http11),
        )
    }
}
