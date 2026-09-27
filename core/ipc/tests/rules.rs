//! Managing rules over the socket.
//!
//! The operations a client uses to write the rules a proxy applies, and the
//! refusals that keep a rule that could not work from ever being stored. Every
//! test speaks the protocol by hand, because what is under test is the bytes
//! on the wire.

mod support;

use support::{request, request_with, TestServer};

use serde_json::{json, Value};

/// The result of a successful response.
fn result(response: &Value) -> &Value {
    assert_eq!(response["ok"], true, "expected success: {response}");

    &response["result"]
}

/// The rule a response carries.
fn rule(response: &Value) -> &Value {
    &result(response)["rule"]
}

/// The ids a rule list holds, in the order it holds them.
fn listed(response: &Value) -> Vec<String> {
    result(response)["rules"]
        .as_array()
        .expect("a list of rules")
        .iter()
        .map(|rule| rule["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// The parameters of a rule that blocks one host.
fn blocking_rule(name: &str, host: &str, priority: i32) -> Value {
    json!({
        "name": name,
        "priority": priority,
        "matcher": {"host": {"exact": host}},
        "action": {"type": "block"},
    })
}

#[tokio::test]
async fn listing_rules_when_there_are_none_is_an_empty_list() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client.request(&request("r1", "list_rules")).await;

    assert_eq!(response["type"], "list_rules");
    assert_eq!(result(&response)["rules"], json!([]));

    server.stop().await;
}

#[tokio::test]
async fn a_created_rule_is_given_back_with_the_id_the_server_minted() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("block the api", "api.example.com", 10),
        ))
        .await;

    assert_eq!(response["type"], "create_rule");
    let created = rule(&response);
    assert!(created["id"].as_str().is_some_and(|id| id.len() == 36));
    assert_eq!(created["name"], "block the api");
    assert_eq!(created["priority"], 10);
    assert_eq!(created["enabled"], true);
    assert_eq!(created["action"]["type"], "block");
    assert_eq!(created["matcher"]["host"]["exact"], "api.example.com");
    assert!(created["created_at_ms"].as_u64().is_some());

    server.stop().await;
}

#[tokio::test]
async fn a_created_rule_reaches_the_engine_the_proxy_is_using() {
    // The point of the whole operation: a rule that has been created is one
    // the next request is evaluated against, without the proxy asking the
    // database anything.
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    assert_eq!(server.rules.snapshot().len(), 0);

    client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("block", "api.example.com", 0),
        ))
        .await;

    let rules = server.rules.snapshot();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules.enabled_count(), 1);
    assert!(!rules
        .evaluate(&rules::RequestContext::new(
            "api.example.com",
            "/users",
            "GET"
        ))
        .is_allow());

    server.stop().await;
}

#[tokio::test]
async fn a_created_rule_is_read_back_by_its_id() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("block", "api.example.com", 0),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    let response = client
        .request(&request_with("r2", "get_rule", json!({"rule_id": id})))
        .await;

    assert_eq!(response["type"], "get_rule");
    assert_eq!(rule(&response), rule(&created));

    server.stop().await;
}

#[tokio::test]
async fn rules_are_listed_in_the_order_they_are_consulted_in() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let late = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("late", "a.example.com", 20),
        ))
        .await;
    let early = client
        .request(&request_with(
            "r2",
            "create_rule",
            blocking_rule("early", "b.example.com", -5),
        ))
        .await;
    let middle = client
        .request(&request_with(
            "r3",
            "create_rule",
            blocking_rule("middle", "c.example.com", 0),
        ))
        .await;

    let response = client.request(&request("r4", "list_rules")).await;

    assert_eq!(
        listed(&response),
        vec![
            rule(&early)["id"].as_str().expect("an id").to_owned(),
            rule(&middle)["id"].as_str().expect("an id").to_owned(),
            rule(&late)["id"].as_str().expect("an id").to_owned(),
        ]
    );

    server.stop().await;
}

#[tokio::test]
async fn an_update_changes_what_it_names_and_nothing_else() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("first", "api.example.com", 10),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    let response = client
        .request(&request_with(
            "r2",
            "update_rule",
            json!({"rule_id": id, "name": "renamed", "priority": 3}),
        ))
        .await;

    assert_eq!(response["type"], "update_rule");
    let updated = rule(&response);
    assert_eq!(updated["name"], "renamed");
    assert_eq!(updated["priority"], 3);
    assert_eq!(updated["matcher"], rule(&created)["matcher"]);
    assert_eq!(updated["action"], rule(&created)["action"]);
    assert_eq!(updated["created_at_ms"], rule(&created)["created_at_ms"]);

    server.stop().await;
}

#[tokio::test]
async fn a_rule_can_be_turned_off_and_on_again() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("rule", "api.example.com", 0),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    let disabled = client
        .request(&request_with(
            "r2",
            "update_rule",
            json!({"rule_id": id, "enabled": false}),
        ))
        .await;

    assert_eq!(rule(&disabled)["enabled"], false);
    // And the engine stopped consulting it, which is the whole point.
    assert_eq!(server.rules.snapshot().enabled_count(), 0);
    assert!(server
        .rules
        .snapshot()
        .evaluate(&rules::RequestContext::new("api.example.com", "/x", "GET"))
        .is_allow());

    let enabled = client
        .request(&request_with(
            "r3",
            "update_rule",
            json!({"rule_id": id, "enabled": true}),
        ))
        .await;

    assert_eq!(rule(&enabled)["enabled"], true);
    assert_eq!(server.rules.snapshot().enabled_count(), 1);

    server.stop().await;
}

#[tokio::test]
async fn a_deleted_rule_is_gone_from_the_list_and_from_the_engine() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("rule", "api.example.com", 0),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    let response = client
        .request(&request_with("r2", "delete_rule", json!({"rule_id": id})))
        .await;

    assert_eq!(response["type"], "delete_rule");
    assert_eq!(result(&response)["deleted"], true);
    assert_eq!(result(&response)["rule_id"], id);
    assert_eq!(
        listed(&client.request(&request("r3", "list_rules")).await).len(),
        0
    );
    assert_eq!(server.rules.snapshot().len(), 0);

    server.stop().await;
}

#[tokio::test]
async fn deleting_a_rule_that_is_not_there_says_so_rather_than_failing() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "delete_rule",
            json!({"rule_id": "0199c0de-0000-7000-8000-00000000002a"}),
        ))
        .await;

    assert_eq!(result(&response)["deleted"], false);

    server.stop().await;
}

#[tokio::test]
async fn asking_for_a_rule_that_is_not_there_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "get_rule",
            json!({"rule_id": "0199c0de-0000-7000-8000-00000000002a"}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "not_found");

    server.stop().await;
}

#[tokio::test]
async fn updating_a_rule_that_is_not_there_is_not_found() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "update_rule",
            json!({"rule_id": "0199c0de-0000-7000-8000-00000000002a", "enabled": false}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "not_found");

    server.stop().await;
}

#[tokio::test]
async fn a_rule_id_that_is_not_an_id_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for operation in ["get_rule", "delete_rule"] {
        let response = client
            .request(&request_with(
                "r1",
                operation,
                json!({"rule_id": "not-a-uuid"}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{operation}");
        assert_eq!(response["error"]["code"], "invalid_rule_id", "{operation}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_rule_with_no_name_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({"name": "  ", "action": {"type": "allow"}}),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_rule");

    server.stop().await;
}

#[tokio::test]
async fn a_matcher_that_could_never_match_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let cases = [
        json!({"host": {"exact": ""}}),
        json!({"path": {"exact": "no-leading-slash"}}),
        json!({"method": {"exact": "GET POST"}}),
    ];

    for matcher in cases {
        let response = client
            .request(&request_with(
                "r1",
                "create_rule",
                json!({"name": "rule", "matcher": matcher, "action": {"type": "allow"}}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{matcher}");
        assert_eq!(response["error"]["code"], "invalid_rule", "{matcher}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_matcher_this_server_cannot_read_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let cases = [
        json!({"host": {"regex": "api\\..*"}}),
        json!({"headers": {"exact": "x"}}),
        json!("api.example.com"),
    ];

    for matcher in cases {
        let response = client
            .request(&request_with(
                "r1",
                "create_rule",
                json!({"name": "rule", "matcher": matcher, "action": {"type": "allow"}}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{matcher}");
        assert_eq!(response["error"]["code"], "invalid_parameters", "{matcher}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_map_remote_destination_that_is_not_one_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let cases = [
        "not-a-url",
        "file:///etc/passwd",
        "ftp://example.com",
        "https://",
        "https://user:secret@example.com",
        "https://example.com:0",
    ];

    for url in cases {
        let response = client
            .request(&request_with(
                "r1",
                "create_rule",
                json!({
                    "name": "map",
                    "action": {"type": "map_remote", "target": remote_target(url)},
                }),
            ))
            .await;

        assert_eq!(response["ok"], false, "{url}");
    }

    server.stop().await;
}

/// A map-remote target as the wire carries it: already parsed into parts.
///
/// A client that has only a URL parses it with the rule model; these are
/// deliberately wrong, so they are built by hand to reach the server.
fn remote_target(url: &str) -> Value {
    match url {
        "not-a-url" => json!({"scheme": "http", "host": ""}),
        "file:///etc/passwd" => json!({"scheme": "file", "host": "etc"}),
        "ftp://example.com" => json!({"scheme": "ftp", "host": "example.com"}),
        "https://" => json!({"scheme": "https", "host": ""}),
        "https://user:secret@example.com" => {
            json!({"scheme": "https", "host": "user:secret@example.com"})
        }
        "https://example.com:0" => json!({"scheme": "https", "host": "example.com", "port": 0}),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_map_local_path_that_is_not_absolute_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({
                "name": "map",
                "action": {"type": "map_local", "path": "relative/fixture.json"},
            }),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_rule");

    server.stop().await;
}

#[tokio::test]
async fn a_block_status_that_is_not_a_status_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for status in [0, 99, 600, 1000] {
        let response = client
            .request(&request_with(
                "r1",
                "create_rule",
                json!({"name": "block", "action": {"type": "block", "status": status}}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{status}");
        assert_eq!(response["error"]["code"], "invalid_rule", "{status}");
    }

    server.stop().await;
}

#[tokio::test]
async fn a_priority_that_is_not_a_priority_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for priority in [json!(4294967296i64), json!("high"), json!(1.5)] {
        let response = client
            .request(&request_with(
                "r1",
                "create_rule",
                json!({"name": "rule", "priority": priority, "action": {"type": "allow"}}),
            ))
            .await;

        assert_eq!(response["ok"], false, "{priority}");
        assert_eq!(
            response["error"]["code"], "invalid_parameters",
            "{priority}"
        );
    }

    server.stop().await;
}

#[tokio::test]
async fn a_rule_operation_that_is_not_shaped_like_one_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let cases = [
        (request("r1", "get_rule"), "invalid_parameters"),
        (request("r1", "create_rule"), "invalid_parameters"),
        (request("r1", "update_rule"), "invalid_parameters"),
        (
            request_with("r1", "list_rules", json!({"enabled": true})),
            "invalid_parameters",
        ),
        (
            request_with("r1", "create_rule", json!({"name": "r"})),
            "invalid_parameters",
        ),
        (
            request_with(
                "r1",
                "create_rule",
                json!({"name": "r", "action": {"type": "allow"}, "unknown": 1}),
            ),
            "invalid_parameters",
        ),
    ];

    for (request, code) in cases {
        let response = client.request(&request).await;

        assert_eq!(response["ok"], false, "{request}");
        assert_eq!(response["error"]["code"], code, "{request}");
    }

    server.stop().await;
}

#[tokio::test]
async fn an_update_that_changes_nothing_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("rule", "api.example.com", 0),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    let response = client
        .request(&request_with("r2", "update_rule", json!({"rule_id": id})))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_parameters");

    server.stop().await;
}

#[tokio::test]
async fn a_refusal_never_repeats_what_the_client_sent() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({
                "name": "needle",
                "matcher": {"host": {"exact": "needle.example.com"}},
                "action": {"type": "map_local", "path": "needle/relative"},
            }),
        ))
        .await;

    assert_eq!(response["ok"], false);
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
async fn rules_and_traffic_are_answered_on_the_same_connection() {
    // The rule operations were added beside the traffic ones, not in place of
    // them: one client, one connection, both kinds of request.
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed_traffic = client.request(&request("r1", "list_traffic")).await;
    assert_eq!(listed_traffic["ok"], true);

    let created = client
        .request(&request_with(
            "r2",
            "create_rule",
            blocking_rule("rule", "api.example.com", 0),
        ))
        .await;
    assert_eq!(created["ok"], true);

    let listed_again = client.request(&request("r3", "list_traffic")).await;
    assert_eq!(listed_again["ok"], true);
    assert_eq!(listed_again["request_id"], "r3");

    server.stop().await;
}

#[tokio::test]
async fn a_rule_survives_the_server_being_restarted() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let created = client
        .request(&request_with(
            "r1",
            "create_rule",
            blocking_rule("rule", "api.example.com", 7),
        ))
        .await;
    let id = rule(&created)["id"].as_str().expect("an id").to_owned();

    // The rules a fresh engine loads from the same database are the ones the
    // last one was applying.
    let reloaded = proxy_ipc::load_rules(&server.storage).expect("the rules load");

    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded.rules()[0].id.to_string(), id);
    assert_eq!(reloaded.rules()[0].priority, 7);

    server.stop().await;
}

#[tokio::test]
async fn a_modify_rule_is_created_and_given_back_as_it_was_written() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let action = json!({
        "type": "modify_request",
        "headers": [{"op": "set", "name": "Authorization", "value": "Bearer x"}],
        "query": [{"op": "set", "name": "env", "value": "staging"}],
    });

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({"name": "auth", "action": action}),
        ))
        .await;

    assert_eq!(rule(&response)["action"], action);

    server.stop().await;
}

#[tokio::test]
async fn a_redirect_rule_is_created_with_the_default_status() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({
                "name": "moved",
                "action": {
                    "type": "redirect",
                    "target": {"scheme": "https", "host": "new.example.com"},
                },
            }),
        ))
        .await;

    assert_eq!(rule(&response)["action"]["status"], 302);

    server.stop().await;
}

#[tokio::test]
async fn a_transformation_that_could_split_a_header_is_refused_by_name() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({
                "name": "smuggle",
                "action": {
                    "type": "modify_response",
                    "headers": [{"op": "set", "name": "X-A", "value": "1\r\nSet-Cookie: x"}],
                },
            }),
        ))
        .await;

    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "invalid_rule");
    assert_eq!(
        response["error"]["message"],
        "a modify header value must not contain line breaks"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_redirect_to_a_status_that_does_not_redirect_is_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let response = client
        .request(&request_with(
            "r1",
            "create_rule",
            json!({
                "name": "moved",
                "action": {
                    "type": "redirect",
                    "target": {"scheme": "https", "host": "new.example.com"},
                    "status": 303,
                },
            }),
        ))
        .await;

    assert_eq!(response["error"]["code"], "invalid_rule");
    assert_eq!(
        response["error"]["message"],
        "a redirect status must be 301, 302, 307 or 308"
    );

    server.stop().await;
}
