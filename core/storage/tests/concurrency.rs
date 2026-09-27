//! Storing and reading traffic from many threads at once.
//!
//! Nothing here sleeps or polls: every test starts its threads, joins them
//! all, and then asserts. What is being checked is that concurrent use is
//! safe and loses nothing, not how fast it is.

mod support;

use std::sync::Arc;
use std::thread;

use support::{http_exchange, with_bodies, TestStore};

use proxy_storage::{BodyCapture, ExchangeId, Storage, TrafficQuery};

/// How many threads, and how many exchanges each of them stores.
const THREADS: usize = 8;
const EACH: usize = 16;

/// An exchange with bodies on both sides, large enough to need files.
fn exchange(host: &str, path: &str) -> proxy_storage::TrafficExchange {
    with_bodies(
        http_exchange(host, path, 200),
        BodyCapture::complete(support::binary(4_096)),
        BodyCapture::complete(support::binary(8_192)),
    )
}

/// Stores `EACH` exchanges from each of `THREADS` threads, and returns every
/// id that was stored.
fn store_from_every_thread(storage: &Storage) -> Vec<ExchangeId> {
    let storage = Arc::new(storage.clone());
    let mut threads = Vec::new();

    for thread_number in 0..THREADS {
        let storage = Arc::clone(&storage);
        threads.push(thread::spawn(move || {
            let mut ids = Vec::with_capacity(EACH);
            for index in 0..EACH {
                let exchange = exchange(
                    "example.com",
                    &format!("/thread-{thread_number}/item-{index}"),
                );
                storage.insert(&exchange).expect("stores");
                ids.push(exchange.id);
            }

            ids
        }));
    }

    threads
        .into_iter()
        .flat_map(|thread| thread.join().expect("the thread does not panic"))
        .collect()
}

#[test]
fn exchanges_stored_from_many_threads_are_all_stored() {
    let store = TestStore::with_inline_limit(64);

    let ids = store_from_every_thread(&store.storage);

    assert_eq!(ids.len(), THREADS * EACH);
    assert_eq!(
        store.storage.count().expect("counts"),
        (THREADS * EACH) as u64
    );
    for id in ids {
        let stored = store
            .storage
            .get(id)
            .expect("reads")
            .expect("every exchange is stored");
        assert_eq!(stored.request.body.data(), support::binary(4_096));
        assert_eq!(
            stored.response.as_ref().expect("a response").body.data(),
            support::binary(8_192)
        );
    }
}

#[test]
fn every_concurrently_stored_exchange_keeps_its_own_body_files() {
    let store = TestStore::with_inline_limit(64);

    let ids = store_from_every_thread(&store.storage);

    let mut expected: Vec<String> = ids
        .iter()
        .flat_map(|id| [format!("{id}.request"), format!("{id}.response")])
        .collect();
    expected.sort();

    assert_eq!(store.body_files(), expected);
}

#[test]
fn reading_from_many_threads_at_once_gives_every_thread_the_same_answer() {
    let store = TestStore::open();
    let mut ids = Vec::new();
    for index in 0..32 {
        let exchange = http_exchange("example.com", &format!("/{index}"), 200);
        store.storage.insert(&exchange).expect("stores");
        ids.push(exchange.id);
    }
    let storage = Arc::new(store.storage.clone());
    let ids = Arc::new(ids);

    let readers: Vec<thread::JoinHandle<(u64, usize)>> = (0..THREADS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            let ids = Arc::clone(&ids);
            thread::spawn(move || {
                for id in ids.iter() {
                    assert!(storage.get(*id).expect("reads").is_some());
                }

                let listed = storage
                    .list(&TrafficQuery::newest().with_limit(100))
                    .expect("lists");

                (storage.count().expect("counts"), listed.len())
            })
        })
        .collect();

    for reader in readers {
        assert_eq!(reader.join().expect("the thread does not panic"), (32, 32));
    }
}

#[test]
fn reading_while_storing_never_sees_half_an_exchange() {
    let store = TestStore::with_inline_limit(64);
    let storage = Arc::new(store.storage.clone());

    let writers: Vec<thread::JoinHandle<Vec<ExchangeId>>> = (0..THREADS)
        .map(|thread_number| {
            let storage = Arc::clone(&storage);
            thread::spawn(move || {
                let mut ids = Vec::with_capacity(EACH);
                for index in 0..EACH {
                    let exchange = exchange("example.com", &format!("/{thread_number}/{index}"));
                    storage.insert(&exchange).expect("stores");
                    ids.push(exchange.id);
                }

                ids
            })
        })
        .collect();

    // Readers run against a database that is being written the whole time.
    // Every exchange a reader sees must be whole: if an insert were visible
    // before its bodies were, reading one back would fail.
    let readers: Vec<thread::JoinHandle<()>> = (0..THREADS)
        .map(|_| {
            let storage = Arc::clone(&storage);
            thread::spawn(move || {
                for _ in 0..EACH {
                    let page = storage
                        .list(&TrafficQuery::newest().with_limit(32))
                        .expect("lists");

                    for summary in page.exchanges {
                        let stored = storage
                            .get(summary.id)
                            .expect("reads")
                            .expect("a listed exchange is a stored exchange");

                        assert_eq!(stored.request.body.data(), support::binary(4_096));
                        assert_eq!(stored.id, summary.id);
                    }
                }
            })
        })
        .collect();

    let stored: Vec<ExchangeId> = writers
        .into_iter()
        .flat_map(|writer| writer.join().expect("the thread does not panic"))
        .collect();
    for reader in readers {
        reader.join().expect("the thread does not panic");
    }

    assert_eq!(stored.len(), THREADS * EACH);
    assert_eq!(
        store.storage.count().expect("counts"),
        (THREADS * EACH) as u64
    );
}

#[test]
fn deleting_while_reading_leaves_the_database_consistent() {
    let store = TestStore::with_inline_limit(64);
    let mut ids = Vec::new();
    for index in 0..64 {
        let exchange = exchange("example.com", &format!("/{index}"));
        store.storage.insert(&exchange).expect("stores");
        ids.push(exchange.id);
    }
    let storage = Arc::new(store.storage.clone());
    let (removed, kept) = ids.split_at(32);

    let deleting = {
        let storage = Arc::clone(&storage);
        let removed: Vec<ExchangeId> = removed.to_vec();
        thread::spawn(move || {
            for id in removed {
                assert!(storage.delete(id).expect("deletes"));
            }
        })
    };
    let reading = {
        let storage = Arc::clone(&storage);
        let kept: Vec<ExchangeId> = kept.to_vec();
        thread::spawn(move || {
            for id in kept {
                let stored = storage
                    .get(id)
                    .expect("reads")
                    .expect("an exchange nobody deleted is still stored");
                assert_eq!(stored.id, id);
            }
        })
    };

    deleting.join().expect("the thread does not panic");
    reading.join().expect("the thread does not panic");

    assert_eq!(store.storage.count().expect("counts"), 32);
    // Exactly the body files of what is left.
    assert_eq!(store.body_files().len(), 64);
}

#[test]
fn a_failure_in_one_thread_does_not_stop_another() {
    let store = TestStore::open();
    let storage = Arc::new(store.storage.clone());
    // An exchange that is stored twice: the second attempt fails, and must
    // fail on its own rather than taking anything else with it.
    let clash = http_exchange("example.com", "/clash", 200);
    storage.insert(&clash).expect("stores");

    let failing = {
        let storage = Arc::clone(&storage);
        let clash = clash.clone();
        thread::spawn(move || {
            for _ in 0..EACH {
                assert!(storage.insert(&clash).is_err());
            }
        })
    };
    let working = {
        let storage = Arc::clone(&storage);
        thread::spawn(move || {
            for index in 0..EACH {
                storage
                    .insert(&http_exchange("example.com", &format!("/{index}"), 200))
                    .expect("stores");
            }
        })
    };

    failing.join().expect("the thread does not panic");
    working.join().expect("the thread does not panic");

    assert_eq!(store.storage.count().expect("counts"), (EACH + 1) as u64);
}
