//! HAR export and import over the socket, as a client speaks them.
//!
//! The protocol by hand, over a real Unix socket, against a real server with
//! a real database behind it — and a real file, in a temporary directory of
//! the test's own. The file is the point of these two operations: a HAR
//! document is the captured traffic with its bodies in it, which does not fit
//! in one 64 KiB request line, so the client names a path and the core is
//! what writes or reads it.
//!
//! What HAR 1.2 *is* — the entry, the timings, the encoding of a body that is
//! not text — is tested in `proxy-har`, with no socket and no database. Here
//! the questions are the boundary's: does the export contain the exchanges
//! that were asked for, does an import put them back, and is a path a client
//! named checked before anything is opened.

mod support;

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use support::{at, exchange, request, request_with, secretive_exchange, started_at, TestServer};

/// A path in the server's own temporary directory, which goes with the test.
fn path_in(server: &TestServer, name: &str) -> PathBuf {
    server
        .socket()
        .parent()
        .expect("the socket is in a directory")
        .join(name)
}

/// Reads the HAR document at `path`.
fn har(path: &PathBuf) -> Value {
    let text = fs::read_to_string(path).expect("the file is there");

    serde_json::from_str(&text).expect("it is JSON")
}

#[tokio::test]
async fn an_export_names_the_exchanges_it_wrote() {
    let server = TestServer::start().await;
    let first = server.store(&exchange("example.com", "/one", 200));
    let second = server.store(&exchange("example.com", "/two", 404));
    server.store(&exchange("other.example", "/three", 500));
    let path = path_in(&server, "selected.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({
                "path": path.to_string_lossy(),
                "exchange_ids": [first.to_string(), second.to_string()],
            }),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["type"], "export_har");
    assert_eq!(response["result"]["entries"], 2);
    assert_eq!(response["result"]["truncated"], false);
    assert!(response["result"]["bytes"].as_u64().expect("a size") > 0);

    let document = har(&path);
    assert_eq!(document["log"]["version"], "1.2");
    assert_eq!(document["log"]["creator"]["name"], "ProxyLab");
    let entries = document["log"]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["request"]["url"], "http://example.com/one");
    assert_eq!(entries[1]["response"]["status"], 404);

    server.stop().await;
}

#[tokio::test]
async fn an_export_with_no_ids_writes_everything_the_filters_match() {
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/one", 200));
    server.store(&exchange("example.com", "/two", 200));
    server.store(&exchange("other.example", "/three", 200));
    let path = path_in(&server, "filtered.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "host": "example.com"}),
        ))
        .await;

    assert_eq!(response["result"]["entries"], 2, "{response}");
    let entries = har(&path)["log"]["entries"]
        .as_array()
        .expect("entries")
        .len();
    assert_eq!(entries, 2);

    server.stop().await;
}

#[tokio::test]
async fn an_export_with_no_ids_and_no_filters_writes_the_whole_history() {
    let server = TestServer::start().await;
    for path in ["/one", "/two", "/three"] {
        server.store(&exchange("example.com", path, 200));
    }
    let file = path_in(&server, "all.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": file.to_string_lossy()}),
        ))
        .await;

    assert_eq!(response["result"]["entries"], 3, "{response}");

    server.stop().await;
}

#[tokio::test]
async fn a_filtered_export_is_in_the_order_the_traffic_happened() {
    // Oldest first, which is the order every other HAR file is in and the
    // order a waterfall is drawn from.
    let server = TestServer::start().await;
    server.store(&started_at(
        exchange("example.com", "/late", 200),
        at(3_000),
    ));
    server.store(&started_at(
        exchange("example.com", "/early", 200),
        at(1_000),
    ));
    server.store(&started_at(
        exchange("example.com", "/middle", 200),
        at(2_000),
    ));
    let path = path_in(&server, "ordered.har");
    let mut client = server.connect().await;

    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    let document = har(&path);
    let urls: Vec<&str> = document["log"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["request"]["url"].as_str().expect("a url"))
        .collect();
    assert_eq!(
        urls,
        [
            "http://example.com/early",
            "http://example.com/middle",
            "http://example.com/late",
        ]
    );

    server.stop().await;
}

#[tokio::test]
async fn an_export_carries_what_the_capture_carried() {
    // Credentials included. A HAR file is for reproducing the traffic, and
    // one that dropped the `Authorization` would reproduce nothing — which
    // is also exactly why exporting is something a person asks for by name.
    let server = TestServer::start().await;
    let id = server.store(&secretive_exchange("api.example.com"));
    let path = path_in(&server, "secrets.har");
    let mut client = server.connect().await;

    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "exchange_ids": [id.to_string()]}),
        ))
        .await;

    let text = fs::read_to_string(&path).expect("the file is there");
    assert!(
        text.contains("Bearer sk-secret"),
        "the credential was dropped"
    );
    assert!(text.contains("session=hunter2"), "the cookie was dropped");

    server.stop().await;
}

#[tokio::test]
async fn an_export_of_nothing_matching_is_a_har_file_with_no_entries() {
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/one", 200));
    let path = path_in(&server, "empty.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "host": "nothing.example"}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["entries"], 0);
    assert_eq!(har(&path)["log"]["entries"], json!([]));

    server.stop().await;
}

#[tokio::test]
async fn an_exchange_deleted_since_it_was_selected_is_left_out_rather_than_losing_the_export() {
    let server = TestServer::start().await;
    let kept = server.store(&exchange("example.com", "/one", 200));
    let gone = proxy_storage::ExchangeId::next();
    let path = path_in(&server, "partial.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({
                "path": path.to_string_lossy(),
                "exchange_ids": [kept.to_string(), gone.to_string()],
            }),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["entries"], 1);

    server.stop().await;
}

// MARK: - Importing

#[tokio::test]
async fn what_was_exported_is_what_is_imported() {
    let server = TestServer::start().await;
    server.store(&secretive_exchange("api.example.com"));
    server.store(&exchange("example.com", "/two", 404));
    let path = path_in(&server, "round-trip.har");
    let mut client = server.connect().await;

    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;
    client.request(&request("2", "clear_traffic")).await;

    let imported = client
        .request(&request_with(
            "3",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    assert_eq!(imported["ok"], true, "{imported}");
    assert_eq!(imported["type"], "import_har");
    assert_eq!(imported["result"]["imported"], 2);
    assert_eq!(imported["result"]["skipped"], 0);

    let listed = client.request(&request("4", "list_traffic")).await;
    assert_eq!(listed["result"]["total"], 2);

    server.stop().await;
}

#[tokio::test]
async fn an_imported_exchange_is_inspected_like_any_other() {
    let server = TestServer::start().await;
    server.store(&secretive_exchange("api.example.com"));
    let path = path_in(&server, "one.har");
    let mut client = server.connect().await;
    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;
    client.request(&request("2", "clear_traffic")).await;
    client
        .request(&request_with(
            "3",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    let listed = client.request(&request("4", "list_traffic")).await;
    let id = listed["result"]["exchanges"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let detail = client
        .request(&request_with(
            "5",
            "get_traffic",
            json!({"exchange_id": id}),
        ))
        .await;

    let exchange = &detail["result"]["exchange"];
    assert_eq!(exchange["request"]["method"], "GET");
    assert_eq!(exchange["request"]["url"], "http://api.example.com/orders");
    assert_eq!(exchange["response"]["status"], 201);
    assert_eq!(
        exchange["request"]["headers"][0]["value"]["text"],
        "Bearer sk-secret"
    );
    assert_eq!(
        exchange["request"]["body"]["data"],
        "cGFzc3dvcmQ9aHVudGVyMg=="
    );
    assert_eq!(
        exchange["connection"]["destination"]["host"],
        "api.example.com"
    );

    server.stop().await;
}

#[tokio::test]
async fn an_import_gives_every_entry_an_id_of_its_own() {
    // Importing the same file twice is two sets of rows, not a failure: an
    // imported exchange is a new record, and the id it had somewhere else is
    // not this store's.
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/one", 200));
    let path = path_in(&server, "twice.har");
    let mut client = server.connect().await;
    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    for id in ["2", "3"] {
        let imported = client
            .request(&request_with(
                id,
                "import_har",
                json!({"path": path.to_string_lossy()}),
            ))
            .await;
        assert_eq!(imported["result"]["imported"], 1, "{imported}");
    }

    let listed = client.request(&request("4", "list_traffic")).await;
    assert_eq!(listed["result"]["total"], 3);

    server.stop().await;
}

#[tokio::test]
async fn an_import_sends_nothing_and_publishes_nothing() {
    // The whole of what "read-only" means: reading a file somebody sent you
    // must not make the requests in it, and must not put a row on the event
    // stream that says the proxy carried one.
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/one", 200));
    let path = path_in(&server, "quiet.har");
    let mut client = server.connect().await;
    client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;
    let subscribed = client.request(&request("2", "subscribe")).await;
    assert_eq!(subscribed["result"]["subscribed"], true);

    client
        .request(&request_with(
            "3",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    // The next message is the answer to this request, not an event: an event
    // would arrive first and be read here instead.
    let listed = client.request(&request("4", "list_traffic")).await;
    assert_eq!(listed["type"], "list_traffic", "{listed}");

    server.stop().await;
}

#[tokio::test]
async fn an_entry_that_is_not_a_request_is_counted_rather_than_imported() {
    let server = TestServer::start().await;
    let path = path_in(&server, "partial.har");
    fs::write(
        &path,
        br#"{"log":{"version":"1.2","creator":{"name":"x","version":"1"},"entries":[
            {"startedDateTime":"2026-09-20T10:11:12.345Z","time":1,
             "request":{"method":"GET","url":"https://example.com/"},
             "response":{"status":200},"cache":{},"timings":{"send":0,"wait":1,"receive":0}},
            {"startedDateTime":"2026-09-20T10:11:13.345Z","time":1,
             "request":{"method":"","url":""},
             "response":{"status":200},"cache":{},"timings":{"send":0,"wait":1,"receive":0}}
        ]}}"#,
    )
    .expect("writes");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    assert_eq!(response["result"]["imported"], 1, "{response}");
    assert_eq!(response["result"]["skipped"], 1);

    server.stop().await;
}

#[tokio::test]
async fn a_har_file_from_another_tool_is_read() {
    let server = TestServer::start().await;
    let path = path_in(&server, "chrome.har");
    fs::write(
        &path,
        br#"{"log":{"version":"1.2","creator":{"name":"WebInspector","version":"537.36"},
        "pages":[],"entries":[{
            "_initiator":{"type":"script"},
            "startedDateTime":"2026-09-20T12:11:12.345+02:00",
            "time":25.5,
            "request":{"method":"POST","url":"https://api.example.com/v1/items?page=2",
                "httpVersion":"h2","cookies":[],
                "headers":[{"name":"content-type","value":"application/json"}],
                "queryString":[{"name":"page","value":"2"}],
                "postData":{"mimeType":"application/json","text":"{\"a\":1}"},
                "headersSize":-1,"bodySize":7},
            "response":{"status":201,"statusText":"","httpVersion":"h2","cookies":[],
                "headers":[{"name":"content-type","value":"application/json"}],
                "content":{"size":11,"mimeType":"application/json","text":"{\"ok\":true}"},
                "redirectURL":"","headersSize":-1,"bodySize":11},
            "cache":{},
            "timings":{"blocked":1,"dns":-1,"connect":5,"send":0,"wait":20,"receive":0.5}
        }]}}"#,
    )
    .expect("writes");
    let mut client = server.connect().await;

    let imported = client
        .request(&request_with(
            "1",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;
    assert_eq!(imported["result"]["imported"], 1, "{imported}");

    let listed = client.request(&request("2", "list_traffic")).await;
    let id = listed["result"]["exchanges"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let detail = client
        .request(&request_with(
            "3",
            "get_traffic",
            json!({"exchange_id": id}),
        ))
        .await;

    let exchange = &detail["result"]["exchange"];
    assert_eq!(exchange["request"]["method"], "POST");
    assert_eq!(exchange["request"]["version"], "HTTP/2.0");
    assert_eq!(exchange["request"]["body"]["data"], "eyJhIjoxfQ==");
    assert_eq!(exchange["response"]["status"], 201);
    assert_eq!(exchange["connection"]["destination"]["port"], 443);
    assert_eq!(exchange["connection"]["scheme"], "https");

    server.stop().await;
}

// MARK: - Refusals

#[tokio::test]
async fn a_relative_path_is_refused_without_writing_anything() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": "traffic.har"}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "har_unavailable");
    assert!(!PathBuf::from("traffic.har").exists());

    server.stop().await;
}

#[tokio::test]
async fn a_path_in_a_folder_that_is_not_there_is_refused_rather_than_created() {
    let server = TestServer::start().await;
    let path = path_in(&server, "nowhere").join("traffic.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "har_unavailable");
    assert!(!path_in(&server, "nowhere").exists());

    server.stop().await;
}

#[tokio::test]
async fn an_empty_path_and_one_with_a_control_character_are_refused_at_the_boundary() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for path in ["", "  ", "/tmp/one\nline.har"] {
        let response = client
            .request(&request_with("1", "import_har", json!({"path": path})))
            .await;

        assert_eq!(response["ok"], false, "{path:?} was accepted");
        assert_eq!(response["error"]["code"], "invalid_parameters", "{path:?}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_file_that_is_not_a_har_document_is_refused_without_quoting_it() {
    let server = TestServer::start().await;
    let path = path_in(&server, "notes.har");
    fs::write(&path, b"Authorization: Bearer sk-secret").expect("writes");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "har_unavailable");
    let message = serde_json::to_string(&response).expect("serializes");
    assert!(!message.contains("sk-secret"), "{message}");
    // Nor the path, which is the one thing out of a client's own request
    // that could have come back.
    assert!(!message.contains("notes.har"), "{message}");

    server.stop().await;
}

#[tokio::test]
async fn a_file_that_is_not_there_is_refused() {
    let server = TestServer::start().await;
    let path = path_in(&server, "missing.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "import_har",
            json!({"path": path.to_string_lossy()}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "har_unavailable");

    server.stop().await;
}

#[tokio::test]
async fn an_export_naming_exchanges_and_filters_at_once_is_refused() {
    let server = TestServer::start().await;
    let id = server.store(&exchange("example.com", "/one", 200));
    let path = path_in(&server, "both.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({
                "path": path.to_string_lossy(),
                "exchange_ids": [id.to_string()],
                "host": "example.com",
            }),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");
    assert!(!path.exists(), "a refused export wrote a file");

    server.stop().await;
}

#[tokio::test]
async fn an_export_naming_no_exchanges_at_all_is_refused() {
    let server = TestServer::start().await;
    let path = path_in(&server, "none.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "exchange_ids": []}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn an_export_naming_more_exchanges_than_the_limit_is_refused() {
    let server = TestServer::start().await;
    let path = path_in(&server, "too-many.har");
    let ids: Vec<String> = (0..501)
        .map(|_| proxy_storage::ExchangeId::next().to_string())
        .collect();
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "exchange_ids": ids}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn an_exchange_id_that_is_not_an_id_is_refused_by_name() {
    let server = TestServer::start().await;
    let path = path_in(&server, "bad-id.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "exchange_ids": ["not-an-id"]}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_exchange_id");

    server.stop().await;
}

#[tokio::test]
async fn a_parameter_the_server_does_not_know_is_refused() {
    let server = TestServer::start().await;
    let path = path_in(&server, "unknown.har");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "pretty": true}),
        ))
        .await;

    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn a_failed_export_leaves_whatever_was_at_the_path_alone() {
    let server = TestServer::start().await;
    let path = path_in(&server, "precious.har");
    fs::write(&path, b"something a person wanted").expect("writes");
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "1",
            "export_har",
            json!({"path": path.to_string_lossy(), "exchange_ids": ["not-an-id"]}),
        ))
        .await;
    assert_eq!(response["ok"], false, "{response}");

    assert_eq!(
        fs::read_to_string(&path).expect("still there"),
        "something a person wanted"
    );

    server.stop().await;
}
