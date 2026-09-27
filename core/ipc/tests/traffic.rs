//! The four things a client may do with the stored traffic.

mod support;

use support::{
    at, exchange, failed_exchange, request, request_with, secretive_exchange, started_at,
    with_method, TestServer,
};

use proxy_storage::{ExchangeId, TrafficErrorKind};
use serde_json::{json, Value};

/// The result of a successful response.
fn result(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "expected success: {response}");

    &response["result"]
}

/// The ids a list response holds, in the order it holds them.
fn listed(response: &Value) -> Vec<String> {
    result(response)["exchanges"]
        .as_array()
        .expect("a list of exchanges")
        .iter()
        .map(|summary| summary["id"].as_str().expect("an id").to_owned())
        .collect()
}

#[tokio::test]
async fn listing_an_empty_history_is_an_empty_page() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "list_traffic")).await;

    assert_eq!(result(&response)["exchanges"], json!([]));
    assert_eq!(result(&response)["total"], 0);
    assert_eq!(result(&response)["next_cursor"], Value::Null);

    server.stop().await;
}

#[tokio::test]
async fn listing_returns_the_newest_traffic_first_with_a_total() {
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..3)
        .map(|index| {
            server.store(&started_at(
                exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            ))
        })
        .collect();
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "list_traffic")).await;

    assert_eq!(
        listed(&response),
        vec![
            stored[2].to_string(),
            stored[1].to_string(),
            stored[0].to_string()
        ]
    );
    assert_eq!(result(&response)["total"], 3);

    server.stop().await;
}

#[tokio::test]
async fn a_listed_exchange_is_a_summary_and_not_a_whole_exchange() {
    let server = TestServer::start().await;
    server.store(&secretive_exchange("example.com"));
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "list_traffic")).await;

    let summary = &result(&response)["exchanges"][0];
    assert_eq!(summary["method"], "GET");
    assert_eq!(summary["url"], "http://example.com/orders");
    assert_eq!(summary["status"], 201);
    assert_eq!(summary["duration_micros"], 3_500);
    // The list is what a traffic list draws, so the captured bodies and
    // headers are not in it — for size, and because nobody asked for them.
    let line = response.to_string();
    for secret in ["sk-secret", "hunter2", "session=", "Authorization"] {
        assert!(!line.contains(secret), "a list carried {secret}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_page_says_where_to_carry_on_from() {
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..5)
        .map(|index| {
            server.store(&started_at(
                exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            ))
        })
        .collect();
    let mut client = server.connect().await;

    let first = client
        .request(&request_with("r1", "list_traffic", json!({"limit": 2})))
        .await;
    assert_eq!(
        listed(&first),
        vec![stored[4].to_string(), stored[3].to_string()]
    );
    assert_eq!(result(&first)["total"], 5, "the total is of everything");

    let cursor = result(&first)["next_cursor"].clone();
    assert_ne!(cursor, Value::Null);
    let second = client
        .request(&request_with(
            "r2",
            "list_traffic",
            json!({"limit": 2, "cursor": cursor}),
        ))
        .await;

    assert_eq!(
        listed(&second),
        vec![stored[2].to_string(), stored[1].to_string()]
    );

    server.stop().await;
}

#[tokio::test]
async fn paging_reaches_the_end_of_the_list() {
    let server = TestServer::start().await;
    for index in 0..5 {
        server.store(&started_at(
            exchange("example.com", &format!("/{index}"), 200),
            at(1_000 + index),
        ));
    }
    let mut client = server.connect().await;

    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut parameters = json!({"limit": 2});
        if cursor != Value::Null {
            parameters["cursor"] = cursor.clone();
        }
        let page = client
            .request(&request_with("r1", "list_traffic", parameters))
            .await;

        seen.extend(listed(&page));
        cursor = result(&page)["next_cursor"].clone();
        if cursor == Value::Null {
            break;
        }
    }

    assert_eq!(seen.len(), 5);
    assert_eq!(
        seen.iter().collect::<std::collections::BTreeSet<_>>().len(),
        5,
        "an exchange was listed twice"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_host_filter_narrows_the_list() {
    let server = TestServer::start().await;
    server.store(&exchange("api.example.com", "/one", 200));
    server.store(&exchange("api.example.com", "/two", 200));
    server.store(&exchange("cdn.example.com", "/three", 200));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"host": "api.example.com"}),
        ))
        .await;

    assert_eq!(listed(&response).len(), 2);
    assert_eq!(result(&response)["total"], 2, "the total is of the filter");

    server.stop().await;
}

#[tokio::test]
async fn a_method_filter_narrows_the_list() {
    let server = TestServer::start().await;
    server.store(&with_method(exchange("example.com", "/one", 200), "GET"));
    server.store(&with_method(exchange("example.com", "/two", 200), "POST"));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"method": "POST"}),
        ))
        .await;

    assert_eq!(listed(&response).len(), 1);
    assert_eq!(result(&response)["exchanges"][0]["method"], "POST");

    server.stop().await;
}

#[tokio::test]
async fn a_status_filter_narrows_the_list() {
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/ok", 200));
    server.store(&exchange("example.com", "/missing", 404));
    server.store(&exchange("example.com", "/gone", 410));
    let mut client = server.connect().await;

    let class = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"status": {"class": 4}}),
        ))
        .await;
    let exact = client
        .request(&request_with(
            "r2",
            "list_traffic",
            json!({"status": {"exact": 404}}),
        ))
        .await;

    assert_eq!(listed(&class).len(), 2);
    assert_eq!(listed(&exact).len(), 1);
    assert_eq!(result(&exact)["exchanges"][0]["status"], 404);

    server.stop().await;
}

#[tokio::test]
async fn a_time_filter_narrows_the_list() {
    let server = TestServer::start().await;
    for seconds in [1_000, 2_000, 3_000] {
        server.store(&started_at(exchange("example.com", "/x", 200), at(seconds)));
    }
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({
                "started_at_or_after": {"seconds": 2_000, "nanos": 0},
                "started_before": {"seconds": 3_000, "nanos": 0},
            }),
        ))
        .await;

    assert_eq!(listed(&response).len(), 1);

    server.stop().await;
}

#[tokio::test]
async fn filters_narrow_each_other() {
    let server = TestServer::start().await;
    server.store(&with_method(
        exchange("api.example.com", "/wanted", 404),
        "GET",
    ));
    server.store(&with_method(exchange("api.example.com", "/ok", 200), "GET"));
    server.store(&with_method(
        exchange("cdn.example.com", "/missing", 404),
        "GET",
    ));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"host": "api.example.com", "method": "GET", "status": {"class": 4}}),
        ))
        .await;

    assert_eq!(listed(&response).len(), 1);
    assert_eq!(
        result(&response)["exchanges"][0]["url"],
        "http://api.example.com/wanted"
    );

    server.stop().await;
}

#[tokio::test]
async fn getting_an_exchange_returns_everything_that_was_kept_of_it() {
    let server = TestServer::start().await;
    let stored = secretive_exchange("example.com");
    let id = server.store(&stored);
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "get_traffic",
            json!({"exchange_id": id.to_string()}),
        ))
        .await;

    let exchange = &result(&response)["exchange"];
    assert_eq!(exchange["id"], id.to_string());
    assert_eq!(exchange["request"]["method"], "GET");
    assert_eq!(exchange["response"]["status"], 201);
    assert_eq!(exchange["timing"]["completed_micros"], 3_500);
    // A client that asks for one exchange is asking for what was captured,
    // credentials included: that is what a debugging proxy is for.
    let line = response.to_string();
    assert!(line.contains("Authorization"), "the headers were dropped");
    assert!(
        line.contains("Bearer sk-secret"),
        "the header value was dropped"
    );
    // The bodies come back as the model spells them, which is base64.
    assert!(
        !exchange["request"]["body"]["data"]
            .as_str()
            .expect("a body")
            .is_empty(),
        "the request body was dropped"
    );

    server.stop().await;
}

#[tokio::test]
async fn getting_an_exchange_that_failed_returns_the_failure() {
    let server = TestServer::start().await;
    let id = server.store(&failed_exchange(
        "example.com",
        TrafficErrorKind::UpstreamConnect,
    ));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "get_traffic",
            json!({"exchange_id": id.to_string()}),
        ))
        .await;

    let exchange = &result(&response)["exchange"];
    assert_eq!(exchange["response"], Value::Null);
    assert_eq!(exchange["error"]["kind"], "upstream_connect");

    server.stop().await;
}

#[tokio::test]
async fn getting_an_exchange_that_is_not_stored_says_so() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "get_traffic",
            json!({"exchange_id": ExchangeId::next().to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "not_found");

    server.stop().await;
}

#[tokio::test]
async fn deleting_an_exchange_removes_it() {
    let server = TestServer::start().await;
    let id = server.store(&exchange("example.com", "/x", 200));
    let mut client = server.connect().await;

    let deleted = client
        .request(&request_with(
            "r1",
            "delete_traffic",
            json!({"exchange_id": id.to_string()}),
        ))
        .await;

    assert_eq!(result(&deleted)["deleted"], true);
    assert_eq!(result(&deleted)["exchange_id"], id.to_string());

    // Gone from the list, and gone from the database.
    let listed = client.request(&request("r2", "list_traffic")).await;
    assert_eq!(result(&listed)["total"], 0);
    assert_eq!(server.storage.count().expect("counts"), 0);

    server.stop().await;
}

#[tokio::test]
async fn deleting_an_exchange_that_is_not_stored_is_not_a_failure() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "delete_traffic",
            json!({"exchange_id": ExchangeId::next().to_string()}),
        ))
        .await;

    assert_eq!(response["ok"], true);
    assert_eq!(result(&response)["deleted"], false);

    server.stop().await;
}

#[tokio::test]
async fn clearing_removes_all_the_traffic_and_says_how_much() {
    let server = TestServer::start().await;
    for index in 0..4 {
        server.store(&exchange("example.com", &format!("/{index}"), 200));
    }
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "clear_traffic")).await;

    assert_eq!(result(&response)["removed"], 4);
    assert_eq!(server.storage.count().expect("counts"), 0);
    let listed = client.request(&request("r2", "list_traffic")).await;
    assert_eq!(result(&listed)["total"], 0);

    server.stop().await;
}

#[tokio::test]
async fn the_history_is_still_usable_after_it_has_been_cleared() {
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/before", 200));
    let mut client = server.connect().await;
    client.request(&request("r1", "clear_traffic")).await;

    let after = server.store(&exchange("example.com", "/after", 200));

    let listed = client.request(&request("r2", "list_traffic")).await;
    assert_eq!(listed["result"]["exchanges"][0]["id"], after.to_string());

    server.stop().await;
}

// Searching and reading the order from the other end, over the wire. Both are
// parameters of `list_traffic` and neither changes what a response is.

#[tokio::test]
async fn a_search_narrows_the_list_to_the_rows_holding_it() {
    let server = TestServer::start().await;
    let wanted = server.store(&exchange("api.example.com", "/v1/login", 200));
    server.store(&exchange("api.example.com", "/v1/orders", 200));
    server.store(&exchange("cdn.example.com", "/logo.png", 200));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"search": "login"}),
        ))
        .await;

    assert_eq!(listed(&response), vec![wanted.to_string()]);
    // The total is the total of what matched, not of everything stored.
    assert_eq!(result(&response)["total"], 1);

    server.stop().await;
}

#[tokio::test]
async fn a_search_matches_a_host_a_method_or_a_status() {
    let server = TestServer::start().await;
    server.store(&with_method(
        exchange("api.example.com", "/v1/orders", 201),
        "POST",
    ));
    server.store(&exchange("cdn.example.com", "/logo.png", 404));
    let mut client = server.connect().await;

    for (search, expected) in [
        ("api.example", 1),
        ("POST", 1),
        ("201", 1),
        ("404", 1),
        ("example.com", 2),
        ("nowhere", 0),
    ] {
        let response = client
            .request(&request_with(
                "r1",
                "list_traffic",
                json!({"search": search}),
            ))
            .await;

        assert_eq!(listed(&response).len(), expected, "searching {search}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_search_combines_with_the_other_filters() {
    let server = TestServer::start().await;
    let wanted = server.store(&with_method(
        exchange("api.example.com", "/v1/users", 201),
        "POST",
    ));
    server.store(&exchange("api.example.com", "/v1/users", 200));
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"search": "users", "method": "POST", "status": {"class": 2}}),
        ))
        .await;

    assert_eq!(listed(&response), vec![wanted.to_string()]);

    server.stop().await;
}

#[tokio::test]
async fn a_search_that_is_not_a_search_is_refused_without_being_repeated() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"search": "needle\nneedle"}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_parameters");
    assert!(
        !response["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("needle"),
        "{response}"
    );

    server.stop().await;
}

#[tokio::test]
async fn listing_can_be_read_oldest_first() {
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..3)
        .map(|index| {
            server.store(&started_at(
                exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            ))
        })
        .collect();
    let mut client = server.connect().await;

    let oldest = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"order": "oldest"}),
        ))
        .await;
    let newest = client
        .request(&request_with(
            "r2",
            "list_traffic",
            json!({"order": "newest"}),
        ))
        .await;

    assert_eq!(
        listed(&oldest),
        stored.iter().map(ExchangeId::to_string).collect::<Vec<_>>()
    );

    let mut reversed = listed(&oldest);
    reversed.reverse();
    assert_eq!(listed(&newest), reversed);

    server.stop().await;
}

#[tokio::test]
async fn a_list_that_names_no_order_is_the_newest_first() {
    // What every client written before the parameter existed sends, and it
    // still means what it always meant.
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..2)
        .map(|index| {
            server.store(&started_at(
                exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            ))
        })
        .collect();
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "list_traffic")).await;

    assert_eq!(
        listed(&response),
        vec![stored[1].to_string(), stored[0].to_string()]
    );

    server.stop().await;
}

#[tokio::test]
async fn paging_oldest_first_walks_the_whole_history_once() {
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..7)
        .map(|index| {
            server.store(&started_at(
                exchange("example.com", &format!("/{index}"), 200),
                at(1_000 + index),
            ))
        })
        .collect();
    let mut client = server.connect().await;

    let mut seen: Vec<String> = Vec::new();
    let mut cursor = Value::Null;

    loop {
        let mut parameters = json!({"limit": 3, "order": "oldest"});
        if !cursor.is_null() {
            parameters["cursor"] = cursor.clone();
        }

        let response = client
            .request(&request_with("r1", "list_traffic", parameters))
            .await;
        seen.extend(listed(&response));

        cursor = result(&response)["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }

    assert_eq!(
        seen,
        stored.iter().map(ExchangeId::to_string).collect::<Vec<_>>()
    );

    server.stop().await;
}

#[tokio::test]
async fn an_order_the_server_does_not_have_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "list_traffic",
            json!({"order": "duration"}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn a_search_matches_a_header_or_a_body() {
    // The search reaches captured headers and bodies. A list page is still a
    // summary: the match is decided in the store, and the bytes stay there
    // until `get_traffic`.
    let server = TestServer::start().await;
    let hit = server.store(&secretive_exchange("api.example.com"));
    server.store(&exchange("cdn.example.com", "/x", 200));
    let mut client = server.connect().await;

    for search in ["sk-secret", "hunter2", "Authorization"] {
        let response = client
            .request(&request_with(
                "r1",
                "list_traffic",
                json!({"search": search}),
            ))
            .await;

        assert_eq!(
            listed(&response),
            vec![hit.to_string()],
            "searching {search}"
        );
        let row = &response["result"]["exchanges"][0];
        assert!(row.get("headers").is_none(), "searching {search}");
        assert!(row.get("body").is_none(), "searching {search}");
    }

    server.stop().await;
}
