//! The database schema, and getting an existing database to it.
//!
//! ```text
//! exchanges     one row per exchange: identity, connection, timing,
//!               outcome, and the request and response fields a traffic
//!               list is built from
//!
//! messages      two rows per exchange at most: the headers and the
//!               captured body of the request, and of the response if
//!               there was one
//!
//! rules         one row per rule: what it matches, what it does, and
//!               where it sits in the order they are consulted in
//!
//! saved_requests  one row per saved request: enough to build the request
//!                 again, and nothing about what happened when it was sent
//! ```
//!
//! The split is by how the columns are used, not by how the model is shaped.
//! Everything a list query filters, sorts, or shows sits in `exchanges`, so
//! that query reads one table through one index. The parts only an inspector
//! opens — headers, body bytes — sit in `messages`. A search may open those
//! rows one at a time through registered SQL functions; a list page still
//! does not drag them through every scan.
//!
//! No column holds a serialized [`TrafficExchange`](crate::TrafficExchange).
//! Method, host, URL, status, timestamp, duration, and both Content-Types
//! are columns of their own, and are what the indexes and the type filter
//! are on. Headers are stored as their own JSON: a header list repeats
//! names, keeps its order, and sometimes carries bytes that are not text.
//! Search may substring that JSON; nothing queries a header by name as a
//! column.
//!
//! # Versioning
//!
//! The schema version lives in SQLite's own `user_version`, and
//! [`MIGRATIONS`] is the list of steps that get a database from nothing to
//! it. A migration is only ever appended: step `n` takes a database at
//! version `n` to version `n + 1`, and once released it is never edited,
//! because some database out there has already run it. Opening a database
//! runs the steps it has not run yet, in one transaction each, so a database
//! is either at the version before a step or the version after it.

use rusqlite::Connection;

use super::error::StorageError;

/// The schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 5;

/// The steps from an empty database to [`SCHEMA_VERSION`].
///
/// Index `n` is the step that takes a database at version `n` to version
/// `n + 1`.
const MIGRATIONS: &[&str] = &[
    V1_INITIAL,
    V2_RULES,
    V3_SAVED_REQUESTS,
    V4_STREAM_MESSAGES,
    V5_CONTENT_TYPE,
];

/// The first schema: exchanges, their messages, and the indexes the traffic
/// list needs.
///
/// `STRICT` is on both tables so a column that says `INTEGER` cannot come
/// back as text, and the `CHECK`s state the invariants the model has: a
/// scheme is one of two things, a destination is a host and a port or
/// neither, and a captured body is inline or in a file but never both.
const V1_INITIAL: &str = "\
CREATE TABLE exchanges (
    id                       TEXT    PRIMARY KEY NOT NULL,
    started_at_ms            INTEGER NOT NULL,
    started_at_subms_nanos   INTEGER NOT NULL,
    response_at_ms           INTEGER,
    duration_micros          INTEGER,
    upstream_connect_micros  INTEGER,
    upstream_response_micros INTEGER,
    client_addr              TEXT    NOT NULL,
    scheme                   TEXT    NOT NULL CHECK (scheme IN ('http', 'https')),
    host                     TEXT    COLLATE NOCASE,
    port                     INTEGER,
    completed                INTEGER NOT NULL CHECK (completed IN (0, 1)),
    error_kind               TEXT,
    error_summary            TEXT,
    request_method           TEXT    NOT NULL,
    request_url              TEXT    NOT NULL,
    request_version          TEXT    NOT NULL,
    response_status          INTEGER,
    response_reason          TEXT,
    response_version         TEXT,
    CHECK ((host IS NULL) = (port IS NULL))
) STRICT;

CREATE TABLE messages (
    exchange_id       TEXT    NOT NULL REFERENCES exchanges (id) ON DELETE CASCADE,
    role              TEXT    NOT NULL CHECK (role IN ('request', 'response')),
    headers           TEXT    NOT NULL,
    body_size         INTEGER NOT NULL,
    body_captured_len INTEGER NOT NULL,
    body_truncated    INTEGER NOT NULL CHECK (body_truncated IN (0, 1)),
    body_inline       BLOB,
    body_file         TEXT,
    PRIMARY KEY (exchange_id, role),
    CHECK (body_inline IS NULL OR body_file IS NULL)
) STRICT;

CREATE INDEX exchanges_newest ON exchanges (started_at_ms DESC, id DESC);
CREATE INDEX exchanges_host   ON exchanges (host, started_at_ms DESC, id DESC);
CREATE INDEX exchanges_method ON exchanges (request_method, started_at_ms DESC, id DESC);
CREATE INDEX exchanges_status ON exchanges (response_status, started_at_ms DESC, id DESC);
";

/// Rules: what the proxy does with a request before it carries it.
///
/// A table of its own, added rather than folded into anything: a rule has
/// nothing to do with an exchange, outlives every exchange, and is written
/// by a person rather than by the proxy. Nothing in `exchanges` or
/// `messages` is touched by this step, so a database full of captured
/// traffic gains a rules table and keeps everything it had.
///
/// `matcher` and `action` are JSON because they are sum types with optional
/// parts — a matcher has three optional criteria, an action is one of four
/// shapes with different fields — and nothing queries inside them. The
/// columns that *are* queried are columns: `enabled` and `priority` decide
/// which rule wins, and `id` breaks the tie. Both are validated on the way
/// back in: see `rules::Rule::validate`.
///
/// The order rules are consulted in is `priority ASC, id ASC`, and the index
/// is on exactly that, so loading the rule set is one ordered scan.
const V2_RULES: &str = "\
CREATE TABLE rules (
    id            TEXT    PRIMARY KEY NOT NULL,
    enabled       INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    name          TEXT    NOT NULL,
    priority      INTEGER NOT NULL,
    matcher       TEXT    NOT NULL,
    action        TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX rules_order ON rules (priority ASC, id ASC);
";

/// Saved requests: the requests a person kept so they can send them again.
///
/// A table of its own, added rather than folded into anything, for the
/// reasons `rules` was: a saved request has nothing to do with an exchange,
/// outlives every exchange, and is written by a person rather than by the
/// proxy. Nothing in `exchanges`, `messages` or `rules` is touched by this
/// step.
///
/// The body is placed the way a captured one is — nothing, a `BLOB`, or a
/// file in the body directory — so a saved request carrying a megabyte of
/// JSON does not put a megabyte in a row that a list query scans past. The
/// same `CHECK` says it is in one place or the other and never both, and
/// `body_size` is how long the body *was*: a request saved out of a capture
/// the proxy cut short knows it is sending less than the original did, which
/// is what `body_truncated` is for.
///
/// `headers` is JSON, for the reason the captured ones are: a header list
/// repeats names, keeps its order, sometimes carries bytes that are not
/// text, and nothing queries inside one.
///
/// The order a list is shown in is the order they were saved, which is
/// `created_at_ms ASC, id ASC` — an id is a time-ordered UUID, so it breaks
/// a tie between two requests saved in the same millisecond and breaks it
/// the same way for ever. The index is on exactly that.
const V3_SAVED_REQUESTS: &str = "\
CREATE TABLE saved_requests (
    id             TEXT    PRIMARY KEY NOT NULL,
    name           TEXT    NOT NULL,
    method         TEXT    NOT NULL,
    url            TEXT    NOT NULL,
    headers        TEXT    NOT NULL,
    body_size      INTEGER NOT NULL,
    body_truncated INTEGER NOT NULL CHECK (body_truncated IN (0, 1)),
    body_inline    BLOB,
    body_file      TEXT,
    created_at_ms  INTEGER NOT NULL,
    updated_at_ms  INTEGER NOT NULL,
    CHECK (body_inline IS NULL OR body_file IS NULL)
) STRICT;

CREATE INDEX saved_requests_order ON saved_requests (created_at_ms ASC, id ASC);
";

/// Stream messages: WebSocket messages and SSE events that belong to one
/// exchange.
///
/// A table of its own rather than more rows in `messages`. That table is two
/// rows per exchange — the HTTP request and the HTTP response — with a
/// primary key of `(exchange_id, role)`. A WebSocket can carry thousands of
/// messages on one handshake, so they cannot share it.
///
/// The payload is placed the way a captured body is: nothing, a `BLOB`, or a
/// file in the body directory under `<message id>.stream`. Clearing traffic
/// removes those files with the HTTP body files, and deleting an exchange
/// takes its stream messages with it by the foreign key.
const V4_STREAM_MESSAGES: &str = "\
CREATE TABLE stream_messages (
    id                      TEXT    PRIMARY KEY NOT NULL,
    exchange_id             TEXT    NOT NULL REFERENCES exchanges (id) ON DELETE CASCADE,
    sequence                INTEGER NOT NULL,
    started_at_ms           INTEGER NOT NULL,
    started_at_subms_nanos  INTEGER NOT NULL,
    direction               TEXT    NOT NULL CHECK (direction IN ('client_to_server', 'server_to_client')),
    kind                    TEXT    NOT NULL CHECK (kind IN ('websocket_text', 'websocket_binary', 'sse')),
    event_name              TEXT,
    event_id                TEXT,
    retry_ms                INTEGER,
    payload_size            INTEGER NOT NULL,
    payload_captured_len    INTEGER NOT NULL,
    payload_truncated       INTEGER NOT NULL CHECK (payload_truncated IN (0, 1)),
    payload_inline          BLOB,
    payload_file            TEXT,
    UNIQUE (exchange_id, sequence),
    CHECK (payload_inline IS NULL OR payload_file IS NULL)
) STRICT;

CREATE INDEX stream_messages_exchange ON stream_messages (exchange_id, sequence ASC);
";

/// Content types copied off the request and response so a filter can narrow
/// by them without opening the header JSON of every row.
///
/// They are the `Content-Type` header as it was spelled, parameters included.
/// Existing rows stay `NULL` and are outside a content-type filter until they
/// are stored again; WebSocket and SSE filters still have `response_status`
/// and `stream_messages` to go on.
const V5_CONTENT_TYPE: &str = "\
ALTER TABLE exchanges ADD COLUMN request_content_type TEXT;
ALTER TABLE exchanges ADD COLUMN response_content_type TEXT;
";

/// The version `connection`'s database is at.
///
/// A database that has never been migrated is at zero, which is what SQLite
/// gives a file it has just created.
pub fn version(connection: &Connection) -> Result<u32, StorageError> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as u32)
}

/// Brings `connection`'s database up to [`SCHEMA_VERSION`].
///
/// A new database gets every step; an existing one gets the steps it has not
/// run; one that is already current is left alone. Each step and the version
/// it sets are one transaction, so an interrupted migration leaves a database
/// at a version, never between two.
///
/// A database at a *later* version is an error rather than a database to
/// work on: this build does not know what a later version added, and writing
/// to it as though it were this one would corrupt it.
pub fn migrate(connection: &mut Connection) -> Result<u32, StorageError> {
    let found = version(connection)?;
    let supported = SCHEMA_VERSION;

    if found > supported {
        return Err(StorageError::UnsupportedSchema { found, supported });
    }

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let target = index as u32 + 1;
        let transaction = connection.transaction()?;

        transaction.execute_batch(migration)?;
        // `user_version` takes no parameter, so the value is written into the
        // statement. It is a constant of this crate's own, never anything
        // that came from outside it.
        transaction.execute_batch(&format!("PRAGMA user_version = {target}"))?;
        transaction.commit()?;
    }

    Ok(supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated() -> Connection {
        let mut connection = Connection::open_in_memory().expect("an in-memory database");
        migrate(&mut connection).expect("migrates");

        connection
    }

    fn table_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("the catalogue is readable");
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("the catalogue is readable")
            .collect::<Result<Vec<String>, _>>()
            .expect("the catalogue is readable");

        names
    }

    fn index_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .expect("the catalogue is readable");
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("the catalogue is readable")
            .collect::<Result<Vec<String>, _>>()
            .expect("the catalogue is readable");

        names
    }

    #[test]
    fn there_is_a_migration_for_every_version() {
        assert_eq!(MIGRATIONS.len(), SCHEMA_VERSION as usize);
    }

    #[test]
    fn a_new_database_starts_at_version_zero() {
        let connection = Connection::open_in_memory().expect("an in-memory database");

        assert_eq!(version(&connection).expect("a version"), 0);
    }

    #[test]
    fn migrating_a_new_database_creates_the_schema() {
        let connection = migrated();

        assert_eq!(version(&connection).expect("a version"), SCHEMA_VERSION);
        assert_eq!(
            table_names(&connection),
            [
                "exchanges",
                "messages",
                "rules",
                "saved_requests",
                "stream_messages",
            ]
        );
        let content_type: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('exchanges') \
                 WHERE name IN ('request_content_type', 'response_content_type')",
                [],
                |row| row.get(0),
            )
            .expect("the catalogue is readable");
        assert_eq!(content_type, 2);
    }

    #[test]
    fn the_indexes_the_traffic_list_needs_are_created() {
        assert_eq!(
            index_names(&migrated()),
            [
                "exchanges_host",
                "exchanges_method",
                "exchanges_newest",
                "exchanges_status",
                "rules_order",
                "saved_requests_order",
                "stream_messages_exchange",
            ]
        );
    }

    #[test]
    fn migrating_an_already_current_database_changes_nothing() {
        let mut connection = migrated();

        assert_eq!(migrate(&mut connection).expect("migrates"), SCHEMA_VERSION);
        assert_eq!(version(&connection).expect("a version"), SCHEMA_VERSION);
        assert_eq!(
            table_names(&connection),
            [
                "exchanges",
                "messages",
                "rules",
                "saved_requests",
                "stream_messages",
            ]
        );
    }

    #[test]
    fn a_database_from_an_earlier_version_is_migrated_forwards_with_everything_in_it() {
        // The step that matters for anyone who was already using ProxyLab: a
        // database full of captured traffic and rules is migrated forwards
        // rather than recreated, and both are still in it afterwards.
        let mut connection = Connection::open_in_memory().expect("an in-memory database");
        connection
            .execute_batch(V1_INITIAL)
            .expect("the first schema applies");
        connection
            .execute_batch(V2_RULES)
            .expect("the second schema applies");
        connection
            .execute_batch("PRAGMA user_version = 2")
            .expect("the version is writable");
        connection
            .execute(
                "INSERT INTO exchanges (
                    id, started_at_ms, started_at_subms_nanos, client_addr, scheme, completed,
                    request_method, request_url, request_version
                 ) VALUES ('e1', 1, 0, '127.0.0.1:1', 'http', 1, 'GET', 'http://x/', 'HTTP/1.1')",
                [],
            )
            .expect("the exchange is stored");
        connection
            .execute(
                "INSERT INTO rules (
                    id, enabled, name, priority, matcher, action, created_at_ms, updated_at_ms
                 ) VALUES ('r1', 1, 'a rule', 0, '{}', '{}', 1, 1)",
                [],
            )
            .expect("the rule is stored");

        assert_eq!(migrate(&mut connection).expect("migrates"), SCHEMA_VERSION);

        assert_eq!(
            table_names(&connection),
            [
                "exchanges",
                "messages",
                "rules",
                "saved_requests",
                "stream_messages",
            ]
        );
        let exchanges: i64 = connection
            .query_row("SELECT COUNT(*) FROM exchanges", [], |row| row.get(0))
            .expect("the traffic survived");
        assert_eq!(exchanges, 1);
        let rules: i64 = connection
            .query_row("SELECT COUNT(*) FROM rules", [], |row| row.get(0))
            .expect("the rules survived");
        assert_eq!(rules, 1);
        let saved: i64 = connection
            .query_row("SELECT COUNT(*) FROM saved_requests", [], |row| row.get(0))
            .expect("the saved requests table is there");
        assert_eq!(saved, 0);
    }

    #[test]
    fn a_database_at_the_first_version_gains_every_table_after_it() {
        let mut connection = Connection::open_in_memory().expect("an in-memory database");
        connection
            .execute_batch(V1_INITIAL)
            .expect("the first schema applies");
        connection
            .execute_batch("PRAGMA user_version = 1")
            .expect("the version is writable");

        assert_eq!(migrate(&mut connection).expect("migrates"), SCHEMA_VERSION);

        assert_eq!(
            table_names(&connection),
            [
                "exchanges",
                "messages",
                "rules",
                "saved_requests",
                "stream_messages",
            ]
        );
    }

    #[test]
    fn a_saved_request_body_must_be_in_one_place_or_the_other() {
        let connection = migrated();

        let refused = connection.execute(
            "INSERT INTO saved_requests (
                 id, name, method, url, headers, body_size, body_truncated,
                 body_inline, body_file, created_at_ms, updated_at_ms
             ) VALUES ('s1', 'n', 'GET', 'http://x/', '[]', 1, 0, X'00', 's1.saved', 1, 1)",
            [],
        );

        assert!(refused.is_err(), "a body in two places was accepted");
    }

    #[test]
    fn a_database_from_a_later_version_is_left_alone() {
        let mut connection = migrated();
        connection
            .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
            .expect("the version is writable");

        let error = migrate(&mut connection).expect_err("a later version is refused");

        assert!(
            matches!(
                error,
                StorageError::UnsupportedSchema {
                    found,
                    supported,
                } if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
            ),
            "{error:?}"
        );
    }

    #[test]
    fn the_tables_reject_a_value_of_the_wrong_type() {
        let connection = migrated();

        // `STRICT` is what makes this a failure rather than a row that reads
        // back as something else later.
        let refused = connection.execute(
            "INSERT INTO exchanges (
                 id, started_at_ms, started_at_subms_nanos, client_addr, scheme, completed,
                 request_method, request_url, request_version
             ) VALUES ('id', 'not a number', 0, '127.0.0.1:1', 'http', 0, 'GET', '/', 'HTTP/1.1')",
            [],
        );

        assert!(refused.is_err(), "a text timestamp was accepted");
    }

    #[test]
    fn the_tables_reject_a_scheme_that_is_neither_http_nor_https() {
        let connection = migrated();

        let refused = connection.execute(
            "INSERT INTO exchanges (
                 id, started_at_ms, started_at_subms_nanos, client_addr, scheme, completed,
                 request_method, request_url, request_version
             ) VALUES ('id', 0, 0, '127.0.0.1:1', 'ftp', 0, 'GET', '/', 'HTTP/1.1')",
            [],
        );

        assert!(refused.is_err(), "an unknown scheme was accepted");
    }

    #[test]
    fn the_tables_reject_a_host_without_a_port() {
        let connection = migrated();

        let refused = connection.execute(
            "INSERT INTO exchanges (
                 id, started_at_ms, started_at_subms_nanos, client_addr, scheme, completed,
                 request_method, request_url, request_version, host
             ) VALUES ('id', 0, 0, '127.0.0.1:1', 'http', 0, 'GET', '/', 'HTTP/1.1', 'example.com')",
            [],
        );

        assert!(refused.is_err(), "a destination with no port was accepted");
    }
}
