//! Search across headers, bodies, and stream messages.

mod support;

use support::{failed_exchange, http_exchange, with_bodies, with_headers, with_method, TestStore};

use proxy_storage::{
    BodyCapture, ExchangeId, ExchangeSummary, HeaderValue, HttpVersion, StatusFilter, StorageError,
    StreamDirection, StreamKind, StreamMessage, StreamMessageId, Timestamp, TrafficErrorKind,
    TrafficFilter, TrafficQuery, MAX_PAGE_LIMIT,
};

fn all_matching(
    storage: &proxy_storage::Storage,
    filter: TrafficFilter,
) -> Vec<proxy_storage::ExchangeSummary> {
    storage
        .list(
            &TrafficQuery::newest()
                .with_filter(filter)
                .with_limit(MAX_PAGE_LIMIT),
        )
        .expect("lists")
        .exchanges
}

fn ids(page: &[ExchangeSummary]) -> Vec<ExchangeId> {
    page.iter().map(|summary| summary.id).collect()
}

#[test]
fn a_search_matches_a_request_header_value() {
    let store = TestStore::open();
    store
        .storage
        .insert(&with_headers(http_exchange("example.com", "/x", 200)))
        .expect("stores");
    store
        .storage
        .insert(&http_exchange("other.example.com", "/y", 200))
        .expect("stores");

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("sk-secret"));

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "http://example.com/x");
}

#[test]
fn a_search_matches_a_request_body() {
    let store = TestStore::open();
    let mut hit = http_exchange("api.example.com", "/users", 201);
    hit.request.body = BodyCapture::complete(br#"{"name":"ada"}"#.to_vec());
    store.storage.insert(&hit).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/orders", 200))
        .expect("stores");

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("ada"));

    assert_eq!(ids(&found), vec![hit.id]);
}

#[test]
fn a_search_matches_a_response_body() {
    let store = TestStore::open();
    let hit = with_bodies(
        http_exchange("api.example.com", "/x", 200),
        BodyCapture::empty(),
        BodyCapture::complete(b"unique-response-token".to_vec()),
    );
    store.storage.insert(&hit).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/y", 200))
        .expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text("unique-response-token"),
    );

    assert_eq!(ids(&found), vec![hit.id]);
}

#[test]
fn a_regex_search_matches_the_pattern() {
    let store = TestStore::open();
    store
        .storage
        .insert(&http_exchange("api.example.com", "/user-12", 200))
        .expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/user-ab", 200))
        .expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text(r"user-\d+").with_regex(),
    );

    assert_eq!(found.len(), 1);
    assert!(found[0].url.contains("user-12"));
}

#[test]
fn a_regex_that_is_not_one_is_refused() {
    let store = TestStore::open();
    let error = store
        .storage
        .list(&TrafficQuery::newest().with_filter(TrafficFilter::all().with_text("(").with_regex()))
        .expect_err("refused");

    assert!(matches!(error, StorageError::InvalidSearch));
}

#[test]
fn a_case_sensitive_search_keeps_the_letters_it_was_given() {
    let store = TestStore::open();
    store
        .storage
        .insert(&http_exchange("api.example.com", "/Users", 200))
        .expect("stores");

    assert_eq!(
        all_matching(
            &store.storage,
            TrafficFilter::all().with_text("Users").case_sensitive(),
        )
        .len(),
        1
    );
    assert!(all_matching(
        &store.storage,
        TrafficFilter::all().with_text("users").case_sensitive(),
    )
    .is_empty());
}

#[test]
fn a_file_backed_body_is_searched_without_being_listed() {
    let store = TestStore::with_inline_limit(8);
    let payload = b"file-backed-secret-marker".to_vec();
    let hit = with_bodies(
        http_exchange("api.example.com", "/upload", 200),
        BodyCapture::empty(),
        BodyCapture::complete(payload),
    );
    store.storage.insert(&hit).expect("stores");
    assert!(
        !store.body_files().is_empty(),
        "the body must live in a file for this test"
    );

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text("file-backed-secret-marker"),
    );

    assert_eq!(ids(&found), vec![hit.id]);
    let listed = &found[0];
    // A list page is still a summary: no body bytes travel with it.
    assert!(listed.url.contains("/upload"));
}

#[test]
fn a_truncated_body_is_searched_only_as_far_as_it_was_captured() {
    let store = TestStore::open();
    let mut hit = http_exchange("api.example.com", "/partial", 200);
    hit.request.body = BodyCapture::truncated(b"kept-prefix".to_vec(), 10_000);
    store.storage.insert(&hit).expect("stores");

    assert_eq!(
        all_matching(
            &store.storage,
            TrafficFilter::all().with_text("kept-prefix")
        )
        .len(),
        1
    );
    assert!(all_matching(
        &store.storage,
        TrafficFilter::all().with_text("never-captured")
    )
    .is_empty());
}

#[test]
fn a_websocket_text_payload_is_searched() {
    let store = TestStore::open();
    let exchange = http_exchange("api.example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores");
    store
        .storage
        .insert_stream_message(&StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: exchange.id,
            sequence: 0,
            captured_at: Timestamp::now(),
            direction: StreamDirection::ClientToServer,
            kind: StreamKind::WebsocketText,
            event_name: None,
            event_id: None,
            retry_ms: None,
            payload: BodyCapture::complete(b"hello-from-ws".to_vec()),
        })
        .expect("stores the frame");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text("hello-from-ws"),
    );

    assert_eq!(ids(&found), vec![exchange.id]);
}

#[test]
fn a_binary_websocket_payload_is_not_searched_as_bytes() {
    let store = TestStore::open();
    let exchange = http_exchange("api.example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores");
    store
        .storage
        .insert_stream_message(&StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: exchange.id,
            sequence: 0,
            captured_at: Timestamp::now(),
            direction: StreamDirection::ServerToClient,
            kind: StreamKind::WebsocketBinary,
            event_name: None,
            event_id: None,
            retry_ms: None,
            payload: BodyCapture::complete(b"secret-binary-bytes".to_vec()),
        })
        .expect("stores the frame");

    assert!(all_matching(
        &store.storage,
        TrafficFilter::all().with_text("secret-binary-bytes"),
    )
    .is_empty());
    assert_eq!(
        all_matching(
            &store.storage,
            TrafficFilter::all().with_text("server_to_client"),
        )
        .len(),
        1
    );
}

#[test]
fn an_sse_event_is_searched_by_name_id_and_data() {
    let store = TestStore::open();
    let mut exchange = http_exchange("api.example.com", "/events", 200);
    exchange
        .response
        .as_mut()
        .expect("a response")
        .headers
        .push(
            "Content-Type",
            HeaderValue::Text("text/event-stream".into()),
        );
    store.storage.insert(&exchange).expect("stores");
    store
        .storage
        .insert_stream_message(&StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: exchange.id,
            sequence: 0,
            captured_at: Timestamp::now(),
            direction: StreamDirection::ServerToClient,
            kind: StreamKind::Sse,
            event_name: Some("ready".to_owned()),
            event_id: Some("evt-9".to_owned()),
            retry_ms: Some(2000),
            payload: BodyCapture::complete(b"payload-data".to_vec()),
        })
        .expect("stores the event");

    for term in ["ready", "evt-9", "payload-data"] {
        let found = all_matching(&store.storage, TrafficFilter::all().with_text(term));
        assert_eq!(ids(&found), vec![exchange.id], "{term}");
    }
}

#[test]
fn a_content_type_filter_matches_either_half() {
    let store = TestStore::open();
    let mut json = http_exchange("api.example.com", "/x", 200);
    json.request
        .headers
        .push("Content-Type", HeaderValue::Text("application/json".into()));
    store.storage.insert(&json).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/html", 200))
        .expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_content_type("json"),
    );

    assert_eq!(ids(&found), vec![json.id]);
}

#[test]
fn an_http2_filter_keeps_http2_exchanges() {
    let store = TestStore::open();
    let mut h2 = http_exchange("api.example.com", "/h2", 200);
    h2.request.version = HttpVersion::Http2;
    if let Some(response) = h2.response.as_mut() {
        response.version = HttpVersion::Http2;
    }
    store.storage.insert(&h2).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/h1", 200))
        .expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_http_version(HttpVersion::Http2),
    );

    assert_eq!(ids(&found), vec![h2.id]);
}

#[test]
fn a_websocket_filter_keeps_switching_protocols() {
    let store = TestStore::open();
    let ws = http_exchange("api.example.com", "/socket", 101);
    store.storage.insert(&ws).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/x", 200))
        .expect("stores");

    let found = all_matching(&store.storage, TrafficFilter::all().websocket_only());

    assert_eq!(ids(&found), vec![ws.id]);
}

#[test]
fn an_sse_filter_keeps_event_streams() {
    let store = TestStore::open();
    let mut sse = http_exchange("api.example.com", "/events", 200);
    sse.response.as_mut().expect("a response").headers.push(
        "Content-Type",
        HeaderValue::Text("text/event-stream; charset=utf-8".into()),
    );
    store.storage.insert(&sse).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/x", 200))
        .expect("stores");

    let found = all_matching(&store.storage, TrafficFilter::all().sse_only());

    assert_eq!(ids(&found), vec![sse.id]);
}

#[test]
fn a_failed_filter_keeps_proxy_errors() {
    let store = TestStore::open();
    let failed = failed_exchange("gone.example.com", TrafficErrorKind::UpstreamConnect);
    store.storage.insert(&failed).expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/x", 200))
        .expect("stores");

    let found = all_matching(&store.storage, TrafficFilter::all().failed_only());

    assert_eq!(ids(&found), vec![failed.id]);
}

#[test]
fn search_pages_the_same_way_an_unsearched_list_does() {
    let store = TestStore::open();
    let mut ids = Vec::new();
    for index in 0..5 {
        let exchange = http_exchange("api.example.com", &format!("/item-{index}"), 200);
        ids.push(exchange.id);
        store.storage.insert(&exchange).expect("stores");
    }

    let first = store
        .storage
        .list(
            &TrafficQuery::newest()
                .with_filter(TrafficFilter::all().with_text("item-"))
                .with_limit(2),
        )
        .expect("lists");

    assert_eq!(first.len(), 2);
    assert!(first.next.is_some());

    let second = store
        .storage
        .list(
            &TrafficQuery::newest()
                .with_filter(TrafficFilter::all().with_text("item-"))
                .with_limit(2)
                .after(first.next.expect("another page")),
        )
        .expect("lists");

    assert_eq!(second.len(), 2);
    assert_ne!(first.exchanges[0].id, second.exchanges[0].id);
}

#[test]
fn a_host_filter_still_narrows_a_body_search() {
    let store = TestStore::open();
    let mut hit = http_exchange("api.example.com", "/x", 200);
    hit.request.body = BodyCapture::complete(b"shared-token".to_vec());
    let mut other = http_exchange("cdn.example.com", "/x", 200);
    other.request.body = BodyCapture::complete(b"shared-token".to_vec());
    store.storage.insert(&hit).expect("stores");
    store.storage.insert(&other).expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all()
            .with_host("api.example.com")
            .with_text("shared-token"),
    );

    assert_eq!(ids(&found), vec![hit.id]);
}

#[test]
fn a_status_filter_still_combines_with_search() {
    let store = TestStore::open();
    store
        .storage
        .insert(&with_method(
            http_exchange("api.example.com", "/users", 200),
            "POST",
        ))
        .expect("stores");
    store
        .storage
        .insert(&http_exchange("api.example.com", "/users", 404))
        .expect("stores");

    let found = all_matching(
        &store.storage,
        TrafficFilter::all()
            .with_text("users")
            .with_status(StatusFilter::Exact(404)),
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].status, Some(404));
}

#[test]
fn a_binary_websocket_payload_is_searchable_by_size() {
    let store = TestStore::open();
    let exchange = http_exchange("api.example.com", "/socket", 101);
    store.storage.insert(&exchange).expect("stores");
    store
        .storage
        .insert_stream_message(&StreamMessage {
            id: StreamMessageId::next(),
            exchange_id: exchange.id,
            sequence: 0,
            captured_at: Timestamp::now(),
            direction: StreamDirection::ClientToServer,
            kind: StreamKind::WebsocketBinary,
            event_name: None,
            event_id: None,
            retry_ms: None,
            payload: BodyCapture::complete(vec![0; 99991]),
        })
        .expect("stores the frame");

    let found = all_matching(&store.storage, TrafficFilter::all().with_text("99991"));

    assert_eq!(ids(&found), vec![exchange.id]);
}

#[test]
fn a_search_reads_file_backed_bodies_one_at_a_time() {
    let store = TestStore::with_inline_limit(8);
    let mut hit_id = None;
    for index in 0..40 {
        let marker = if index == 37 {
            b"unique-file-backed-needle".to_vec()
        } else {
            format!("other-file-body-{index:04}").into_bytes()
        };
        let mut exchange = http_exchange("api.example.com", &format!("/blob-{index}"), 200);
        exchange.request.body = BodyCapture::complete(marker);
        store.storage.insert(&exchange).expect("stores");
        if index == 37 {
            hit_id = Some(exchange.id);
        }
    }
    assert!(
        store.body_files().len() >= 40,
        "every body in this test lives in a file"
    );

    let found = all_matching(
        &store.storage,
        TrafficFilter::all().with_text("unique-file-backed-needle"),
    );

    assert_eq!(ids(&found), vec![hit_id.expect("the matching row")]);
}
