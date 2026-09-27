//! Clients watching traffic as it happens.
//!
//! Nothing here sleeps. A subscription is confirmed by its response before
//! anything is published, so a test that has the response knows the server is
//! listening; and every publish says how many subscribers received it, so a
//! test can assert that the server was one of them before it reads.

mod support;

use support::{exchange, request, secretive_exchange, TestServer, TEST_TIMEOUT};

use proxy_storage::DEFAULT_EVENT_CAPACITY;
use serde_json::Value;

/// Subscribes `client` and waits for the confirmation.
///
/// On return the server has a subscription, so anything published after this
/// reaches this client.
async fn subscribe(client: &mut support::Client, id: &str) {
    let response = client.request(&request(id, "subscribe")).await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["type"], "subscribe");
    assert_eq!(response["result"]["subscribed"], true);
}

/// Reads the next message, which must be an event.
async fn next_event(client: &mut support::Client) -> Value {
    let message = client.next_message().await;

    assert!(
        message.get("ok").is_none(),
        "expected an event, got a response: {message}"
    );
    assert_eq!(message["version"], 1);

    message
}

#[tokio::test]
async fn a_subscriber_is_told_about_new_traffic() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;

    let exchange = exchange("example.com", "/hello", 200);
    let id = exchange.id;
    assert_eq!(server.publish(exchange), 1, "the server was not subscribed");

    let event = next_event(&mut client).await;

    assert_eq!(event["type"], "traffic_added");
    assert_eq!(event["exchange"]["id"], id.to_string());

    server.stop().await;
}

#[tokio::test]
async fn an_event_carries_the_summary_a_traffic_list_shows() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;
    server.publish(exchange("api.example.com", "/users", 201));

    let event = next_event(&mut client).await;

    let summary = &event["exchange"];
    assert_eq!(summary["method"], "GET");
    assert_eq!(
        summary["url"],
        "https://api.example.com/users".replace("https", "http")
    );
    assert_eq!(summary["status"], 201);
    assert_eq!(summary["scheme"], "http");
    assert_eq!(summary["destination"]["host"], "api.example.com");
    assert_eq!(summary["completed"], true);
    assert!(summary["started_at"]["seconds"].is_number());

    server.stop().await;
}

#[tokio::test]
async fn an_event_does_not_carry_headers_or_bodies() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;
    server.publish(secretive_exchange("example.com"));

    let event = next_event(&mut client).await;
    let line = event.to_string();

    for secret in [
        "sk-secret",
        "hunter2",
        "session=",
        "Authorization",
        "Cookie",
    ] {
        assert!(!line.contains(secret), "an event carried {secret}: {line}");
    }
    assert!(event["exchange"].get("request").is_none(), "{line}");
    assert!(event["exchange"].get("response").is_none(), "{line}");

    server.stop().await;
}

#[tokio::test]
async fn an_event_says_which_exchange_to_ask_about() {
    let server = TestServer::start().await;
    let stored = secretive_exchange("example.com");
    let id = server.store(&stored);
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;
    server.publish(stored);

    // The whole point of an event: the id is enough to go and get the rest.
    let event = next_event(&mut client).await;
    assert_eq!(event["exchange"]["id"], id.to_string());

    let response = client
        .request(&support::request_with(
            "r2",
            "get_traffic",
            serde_json::json!({"exchange_id": event["exchange"]["id"]}),
        ))
        .await;

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["exchange"]["id"], id.to_string());

    server.stop().await;
}

#[tokio::test]
async fn events_arrive_in_the_order_the_traffic_happened() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;

    let mut ids = Vec::new();
    for index in 0..5 {
        let exchange = exchange("example.com", &format!("/{index}"), 200);
        ids.push(exchange.id);
        server.publish(exchange);
    }

    for id in ids {
        assert_eq!(
            next_event(&mut client).await["exchange"]["id"],
            id.to_string()
        );
    }

    server.stop().await;
}

#[tokio::test]
async fn every_subscriber_is_told_about_the_same_traffic() {
    let server = TestServer::start().await;
    let mut first = server.connect().await;
    let mut second = server.connect().await;
    let mut third = server.connect().await;
    subscribe(&mut first, "r1").await;
    subscribe(&mut second, "r2").await;
    subscribe(&mut third, "r3").await;

    let exchange = exchange("example.com", "/hello", 200);
    let id = exchange.id;
    assert_eq!(
        server.publish(exchange),
        3,
        "not every client was subscribed"
    );

    for client in [&mut first, &mut second, &mut third] {
        assert_eq!(next_event(client).await["exchange"]["id"], id.to_string());
    }

    server.stop().await;
}

#[tokio::test]
async fn a_client_that_has_not_subscribed_is_told_nothing() {
    let server = TestServer::start().await;
    let mut watching = server.connect().await;
    let mut quiet = server.connect().await;
    subscribe(&mut watching, "r1").await;

    server.publish(exchange("example.com", "/hello", 200));
    next_event(&mut watching).await;

    // The quiet client's next message is the answer to its own request, not
    // an event it never asked for.
    let response = quiet.request(&request("r2", "list_traffic")).await;
    assert_eq!(response["request_id"], "r2");

    server.stop().await;
}

#[tokio::test]
async fn subscribing_twice_on_one_connection_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;

    let response = client.request(&request("r2", "subscribe")).await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "already_subscribed");

    // The subscription it already had is untouched.
    server.publish(exchange("example.com", "/hello", 200));
    assert_eq!(next_event(&mut client).await["type"], "traffic_added");

    server.stop().await;
}

#[tokio::test]
async fn a_subscriber_can_still_ask_questions() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;

    let response = client.request(&request("r2", "list_traffic")).await;
    assert_eq!(response["ok"], true);
    assert_eq!(response["request_id"], "r2");

    // And is still subscribed afterwards.
    server.publish(exchange("example.com", "/hello", 200));
    assert_eq!(next_event(&mut client).await["type"], "traffic_added");

    server.stop().await;
}

#[tokio::test]
async fn a_subscriber_that_disconnects_does_not_disturb_the_others() {
    let server = TestServer::start().await;
    let mut leaving = server.connect().await;
    let mut staying = server.connect().await;
    subscribe(&mut leaving, "r1").await;
    subscribe(&mut staying, "r2").await;

    leaving.disconnect().await;

    // Publishing keeps working whether the gone client's subscription has
    // been noticed yet or not, and the client that is still there keeps
    // receiving.
    let exchange = exchange("example.com", "/after", 200);
    let id = exchange.id;
    server.publish(exchange);

    assert_eq!(
        next_event(&mut staying).await["exchange"]["id"],
        id.to_string()
    );

    server.stop().await;
}

#[tokio::test]
async fn a_subscriber_that_never_reads_costs_the_others_nothing() {
    let server = TestServer::start().await;
    let mut silent = server.connect().await;
    let mut reading = server.connect().await;
    subscribe(&mut silent, "r1").await;
    subscribe(&mut reading, "r2").await;

    // A handful of exchanges — comfortably fewer than the stream holds —
    // while one subscriber reads nothing at all. The other must receive
    // every one of them, in order and with nothing missing.
    let mut ids = Vec::new();
    for index in 0..3 {
        let exchange = exchange("example.com", &format!("/{index}"), 200);
        ids.push(exchange.id);
        assert_eq!(server.publish(exchange), 2, "both clients are subscribed");
    }

    for id in ids {
        let event = next_event(&mut reading).await;

        assert_eq!(event["type"], "traffic_added", "{event}");
        assert_eq!(event["exchange"]["id"], id.to_string());
    }

    server.stop().await;
}

#[tokio::test]
async fn publishing_never_waits_for_a_subscriber_that_is_not_reading() {
    let server = TestServer::start().await;
    let mut silent = server.connect().await;
    subscribe(&mut silent, "r1").await;

    // Far more than the event stream holds, published to a subscriber that
    // reads nothing at all. If a client that has stopped reading could hold
    // the publisher up — and so the proxy, which publishes as it finishes a
    // response — this would not finish.
    let published = tokio::time::timeout(TEST_TIMEOUT, async {
        for index in 0..(DEFAULT_EVENT_CAPACITY * 8) {
            server.publish(exchange("example.com", &format!("/{index}"), 200));
        }
    })
    .await;

    assert!(published.is_ok(), "a silent client held up the publisher");

    // And the server stops without waiting for that client either.
    server.stop().await;
}

#[tokio::test]
async fn a_subscriber_that_fell_behind_is_told_how_much_it_missed() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    subscribe(&mut client, "r1").await;

    // More than the stream holds for one subscriber, published before it
    // reads anything: the oldest are dropped rather than queued for it.
    let published = DEFAULT_EVENT_CAPACITY * 4;
    for index in 0..published {
        server.publish(exchange("example.com", &format!("/{index}"), 200));
    }

    // Somewhere in what it reads is the message saying it fell behind, which
    // is its cue to ask for the list again.
    let mut lost = None;
    for _ in 0..DEFAULT_EVENT_CAPACITY {
        let event = next_event(&mut client).await;
        if event["type"] == "traffic_lost" {
            lost = Some(event);
            break;
        }
    }

    let lost = lost.expect("the subscriber was never told it fell behind");
    let missed = lost["missed"].as_u64().expect("a count");
    assert!(missed > 0, "{lost}");
    assert!(
        missed < published as u64,
        "it was told it missed more than was published: {lost}"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_subscription_ends_with_the_connection_that_asked_for_it() {
    let server = TestServer::start().await;
    let mut first = server.connect().await;
    subscribe(&mut first, "r1").await;
    assert_eq!(server.publisher.subscribers(), 1);
    first.disconnect().await;

    // The next client's subscription is its own: a connection that has gone
    // does not leave one behind.
    let mut second = server.connect().await;
    subscribe(&mut second, "r2").await;
    let exchange = exchange("example.com", "/after", 200);
    let id = exchange.id;
    server.publish(exchange);

    assert_eq!(
        next_event(&mut second).await["exchange"]["id"],
        id.to_string()
    );

    server.stop().await;
}
