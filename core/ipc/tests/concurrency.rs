//! Many clients at once, doing different things.
//!
//! Nothing here waits for a length of time: every client is driven to
//! completion and joined, and the assertions are made afterwards.

mod support;

use support::{exchange, request, request_with, Client, TestServer};

use proxy_storage::ExchangeId;
use serde_json::json;

/// How many clients, and how many requests each of them sends.
const CLIENTS: usize = 8;
const EACH: usize = 16;

#[tokio::test(flavor = "multi_thread")]
async fn many_clients_asking_at_once_are_all_answered_correctly() {
    let server = TestServer::start().await;
    for index in 0..32 {
        server.store(&exchange("example.com", &format!("/{index}"), 200));
    }
    let socket = server.socket().to_path_buf();

    let clients: Vec<_> = (0..CLIENTS)
        .map(|client| {
            let socket = socket.clone();
            tokio::spawn(async move {
                let mut connection = Client::connect(&socket).await;

                for number in 0..EACH {
                    let id = format!("client-{client}-request-{number}");
                    let response = connection.request(&request(&id, "list_traffic")).await;

                    // Every answer goes to the client that asked, labelled
                    // with the request it is answering.
                    assert_eq!(response["request_id"], id, "{response}");
                    assert_eq!(response["ok"], true, "{response}");
                    assert_eq!(response["result"]["total"], 32, "{response}");
                }
            })
        })
        .collect();

    for client in clients {
        client.await.expect("a client task does not panic");
    }

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_and_deleting_at_once_leaves_the_history_consistent() {
    let server = TestServer::start().await;
    let stored: Vec<ExchangeId> = (0..64)
        .map(|index| server.store(&exchange("example.com", &format!("/{index}"), 200)))
        .collect();
    let socket = server.socket().to_path_buf();
    let (removed, kept) = stored.split_at(32);

    let deleting = {
        let socket = socket.clone();
        let removed = removed.to_vec();
        tokio::spawn(async move {
            let mut client = Client::connect(&socket).await;

            for id in removed {
                let response = client
                    .request(&request_with(
                        "delete",
                        "delete_traffic",
                        json!({"exchange_id": id.to_string()}),
                    ))
                    .await;

                assert_eq!(response["result"]["deleted"], true, "{response}");
            }
        })
    };
    let reading = {
        let socket = socket.clone();
        let kept = kept.to_vec();
        tokio::spawn(async move {
            let mut client = Client::connect(&socket).await;

            for id in kept {
                let response = client
                    .request(&request_with(
                        "get",
                        "get_traffic",
                        json!({"exchange_id": id.to_string()}),
                    ))
                    .await;

                // An exchange nobody is deleting is still there, whole.
                assert_eq!(response["ok"], true, "{response}");
                assert_eq!(response["result"]["exchange"]["id"], id.to_string());
            }
        })
    };
    let listing = tokio::spawn(async move {
        let mut client = Client::connect(&socket).await;

        for _ in 0..EACH {
            let response = client.request(&request("list", "list_traffic")).await;

            assert_eq!(response["ok"], true, "{response}");
        }
    });

    for task in [deleting, reading, listing] {
        task.await.expect("a client task does not panic");
    }

    assert_eq!(server.storage.count().expect("counts"), 32);

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_come_and_go_while_others_keep_working() {
    let server = TestServer::start().await;
    server.store(&exchange("example.com", "/x", 200));
    let mut steady = server.connect().await;

    for round in 0..16 {
        // A client that connects, asks one thing, and leaves, over and over,
        // while another connection stays up throughout.
        let mut passing = server.connect().await;
        let response = passing
            .request(&request(&format!("passing-{round}"), "list_traffic"))
            .await;
        assert_eq!(response["ok"], true);
        passing.disconnect().await;

        let response = steady
            .request(&request(&format!("steady-{round}"), "list_traffic"))
            .await;
        assert_eq!(response["request_id"], format!("steady-{round}"));
        assert_eq!(response["result"]["total"], 1);
    }

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribers_and_askers_do_not_get_in_each_others_way() {
    let server = TestServer::start().await;
    let mut watcher = server.connect().await;
    let mut asker = server.connect().await;
    assert_eq!(
        watcher.request(&request("r1", "subscribe")).await["ok"],
        true
    );

    // Traffic arriving while another client is asking questions: the asker's
    // answers are answers, and the watcher's events are events.
    for index in 0..8 {
        let exchange = exchange("example.com", &format!("/{index}"), 200);
        let id = exchange.id;
        server.store(&exchange);
        server.publish(exchange);

        let response = asker
            .request(&request(&format!("r{index}"), "list_traffic"))
            .await;
        assert_eq!(response["ok"], true, "{response}");

        let event = watcher.next_message().await;
        assert!(event.get("ok").is_none(), "{event}");
        assert_eq!(event["exchange"]["id"], id.to_string());
    }

    server.stop().await;
}
