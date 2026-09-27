//! Storing exchanges, and getting exactly what was stored back.
//!
//! Every test here stores an exchange and reads it back, because the two
//! halves are one question: a store that accepts an exchange and returns
//! something else has not stored it.

mod support;

use support::{
    binary, failed_exchange, http_exchange, https_exchange, unanswered_exchange, with_bodies,
    with_headers, TestStore,
};

use proxy_storage::{
    BodyCapture, HeaderValue, Scheme, StorageError, TrafficError, TrafficErrorKind, TrafficExchange,
};

/// Stores `exchange` and reads it back, which is the shape of most of these
/// tests.
fn round_trip(store: &TestStore, exchange: &TrafficExchange) -> TrafficExchange {
    store.storage.insert(exchange).expect("stores");

    store
        .storage
        .get(exchange.id)
        .expect("reads")
        .expect("the exchange is stored")
}

#[test]
fn a_plaintext_exchange_comes_back_as_it_went_in() {
    let store = TestStore::open();
    let exchange = with_headers(with_bodies(
        http_exchange("example.com", "/api/users?active=true", 200),
        BodyCapture::complete(br#"{"name":"ada"}"#.to_vec()),
        BodyCapture::complete(br#"{"id":1}"#.to_vec()),
    ));

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored, exchange);
    assert_eq!(stored.connection.scheme, Scheme::Http);
}

#[test]
fn an_intercepted_https_exchange_comes_back_as_it_went_in() {
    let store = TestStore::open();
    let exchange = with_headers(with_bodies(
        https_exchange("secure.example.com", "/token", 201),
        BodyCapture::complete(b"grant_type=password".to_vec()),
        BodyCapture::complete(br#"{"access_token":"sk-secret"}"#.to_vec()),
    ));

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored, exchange);
    assert_eq!(stored.connection.scheme, Scheme::Https);
    assert_eq!(
        stored.connection.destination.as_ref().map(|to| to.port),
        Some(443)
    );
}

#[test]
fn an_exchange_with_no_response_comes_back_with_no_response() {
    let store = TestStore::open();
    let exchange = unanswered_exchange("example.com");

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored, exchange);
    assert_eq!(stored.response, None);
    assert_eq!(stored.error, None);
    assert_eq!(stored.timing.completed_micros, None);
    // The request half is still all there: that is the point of storing an
    // exchange that never finished.
    assert_eq!(stored.request.method, "POST");
}

#[test]
fn an_exchange_that_failed_comes_back_with_the_failure_that_was_recorded() {
    let store = TestStore::open();
    let mut exchange = failed_exchange("example.com", TrafficErrorKind::UpstreamTls);
    exchange.error = Some(TrafficError::new(
        TrafficErrorKind::UpstreamTls,
        "the origin's certificate did not verify",
    ));

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored, exchange);
    let error = stored.error.expect("an error");
    assert_eq!(error.kind(), TrafficErrorKind::UpstreamTls);
    assert_eq!(error.summary(), "the origin's certificate did not verify");
}

#[test]
fn every_kind_of_failure_comes_back_as_the_kind_it_was() {
    let store = TestStore::open();

    for kind in [
        TrafficErrorKind::RequestRejected,
        TrafficErrorKind::ProxyFailure,
        TrafficErrorKind::UpstreamConnect,
        TrafficErrorKind::UpstreamTls,
        TrafficErrorKind::ClientTls,
        TrafficErrorKind::UpstreamHttp,
        TrafficErrorKind::ResponseForwarding,
        TrafficErrorKind::ClientDisconnected,
        TrafficErrorKind::Incomplete,
    ] {
        let exchange = failed_exchange("example.com", kind);

        let stored = round_trip(&store, &exchange);

        assert_eq!(stored.error.map(|error| error.kind()), Some(kind));
    }
}

#[test]
fn request_headers_come_back_in_order_with_every_repetition_and_every_byte() {
    let store = TestStore::open();
    let exchange = with_headers(http_exchange("example.com", "/x", 200));

    let stored = round_trip(&store, &exchange);

    let headers = &stored.request.headers;
    assert_eq!(headers, &exchange.request.headers);
    let names: Vec<&str> = headers.iter().map(|header| header.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Accept",
            "Authorization",
            "X-Repeated",
            "X-Repeated",
            "X-Raw"
        ]
    );
    let repeated: Vec<Option<&str>> = headers
        .get_all("x-repeated")
        .map(HeaderValue::as_text)
        .collect();
    assert_eq!(repeated, [Some("first"), Some("second")]);
    assert_eq!(
        headers.get("x-raw"),
        Some(&HeaderValue::Bytes(vec![0x00, 0x80, 0xff]))
    );
    // The credentials a debugging proxy exists to show are kept.
    assert_eq!(headers.text("authorization"), Some("Bearer sk-secret"));
}

#[test]
fn response_headers_come_back_in_order_with_every_repetition() {
    let store = TestStore::open();
    let exchange = with_headers(http_exchange("example.com", "/x", 200));

    let stored = round_trip(&store, &exchange);

    let headers = &stored.response.as_ref().expect("a response").headers;
    let cookies: Vec<Option<&str>> = headers
        .get_all("set-cookie")
        .map(HeaderValue::as_text)
        .collect();
    assert_eq!(cookies, [Some("session=abc"), Some("theme=dark")]);
    assert_eq!(headers.text("content-type"), Some("application/json"));
}

#[test]
fn an_exchange_with_no_headers_at_all_comes_back_with_none() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 204);

    let stored = round_trip(&store, &exchange);

    assert!(stored.request.headers.is_empty());
    assert!(stored
        .response
        .as_ref()
        .expect("a response")
        .headers
        .is_empty());
}

#[test]
fn empty_bodies_come_back_empty() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 204);

    let stored = round_trip(&store, &exchange);

    assert!(stored.request.body.is_empty());
    assert_eq!(stored.request.body.size(), 0);
    assert!(!stored.request.body.is_truncated());
    assert!(stored
        .response
        .as_ref()
        .expect("a response")
        .body
        .is_empty());
    // Nothing was captured, so nothing was written.
    assert!(store.body_files().is_empty());
}

#[test]
fn binary_bodies_come_back_byte_for_byte() {
    let store = TestStore::open();
    let bytes = binary(1_024);
    let exchange = with_bodies(
        http_exchange("example.com", "/download", 200),
        BodyCapture::complete(bytes.clone()),
        BodyCapture::complete(bytes.clone()),
    );

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored.request.body.data(), bytes);
    assert_eq!(
        stored.response.as_ref().expect("a response").body.data(),
        bytes
    );
    assert_eq!(stored.request.body.as_text(), None);
}

#[test]
fn a_truncated_body_stores_what_was_captured_and_how_long_the_body_really_was() {
    let store = TestStore::open();
    // What Step 7 produces for a 20 MB body captured under a 1 MB limit: the
    // first mebibyte, and the real size.
    let captured = vec![b'a'; 1024 * 1024];
    let real_size = 20 * 1024 * 1024;
    let exchange = with_bodies(
        http_exchange("example.com", "/big", 200),
        BodyCapture::empty(),
        BodyCapture::truncated(captured.clone(), real_size),
    );

    let stored = round_trip(&store, &exchange);

    let body = &stored.response.as_ref().expect("a response").body;
    assert_eq!(body.captured_len(), captured.len());
    assert_eq!(body.data(), captured);
    assert_eq!(body.size(), real_size);
    assert!(body.is_truncated());
    // Nothing was invented to fill the gap.
    assert!(
        (body.captured_len() as u64) < body.size(),
        "the capture grew to the size of the body"
    );
}

#[test]
fn a_body_captured_under_a_limit_of_zero_stores_its_size_and_no_bytes() {
    let store = TestStore::open();
    let exchange = with_bodies(
        http_exchange("example.com", "/big", 200),
        BodyCapture::truncated(Vec::new(), 4_096),
        BodyCapture::truncated(Vec::new(), 8_192),
    );

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored.request.body.captured_len(), 0);
    assert_eq!(stored.request.body.size(), 4_096);
    assert!(stored.request.body.is_truncated());
    assert_eq!(
        stored.response.as_ref().expect("a response").body.size(),
        8_192
    );
    assert!(store.body_files().is_empty());
}

#[test]
fn a_body_within_the_inline_limit_is_kept_in_the_database() {
    let store = TestStore::with_inline_limit(1_024);
    let exchange = with_bodies(
        http_exchange("example.com", "/small", 200),
        BodyCapture::complete(b"small request".to_vec()),
        BodyCapture::complete(b"small response".to_vec()),
    );

    let stored = round_trip(&store, &exchange);

    assert!(
        store.body_files().is_empty(),
        "a small body was written to a file: {:?}",
        store.body_files()
    );
    assert_eq!(stored.request.body.data(), b"small request");
    assert_eq!(
        stored.response.as_ref().expect("a response").body.data(),
        b"small response"
    );
}

#[test]
fn a_body_over_the_inline_limit_is_kept_in_a_file_of_its_own() {
    let store = TestStore::with_inline_limit(16);
    let request_bytes = binary(4_096);
    let response_bytes = binary(8_192);
    let exchange = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(request_bytes.clone()),
        BodyCapture::complete(response_bytes.clone()),
    );
    let id = exchange.id;

    let stored = round_trip(&store, &exchange);

    assert_eq!(
        store.body_files(),
        [format!("{id}.request"), format!("{id}.response")],
        "a large body was not written to a file"
    );
    // What comes back is the captured bytes, whichever side of the limit
    // they were stored on.
    assert_eq!(stored.request.body.data(), request_bytes);
    assert_eq!(
        stored.response.as_ref().expect("a response").body.data(),
        response_bytes
    );
    assert_eq!(stored, exchange);
}

#[test]
fn a_body_reference_names_a_file_that_is_there_and_holds_the_body() {
    let store = TestStore::with_inline_limit(0);
    let bytes = binary(2_048);
    let exchange = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(bytes.clone()),
        BodyCapture::empty(),
    );
    let id = exchange.id;

    round_trip(&store, &exchange);

    let path = store.config().bodies.join(format!("{id}.request"));
    assert!(path.is_file(), "the referenced body file is not there");
    assert_eq!(std::fs::read(&path).expect("readable"), bytes);
}

#[test]
fn a_file_backed_body_survives_the_database_being_reopened() {
    let store = TestStore::with_inline_limit(8);
    let bytes = binary(4_096);
    let exchange = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(bytes.clone()),
        BodyCapture::empty(),
    );
    let id = exchange.id;
    store.storage.insert(&exchange).expect("stores");

    let reopened = store.reopen();

    let stored = reopened.get(id).expect("reads").expect("still stored");
    assert_eq!(stored.request.body.data(), bytes);
}

#[test]
fn a_truncated_file_backed_body_keeps_both_its_bytes_and_its_real_size() {
    let store = TestStore::with_inline_limit(64);
    let captured = binary(4_096);
    let exchange = with_bodies(
        http_exchange("example.com", "/huge", 200),
        BodyCapture::empty(),
        BodyCapture::truncated(captured.clone(), 20 * 1024 * 1024),
    );

    let stored = round_trip(&store, &exchange);

    let body = &stored.response.as_ref().expect("a response").body;
    assert_eq!(body.data(), captured);
    assert_eq!(body.size(), 20 * 1024 * 1024);
    assert!(body.is_truncated());
}

#[test]
fn a_failed_insert_leaves_no_body_file_and_no_row_behind() {
    let store = TestStore::with_inline_limit(0);
    let first = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(b"the stored body".to_vec()),
        BodyCapture::empty(),
    );
    let id = first.id;
    store.storage.insert(&first).expect("stores");
    let files_before = store.body_files();

    // The same id again: an id identifies one exchange, so this is refused.
    // Its body file was written before the database was touched, and must be
    // taken back — without disturbing the body file of the exchange that is
    // already stored under that name.
    let again = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(b"a body that must not survive".to_vec()),
        BodyCapture::empty(),
    );
    let mut again = again;
    again.id = id;

    let error = store
        .storage
        .insert(&again)
        .expect_err("an id identifies one exchange");

    assert!(matches!(error, StorageError::Database(_)), "{error:?}");
    assert_eq!(
        store.body_files(),
        files_before,
        "a failed insert left a body file behind"
    );
    assert_eq!(store.storage.count().expect("counts"), 1);
    // The exchange that was already stored is untouched.
    let stored = store.storage.get(id).expect("reads").expect("still stored");
    assert_eq!(stored.request.body.data(), b"the stored body");
}

#[test]
fn an_insert_that_cannot_write_its_body_file_fails_without_storing_anything() {
    let store = TestStore::with_inline_limit(0);
    let bodies = store.config().bodies;
    // Something that is not a directory where the body directory was: every
    // body that needs a file now fails to be written.
    std::fs::remove_dir_all(&bodies).expect("the body directory goes");
    std::fs::write(&bodies, b"not a directory").expect("a file in its place");
    let exchange = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(b"a body that needs a file".to_vec()),
        BodyCapture::empty(),
    );

    let error = store
        .storage
        .insert(&exchange)
        .expect_err("there is nowhere to write the body");

    assert!(matches!(error, StorageError::Body { .. }), "{error:?}");
    assert_eq!(store.storage.count().expect("counts"), 0);
    assert_eq!(store.storage.get(exchange.id).expect("reads"), None);
}

#[test]
fn a_body_file_that_has_gone_missing_is_a_failure_and_not_a_wrong_body() {
    let store = TestStore::with_inline_limit(0);
    let exchange = with_bodies(
        http_exchange("example.com", "/large", 200),
        BodyCapture::complete(b"a body in a file".to_vec()),
        BodyCapture::empty(),
    );
    let id = exchange.id;
    store.storage.insert(&exchange).expect("stores");
    std::fs::remove_file(store.config().bodies.join(format!("{id}.request")))
        .expect("the body file goes");

    let error = store.storage.get(id).expect_err("the body is not there");

    assert!(matches!(error, StorageError::Body { .. }), "{error:?}");
}

#[test]
fn every_stored_exchange_keeps_its_own_identity() {
    let store = TestStore::open();
    let first = http_exchange("example.com", "/one", 200);
    let second = http_exchange("example.com", "/two", 200);

    store.storage.insert(&first).expect("stores");
    store.storage.insert(&second).expect("stores");

    assert_ne!(first.id, second.id);
    assert_eq!(
        store
            .storage
            .get(first.id)
            .expect("reads")
            .map(|stored| stored.request.url),
        Some("http://example.com/one".to_owned())
    );
    assert_eq!(
        store
            .storage
            .get(second.id)
            .expect("reads")
            .map(|stored| stored.request.url),
        Some("http://example.com/two".to_owned())
    );
}

#[test]
fn timing_comes_back_as_it_was_measured() {
    let store = TestStore::open();
    let exchange = http_exchange("example.com", "/x", 200);

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored.timing, exchange.timing);
    assert_eq!(stored.timing.upstream_connect_micros, Some(120));
    assert_eq!(stored.timing.upstream_response_micros, Some(2_000));
    assert_eq!(stored.timing.completed_micros, Some(3_500));
    assert_eq!(stored.duration_ms(), Some(3.5));
}

#[test]
fn an_exchange_with_no_destination_comes_back_without_one() {
    let store = TestStore::open();
    let mut exchange = http_exchange("example.com", "/x", 200);
    exchange.connection.destination = None;

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored.connection.destination, None);
    assert_eq!(stored, exchange);
}

#[test]
fn an_ipv6_client_and_destination_come_back_as_they_were() {
    let store = TestStore::open();
    let mut exchange = http_exchange("::1", "/x", 200);
    exchange.connection.client = "[::1]:54321".parse().expect("a valid address");
    exchange.connection.destination = Some(proxy_storage::Destination::new("::1", 8080));

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored, exchange);
}

#[test]
fn a_response_with_no_reason_phrase_comes_back_without_one() {
    let store = TestStore::open();
    let mut exchange = http_exchange("example.com", "/x", 599);
    exchange.response.as_mut().expect("a response").reason = None;

    let stored = round_trip(&store, &exchange);

    assert_eq!(stored.response.as_ref().expect("a response").reason, None);
    assert_eq!(stored.status(), Some(599));
}

#[test]
fn reading_an_exchange_that_was_never_stored_is_not_a_failure() {
    let store = TestStore::open();

    assert_eq!(
        store
            .storage
            .get(proxy_storage::ExchangeId::next())
            .expect("reads"),
        None
    );
}
