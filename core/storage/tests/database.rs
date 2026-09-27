//! Opening, creating, and migrating the traffic database.

mod support;

use support::TestStore;

use proxy_storage::{Storage, StorageConfig, StorageError, SCHEMA_VERSION};
use tempfile::TempDir;

#[test]
fn a_new_database_is_created_where_it_is_asked_for() {
    let directory = TempDir::new().expect("a temporary directory");
    let config = StorageConfig::in_directory(directory.path());

    let storage = Storage::open(config.clone()).expect("a new store opens");

    assert!(config.database.is_file(), "no database file was created");
    assert!(config.bodies.is_dir(), "no body directory was created");
    assert_eq!(storage.count().expect("counts"), 0);
}

#[test]
fn a_new_database_is_created_at_the_current_schema_version() {
    let store = TestStore::open();

    assert_eq!(
        store.storage.schema_version().expect("a version"),
        SCHEMA_VERSION
    );
}

#[test]
fn a_new_database_has_the_schema_the_queries_need() {
    let store = TestStore::open();

    // Every operation the schema has to support, on an empty database: if a
    // table or a column were missing, one of these would fail rather than
    // return nothing.
    assert_eq!(store.storage.count().expect("counts"), 0);
    assert!(store
        .storage
        .list(&proxy_storage::TrafficQuery::newest())
        .expect("lists")
        .is_empty());
    assert_eq!(
        store
            .storage
            .get(proxy_storage::ExchangeId::next())
            .expect("reads"),
        None
    );
}

#[test]
fn a_missing_directory_is_created_rather_than_refused() {
    let directory = TempDir::new().expect("a temporary directory");
    let nested = directory.path().join("one").join("two").join("three");

    let storage = Storage::open_in(&nested).expect("a store in a directory that is not there yet");

    assert_eq!(storage.count().expect("counts"), 0);
    assert!(nested.join("traffic.db").is_file());
}

#[test]
fn an_existing_database_is_reopened_with_its_traffic_still_in_it() {
    let store = TestStore::open();
    let exchange = support::http_exchange("example.com", "/kept", 200);
    let id = exchange.id;
    store.storage.insert(&exchange).expect("stores");

    // A second handle, opened as a fresh process would open it.
    let reopened = store.reopen();

    assert_eq!(reopened.count().expect("counts"), 1);
    assert_eq!(
        reopened.get(id).expect("reads").map(|stored| stored.id),
        Some(id)
    );
}

#[test]
fn an_existing_schema_is_recognised_rather_than_recreated() {
    let store = TestStore::open();
    let exchange = support::http_exchange("example.com", "/kept", 200);
    store.storage.insert(&exchange).expect("stores");

    // Opening again runs the migration step, which must find nothing to do:
    // a re-run of the initial migration would fail on the tables it creates,
    // and a schema that was recreated would have lost the exchange.
    let reopened = store.reopen();

    assert_eq!(
        reopened.schema_version().expect("a version"),
        SCHEMA_VERSION
    );
    assert_eq!(reopened.count().expect("counts"), 1);
}

#[test]
fn the_same_database_can_be_opened_many_times_over() {
    let store = TestStore::open();

    let handles: Vec<Storage> = (0..8).map(|_| store.reopen()).collect();

    for handle in &handles {
        assert_eq!(handle.schema_version().expect("a version"), SCHEMA_VERSION);
        assert_eq!(handle.count().expect("counts"), 0);
    }

    // What one handle stores, another sees.
    handles[0]
        .insert(&support::http_exchange("example.com", "/x", 200))
        .expect("stores");

    assert_eq!(handles[7].count().expect("counts"), 1);
}

#[test]
fn a_database_written_by_a_later_version_is_left_alone() {
    let store = TestStore::open();
    let config = store.config();
    // Stand in for a database some future ProxyLab wrote. Reading it as
    // though it were this version is exactly what must not happen.
    let connection = rusqlite_open(&config);
    connection
        .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
        .expect("the version is writable");
    drop(connection);

    let error = Storage::open(config).expect_err("a database from the future is refused");

    assert!(
        matches!(error, StorageError::UnsupportedSchema { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION),
        "{error:?}"
    );
}

#[test]
fn a_database_path_that_cannot_be_created_fails_safely() {
    let directory = TempDir::new().expect("a temporary directory");
    // A file where the traffic directory should be: the database cannot be
    // put underneath it, and finding that out must be an error rather than a
    // panic.
    let blocked = directory.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("a file in the way");

    let error = Storage::open_in(blocked.join("inside")).expect_err("there is nowhere to open");

    assert!(matches!(error, StorageError::Directory { .. }), "{error:?}");
    assert!(!error.to_string().is_empty());
}

#[test]
fn a_database_that_is_not_a_database_fails_safely() {
    let directory = TempDir::new().expect("a temporary directory");
    let config = StorageConfig::in_directory(directory.path());
    std::fs::create_dir_all(directory.path()).expect("a directory");
    std::fs::write(&config.database, b"this is not a SQLite file").expect("a file in the way");

    let error = Storage::open(config).expect_err("that file is not a database");

    assert!(matches!(error, StorageError::Database(_)), "{error:?}");
    assert!(!error.to_string().is_empty());
}

#[test]
fn a_body_directory_that_cannot_be_created_fails_safely() {
    let directory = TempDir::new().expect("a temporary directory");
    let blocked = directory.path().join("bodies");
    std::fs::write(&blocked, b"not a directory").expect("a file in the way");
    let config = StorageConfig::new(directory.path().join("traffic.db"), blocked);

    let error = Storage::open(config).expect_err("there is nowhere to keep bodies");

    assert!(matches!(error, StorageError::Directory { .. }), "{error:?}");
}

/// A raw connection to a store's database, for the tests that have to stand
/// in for a different version of ProxyLab.
fn rusqlite_open(config: &StorageConfig) -> rusqlite::Connection {
    rusqlite::Connection::open(&config.database).expect("the database opens")
}
