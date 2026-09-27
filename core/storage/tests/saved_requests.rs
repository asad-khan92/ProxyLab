//! Saved requests, persisted: stored, read back, renamed, edited, removed.
//!
//! The point of every one of these is that a saved request survives the
//! process that was told about it and comes back as what went in — headers in
//! order, a body byte for byte, and the truncation a captured body carried.
//!
//! Each test opens a store in a temporary directory of its own, so nothing
//! here writes to a developer's own saved requests.

mod support;

use proxy_storage::{
    BodyCapture, HeaderValue, Headers, NewSavedRequest, SavedRequest, SavedRequestId,
    SavedRequestUpdate,
};
use support::{binary, TestStore};

/// A request with headers worth recognising and a JSON body.
fn new_request(name: &str) -> NewSavedRequest {
    let mut request = NewSavedRequest::new(name, "POST", "https://example.com/users");
    request
        .headers
        .push("Content-Type", HeaderValue::Text("application/json".into()));
    request.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer sk-secret".into()),
    );
    request
        .headers
        .push("X-Repeated", HeaderValue::Text("first".into()));
    request
        .headers
        .push("X-Repeated", HeaderValue::Text("second".into()));
    request.body = br#"{"name":"ada"}"#.to_vec();

    request
}

fn saved(name: &str) -> SavedRequest {
    SavedRequest::create(new_request(name)).expect("a saved request")
}

#[test]
fn a_saved_request_is_read_back_as_what_went_in() {
    let store = TestStore::open();
    let request = saved("Create a user");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let back = store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .expect("there is one");

    assert_eq!(back, request);
    assert_eq!(back.headers.len(), 4);
    assert_eq!(back.headers.text("authorization"), Some("Bearer sk-secret"));
    assert_eq!(back.body.as_text(), Some(r#"{"name":"ada"}"#));
}

#[test]
fn a_saved_request_outlives_the_handle_that_stored_it() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let reopened = store.reopen();

    assert_eq!(
        reopened.saved_request(request.id).expect("reads"),
        Some(request)
    );
}

#[test]
fn asking_for_a_request_that_is_not_there_is_not_an_error() {
    let store = TestStore::open();

    assert_eq!(
        store
            .storage
            .saved_request(SavedRequestId::next())
            .expect("reads"),
        None
    );
}

#[test]
fn saved_requests_come_back_in_the_order_they_were_saved() {
    let store = TestStore::open();
    let first = saved("First");
    let second = saved("Second");
    let third = saved("Third");

    for request in [&third, &first, &second] {
        store.storage.insert_saved_request(request).expect("stores");
    }

    let names: Vec<String> = store
        .storage
        .list_saved_requests()
        .expect("lists")
        .into_iter()
        .map(|request| request.name)
        .collect();

    // Saved in the order their ids were minted, whatever order they were
    // written to the database in.
    assert_eq!(names, ["First", "Second", "Third"]);
    assert_eq!(store.storage.count_saved_requests().expect("counts"), 3);
}

#[test]
fn storing_a_request_whose_id_is_already_there_fails_and_changes_nothing() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let mut same_id = saved("Something else");
    same_id.id = request.id;

    assert!(store.storage.insert_saved_request(&same_id).is_err());
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("still there")
            .name,
        "Create a user"
    );
}

#[test]
fn renaming_changes_the_name_and_nothing_else() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let renamed = store
        .storage
        .update_saved_request(request.id, SavedRequestUpdate::name("Create Ada"))
        .expect("updates")
        .expect("there is one");

    assert_eq!(renamed.name, "Create Ada");
    assert_eq!(renamed.method, request.method);
    assert_eq!(renamed.url, request.url);
    assert_eq!(renamed.headers, request.headers);
    assert_eq!(renamed.body, request.body);
    assert_eq!(renamed.created_at_ms, request.created_at_ms);
    assert_eq!(
        store.storage.saved_request(request.id).expect("reads"),
        Some(renamed)
    );
}

#[test]
fn editing_a_request_replaces_the_fields_it_names() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let mut headers = Headers::new();
    headers.push("Accept", HeaderValue::Text("text/plain".into()));

    let edited = store
        .storage
        .update_saved_request(
            request.id,
            SavedRequestUpdate {
                method: Some("PUT".to_owned()),
                url: Some("https://example.com/users/1".to_owned()),
                headers: Some(headers),
                body: Some(b"nothing like JSON".to_vec()),
                ..SavedRequestUpdate::none()
            },
        )
        .expect("updates")
        .expect("there is one");

    assert_eq!(edited.method, "PUT");
    assert_eq!(edited.url, "https://example.com/users/1");
    assert_eq!(edited.headers.len(), 1);
    assert_eq!(edited.body.as_text(), Some("nothing like JSON"));
    assert_eq!(
        store.storage.saved_request(request.id).expect("reads"),
        Some(edited)
    );
}

#[test]
fn updating_a_request_that_is_not_there_says_so() {
    let store = TestStore::open();

    assert_eq!(
        store
            .storage
            .update_saved_request(SavedRequestId::next(), SavedRequestUpdate::name("x"))
            .expect("updates"),
        None
    );
}

#[test]
fn an_update_that_would_leave_it_nameless_is_refused_and_stores_nothing() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert!(store
        .storage
        .update_saved_request(request.id, SavedRequestUpdate::name("  "))
        .is_err());
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("still there")
            .name,
        "Create a user"
    );
}

#[test]
fn deleting_removes_it_and_says_it_did() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert!(store
        .storage
        .delete_saved_request(request.id)
        .expect("deletes"));
    assert_eq!(
        store.storage.saved_request(request.id).expect("reads"),
        None
    );
    assert_eq!(store.storage.count_saved_requests().expect("counts"), 0);

    // Deleting it again is not an error, and says there was nothing to do.
    assert!(!store
        .storage
        .delete_saved_request(request.id)
        .expect("deletes"));
}

#[test]
fn deleting_one_leaves_the_others_alone() {
    let store = TestStore::open();
    let first = saved("First");
    let second = saved("Second");
    store.storage.insert_saved_request(&first).expect("stores");
    store.storage.insert_saved_request(&second).expect("stores");

    store
        .storage
        .delete_saved_request(first.id)
        .expect("deletes");

    let remaining = store.storage.list_saved_requests().expect("lists");
    assert_eq!(remaining, vec![second]);
}

// MARK: - Bodies

#[test]
fn a_small_body_is_kept_in_the_database() {
    let store = TestStore::with_inline_limit(1_024);
    let request = saved("Create a user");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert!(store.body_files().is_empty(), "{:?}", store.body_files());
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("there is one")
            .body
            .as_text(),
        Some(r#"{"name":"ada"}"#)
    );
}

#[test]
fn a_large_body_goes_to_a_file_of_its_own_and_comes_back_whole() {
    let store = TestStore::with_inline_limit(16);
    let bytes = binary(4_096);
    let mut request = new_request("A big one");
    request.body = bytes.clone();
    let request = SavedRequest::create(request).expect("a saved request");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert_eq!(store.body_files(), [format!("{}.saved", request.id)]);
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("there is one")
            .body
            .data(),
        bytes
    );
}

#[test]
fn a_saved_requests_body_file_survives_the_traffic_being_cleared() {
    // A saved request is not traffic: emptying the history must not take the
    // request somebody kept, or the file its body is in.
    let store = TestStore::with_inline_limit(16);
    let mut request = new_request("A big one");
    request.body = binary(4_096);
    let request = SavedRequest::create(request).expect("a saved request");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");
    store
        .storage
        .insert(&support::http_exchange("example.com", "/x", 200))
        .expect("stores an exchange");

    store.storage.clear().expect("clears the traffic");

    assert_eq!(store.body_files(), [format!("{}.saved", request.id)]);
    assert!(store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .is_some());
}

#[test]
fn deleting_a_saved_request_removes_its_body_file() {
    let store = TestStore::with_inline_limit(16);
    let mut request = new_request("A big one");
    request.body = binary(4_096);
    let request = SavedRequest::create(request).expect("a saved request");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    store
        .storage
        .delete_saved_request(request.id)
        .expect("deletes");

    assert!(store.body_files().is_empty(), "{:?}", store.body_files());
}

#[test]
fn shrinking_a_body_out_of_its_file_removes_the_file() {
    let store = TestStore::with_inline_limit(16);
    let mut request = new_request("A big one");
    request.body = binary(4_096);
    let request = SavedRequest::create(request).expect("a saved request");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let edited = store
        .storage
        .update_saved_request(
            request.id,
            SavedRequestUpdate {
                body: Some(b"small".to_vec()),
                ..SavedRequestUpdate::none()
            },
        )
        .expect("updates")
        .expect("there is one");

    assert!(store.body_files().is_empty(), "{:?}", store.body_files());
    assert_eq!(edited.body.as_text(), Some("small"));
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("there is one")
            .body
            .as_text(),
        Some("small")
    );
}

#[test]
fn replacing_a_large_body_with_another_leaves_one_file_holding_the_new_one() {
    let store = TestStore::with_inline_limit(16);
    let mut request = new_request("A big one");
    request.body = binary(4_096);
    let request = SavedRequest::create(request).expect("a saved request");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");
    let replacement = vec![b'z'; 2_048];

    store
        .storage
        .update_saved_request(
            request.id,
            SavedRequestUpdate {
                body: Some(replacement.clone()),
                ..SavedRequestUpdate::none()
            },
        )
        .expect("updates");

    assert_eq!(store.body_files(), [format!("{}.saved", request.id)]);
    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("there is one")
            .body
            .data(),
        replacement
    );
}

#[test]
fn a_request_with_no_body_stores_no_bytes_anywhere() {
    let store = TestStore::with_inline_limit(16);
    let request = SavedRequest::create(NewSavedRequest::new(
        "Just a GET",
        "GET",
        "https://example.com/",
    ))
    .expect("a saved request");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert!(store.body_files().is_empty());
    assert!(store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .expect("there is one")
        .body
        .is_empty());
}

#[test]
fn a_body_saved_from_a_truncated_capture_still_says_it_is_truncated() {
    // Saving a request whose captured body the proxy cut short must not
    // pretend it is the whole body: what is sent again is less than what was
    // sent, and the request has to say so.
    let store = TestStore::open();
    let mut request = saved("From a big upload");
    request.body = BodyCapture::truncated(b"the first bit".to_vec(), 5_000_000);

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let back = store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .expect("there is one");
    assert!(back.body.is_truncated());
    assert_eq!(back.body.size(), 5_000_000);
    assert_eq!(back.body.data(), b"the first bit");
}

#[test]
fn editing_the_body_of_a_truncated_request_makes_it_whole_in_the_database_too() {
    let store = TestStore::open();
    let mut request = saved("From a big upload");
    request.body = BodyCapture::truncated(b"the first bit".to_vec(), 5_000_000);
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    store
        .storage
        .update_saved_request(
            request.id,
            SavedRequestUpdate {
                body: Some(b"all of it".to_vec()),
                ..SavedRequestUpdate::none()
            },
        )
        .expect("updates");

    let back = store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .expect("there is one");
    assert!(!back.body.is_truncated());
    assert_eq!(back.body.size(), 9);
}

#[test]
fn a_binary_body_comes_back_byte_for_byte() {
    let store = TestStore::open();
    let bytes = binary(256);
    let mut request = new_request("Protocol buffers");
    request.body = bytes.clone();
    let request = SavedRequest::create(request).expect("a saved request");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    assert_eq!(
        store
            .storage
            .saved_request(request.id)
            .expect("reads")
            .expect("there is one")
            .body
            .data(),
        bytes
    );
}

#[test]
fn a_header_value_that_is_not_text_comes_back_as_the_bytes_it_was() {
    let store = TestStore::open();
    let mut request = new_request("Odd headers");
    request
        .headers
        .push("X-Raw", HeaderValue::from_bytes(&[0x00, 0x80, 0xff]));
    let request = SavedRequest::create(request).expect("a saved request");

    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    let back = store
        .storage
        .saved_request(request.id)
        .expect("reads")
        .expect("there is one");
    assert_eq!(
        back.headers.get("x-raw").map(HeaderValue::as_bytes),
        Some([0x00, 0x80, 0xff].as_slice())
    );
}

// MARK: - Rows that were not written by this build

#[test]
fn a_row_with_no_name_is_reported_rather_than_handed_back() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    store.execute("UPDATE saved_requests SET name = ?1", "");

    assert!(store.storage.saved_request(request.id).is_err());
    assert!(store.storage.list_saved_requests().is_err());
}

#[test]
fn a_row_whose_headers_are_not_json_is_reported() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");

    store.execute("UPDATE saved_requests SET headers = ?1", "not json");

    assert!(store.storage.saved_request(request.id).is_err());
}

#[test]
fn saved_requests_and_the_traffic_do_not_disturb_each_other() {
    let store = TestStore::open();
    let request = saved("Create a user");
    store
        .storage
        .insert_saved_request(&request)
        .expect("stores");
    store
        .storage
        .insert(&support::http_exchange("example.com", "/x", 200))
        .expect("stores an exchange");

    assert_eq!(store.storage.count().expect("counts"), 1);
    assert_eq!(store.storage.count_saved_requests().expect("counts"), 1);

    store.storage.clear().expect("clears");

    assert_eq!(store.storage.count().expect("counts"), 0);
    assert_eq!(store.storage.count_saved_requests().expect("counts"), 1);
}
