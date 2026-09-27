//! Removing stored traffic, and the body files that went with it.

mod support;

use support::{http_exchange, with_bodies, TestStore};

use proxy_storage::{BodyCapture, TrafficQuery};

/// An exchange whose captured bodies are both large enough to need files.
fn exchange_with_body_files(host: &str, path: &str) -> proxy_storage::TrafficExchange {
    with_bodies(
        http_exchange(host, path, 200),
        BodyCapture::complete(support::binary(2_048)),
        BodyCapture::complete(support::binary(4_096)),
    )
}

#[test]
fn deleting_an_exchange_says_it_deleted_one() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 200);
    store.storage.insert(&exchange).expect("stores");

    assert!(store.storage.delete(exchange.id).expect("deletes"));
    assert_eq!(store.storage.count().expect("counts"), 0);
}

#[test]
fn a_deleted_exchange_cannot_be_read_back() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 200);
    store.storage.insert(&exchange).expect("stores");

    store.storage.delete(exchange.id).expect("deletes");

    assert_eq!(store.storage.get(exchange.id).expect("reads"), None);
    assert!(store
        .storage
        .list(&TrafficQuery::newest())
        .expect("lists")
        .is_empty());
}

#[test]
fn deleting_one_exchange_leaves_the_others_alone() {
    let store = TestStore::open();
    let kept = http_exchange("example.com", "/kept", 200);
    let removed = http_exchange("example.com", "/removed", 200);
    store.storage.insert(&kept).expect("stores");
    store.storage.insert(&removed).expect("stores");

    store.storage.delete(removed.id).expect("deletes");

    assert_eq!(store.storage.count().expect("counts"), 1);
    assert_eq!(
        store.storage.get(kept.id).expect("reads").map(|e| e.id),
        Some(kept.id)
    );
}

#[test]
fn deleting_an_exchange_removes_its_body_files() {
    let store = TestStore::with_inline_limit(16);
    let exchange = exchange_with_body_files("example.com", "/large");
    let id = exchange.id;
    store.storage.insert(&exchange).expect("stores");
    assert_eq!(
        store.body_files(),
        [format!("{id}.request"), format!("{id}.response")]
    );

    store.storage.delete(id).expect("deletes");

    assert!(
        store.body_files().is_empty(),
        "body files outlived the exchange: {:?}",
        store.body_files()
    );
}

#[test]
fn deleting_an_exchange_leaves_another_exchanges_body_files_alone() {
    let store = TestStore::with_inline_limit(16);
    let kept = exchange_with_body_files("example.com", "/kept");
    let removed = exchange_with_body_files("example.com", "/removed");
    store.storage.insert(&kept).expect("stores");
    store.storage.insert(&removed).expect("stores");

    store.storage.delete(removed.id).expect("deletes");

    assert_eq!(
        store.body_files(),
        [
            format!("{}.request", kept.id),
            format!("{}.response", kept.id)
        ]
    );
    let stored = store
        .storage
        .get(kept.id)
        .expect("reads")
        .expect("still stored");
    assert_eq!(stored.request.body.data(), support::binary(2_048));
}

#[test]
fn deleting_an_exchange_that_is_not_stored_says_so_rather_than_failing() {
    let store = TestStore::open();

    assert!(!store
        .storage
        .delete(proxy_storage::ExchangeId::next())
        .expect("deletes nothing"));
}

#[test]
fn deleting_the_same_exchange_twice_is_not_a_failure() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 200);
    store.storage.insert(&exchange).expect("stores");

    assert!(store.storage.delete(exchange.id).expect("deletes"));
    assert!(!store.storage.delete(exchange.id).expect("deletes nothing"));
}

#[test]
fn clearing_removes_every_exchange() {
    let store = TestStore::open();
    for index in 0..5 {
        store
            .storage
            .insert(&http_exchange("example.com", &format!("/{index}"), 200))
            .expect("stores");
    }

    assert_eq!(store.storage.clear().expect("clears"), 5);

    assert_eq!(store.storage.count().expect("counts"), 0);
    assert!(store
        .storage
        .list(&TrafficQuery::newest())
        .expect("lists")
        .is_empty());
}

#[test]
fn clearing_removes_every_body_file() {
    let store = TestStore::with_inline_limit(16);
    for index in 0..3 {
        store
            .storage
            .insert(&exchange_with_body_files(
                "example.com",
                &format!("/{index}"),
            ))
            .expect("stores");
    }
    assert_eq!(store.body_files().len(), 6);

    store.storage.clear().expect("clears");

    assert!(
        store.body_files().is_empty(),
        "body files outlived the history: {:?}",
        store.body_files()
    );
}

#[test]
fn clearing_an_empty_database_is_not_a_failure() {
    let store = TestStore::open();

    assert_eq!(store.storage.clear().expect("clears"), 0);
}

#[test]
fn a_cleared_database_still_works() {
    let store = TestStore::with_inline_limit(16);
    store
        .storage
        .insert(&exchange_with_body_files("example.com", "/before"))
        .expect("stores");
    store.storage.clear().expect("clears");

    let after = exchange_with_body_files("example.com", "/after");
    store.storage.insert(&after).expect("stores");

    assert_eq!(store.storage.count().expect("counts"), 1);
    let stored = store
        .storage
        .get(after.id)
        .expect("reads")
        .expect("stored after the clear");
    assert_eq!(stored, after);
}

#[test]
fn what_one_handle_deletes_another_handle_sees_gone() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 200);
    store.storage.insert(&exchange).expect("stores");
    let other = store.reopen();

    store.storage.delete(exchange.id).expect("deletes");

    assert_eq!(other.get(exchange.id).expect("reads"), None);
    assert_eq!(other.count().expect("counts"), 0);
}
