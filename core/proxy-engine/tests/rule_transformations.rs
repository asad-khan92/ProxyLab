//! End-to-end tests for Modify Request, Modify Response and Redirect.
//!
//! Every test runs the real listener — or the transparent bridge, or the
//! replayer — against in-process origins that record what they received, and
//! asserts two things: what arrived at the far end, and what the capture
//! recorded. The second matters as much as the first: an exchange is
//! remembered by the traffic that actually went past, after the rules
//! changed it, and never as a pair of before and after.
//!
//! Nothing reaches the network, nothing reads a developer's CA, and every
//! wait is on something the other side did.

mod support;

use std::sync::Arc;

use proxy_engine::capture::Capture;
use proxy_engine::replay::Replayer;
use proxy_engine::upstream::UpstreamManager;
use proxy_http::RequestSpec;
use proxy_ipc::{
    BreakpointControl, BreakpointDirection, NewBreakpoint, PauseDecision, PauseNotice, PauseStream,
};
use proxy_storage::{HeaderValue, TrafficExchange};
use proxy_tls::UpstreamTrust;
use rules::{
    BodyEdit, HeaderEdit, ModifyRequest, ModifyResponse, NewRule, PathEdit, QueryEdit, Rule,
    RuleAction, RuleEngine, RuleMatcher, RuleSet,
};
use support::http2::{Http2Client, Http2Origin, Http2Reply};
use support::{
    interceptor, next_exchange, origin_form_request, request, request_with_body, HttpsClient,
    Origin, Proxy, TlsOrigin, TEST_TIMEOUT,
};
use tokio::time::timeout;

const OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Origin: yes\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";

fn rule(name: &str, matcher: RuleMatcher, action: RuleAction, priority: i32) -> Rule {
    Rule::create(NewRule::new(name, matcher, action).with_priority(priority)).expect("a valid rule")
}

fn rules(rules: impl IntoIterator<Item = Rule>) -> RuleSet {
    RuleSet::new(rules)
}

fn set_header(name: &str, value: &str) -> HeaderEdit {
    HeaderEdit::Set {
        name: name.into(),
        value: value.into(),
    }
}

fn modify_request(change: ModifyRequest) -> RuleAction {
    RuleAction::ModifyRequest(change)
}

fn modify_response(change: ModifyResponse) -> RuleAction {
    RuleAction::ModifyResponse(change)
}

fn adding_header(name: &str, value: &str) -> RuleAction {
    modify_request(ModifyRequest {
        headers: vec![set_header(name, value)],
        ..ModifyRequest::default()
    })
}

/// A captured header's value, as text.
fn captured<'a>(exchange: &'a TrafficExchange, name: &str) -> Option<&'a str> {
    match exchange.request.headers.get(name)? {
        HeaderValue::Text(text) => Some(text),
        _ => None,
    }
}

fn captured_response<'a>(exchange: &'a TrafficExchange, name: &str) -> Option<&'a str> {
    match exchange.response.as_ref()?.headers.get(name)? {
        HeaderValue::Text(text) => Some(text),
        _ => None,
    }
}

async fn plaintext_proxy(rule_set: RuleSet) -> Proxy {
    Proxy::ruled(
        UpstreamTrust::PublicRoots,
        Capture::with_defaults(),
        rule_set,
    )
    .await
}

// MARK: - Requests, over HTTP/1.1

#[tokio::test]
async fn a_request_is_sent_with_its_method_path_query_and_headers_changed() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "rewrite",
        RuleMatcher::any().with_path_prefix("/v1/"),
        modify_request(ModifyRequest {
            method: Some("POST".into()),
            path: Some(PathEdit::ReplacePrefix {
                from: "/v1/".into(),
                to: "/v2/".into(),
            }),
            query: vec![
                QueryEdit::Set {
                    name: "env".into(),
                    value: "staging".into(),
                },
                QueryEdit::Remove {
                    name: "debug".into(),
                },
            ],
            headers: vec![
                set_header("Authorization", "Bearer rule-token"),
                HeaderEdit::Remove {
                    name: "Cookie".into(),
                },
            ],
            body: None,
        }),
        0,
    )]))
    .await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/v1/users?env=prod&debug=1&page=2", origin.addr()),
        &[("Cookie", "session=secret")],
    )
    .await;

    assert_eq!(response.status, 200);
    let received = origin.only_request();
    assert_eq!(received.method, "POST");
    assert_eq!(received.target, "/v2/users?env=staging&page=2");
    assert_eq!(received.header("authorization"), Some("Bearer rule-token"));
    assert_eq!(received.header("cookie"), None);

    // The capture is the request that was sent.
    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.request.method, "POST");
    assert_eq!(
        exchange.request.url,
        format!("http://{}/v2/users?env=staging&page=2", origin.addr())
    );
    assert_eq!(
        captured(&exchange, "authorization"),
        Some("Bearer rule-token")
    );
    assert_eq!(captured(&exchange, "cookie"), None);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_body_is_replaced_and_captured_as_the_replacement() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "body",
        RuleMatcher::any().with_method("POST"),
        modify_request(ModifyRequest {
            body: Some(BodyEdit::Replace {
                text: "{\"replaced\":1}".into(),
            }),
            ..ModifyRequest::default()
        }),
        0,
    )]))
    .await;
    let mut events = proxy.events();

    let response = request_with_body(
        proxy.addr(),
        "POST",
        &format!("http://{}/orders", origin.addr()),
        &[("Content-Type", "application/json")],
        "{\"original\":true}",
    )
    .await;

    assert_eq!(response.status, 200);
    let received = origin.only_request();
    assert_eq!(received.body_text(), "{\"replaced\":1}");
    assert_eq!(received.header("content-length"), Some("14"));

    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.request.body.data(), b"{\"replaced\":1}");

    proxy.stop().await;
}

#[tokio::test]
async fn a_json_field_in_a_request_body_is_set() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "json",
        RuleMatcher::any(),
        modify_request(ModifyRequest {
            body: Some(BodyEdit::SetJson {
                path: "user.role".into(),
                value: "\"admin\"".into(),
            }),
            ..ModifyRequest::default()
        }),
        0,
    )]))
    .await;

    request_with_body(
        proxy.addr(),
        "POST",
        &format!("http://{}/users", origin.addr()),
        &[],
        "{\"user\":{\"role\":\"viewer\",\"id\":3}}",
    )
    .await;

    assert_eq!(
        origin.only_request().body_text(),
        "{\"user\":{\"id\":3,\"role\":\"admin\"}}"
    );

    proxy.stop().await;
}

// MARK: - Responses

#[tokio::test]
async fn a_response_is_given_its_status_headers_and_body_and_captured_as_given() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "not found",
        RuleMatcher::any(),
        modify_response(ModifyResponse {
            status: Some(404),
            headers: vec![
                set_header("X-Rule", "applied"),
                HeaderEdit::Remove {
                    name: "X-Origin".into(),
                },
            ],
            body: Some(BodyEdit::Replace {
                text: "{\"error\":\"gone\"}".into(),
            }),
        }),
        0,
    )]))
    .await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/users", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 404);
    assert_eq!(response.header("x-rule"), Some("applied"));
    assert_eq!(response.header("x-origin"), None);
    assert_eq!(response.body_text(), "{\"error\":\"gone\"}");

    let exchange = next_exchange(&mut events).await;
    let captured = exchange.response.as_ref().expect("a response");
    assert_eq!(captured.status, 404);
    assert_eq!(captured_response(&exchange, "x-rule"), Some("applied"));
    assert_eq!(captured.body.data(), b"{\"error\":\"gone\"}");
    // The origin was asked and did answer: a response transformation is not
    // a block.
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test]
async fn a_json_field_in_a_response_body_is_set() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "flip",
        RuleMatcher::any(),
        modify_response(ModifyResponse {
            body: Some(BodyEdit::SetJson {
                path: "ok".into(),
                value: "false".into(),
            }),
            ..ModifyResponse::default()
        }),
        0,
    )]))
    .await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/status", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.body_text(), "{\"ok\":false}");
    assert_eq!(response.header("content-length"), Some("12"));

    proxy.stop().await;
}

// MARK: - Redirect

#[tokio::test]
async fn a_redirect_answers_with_its_location_and_never_asks_the_origin() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "moved",
        RuleMatcher::any().with_path("/old"),
        RuleAction::redirect("https://new.example.com/new?from=old", 307).expect("a redirect"),
        0,
    )]))
    .await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "POST",
        &format!("http://{}/old", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 307);
    assert_eq!(
        response.header("location"),
        Some("https://new.example.com/new?from=old")
    );
    assert!(origin.requests().is_empty());

    let exchange = next_exchange(&mut events).await;
    assert_eq!(exchange.response.as_ref().map(|r| r.status), Some(307));

    proxy.stop().await;
}

// MARK: - Order

#[tokio::test]
async fn where_two_rules_set_the_same_header_the_higher_one_wins() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([
        rule("low", RuleMatcher::any(), adding_header("X-Env", "low"), 20),
        rule(
            "high",
            RuleMatcher::any(),
            adding_header("X-Env", "high"),
            10,
        ),
    ]))
    .await;

    request(
        proxy.addr(),
        "GET",
        &format!("http://{}/", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(origin.only_request().header("x-env"), Some("high"));

    proxy.stop().await;
}

#[tokio::test]
async fn a_blocked_request_is_not_transformed_and_is_recorded_as_sent() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([
        rule("auth", RuleMatcher::any(), adding_header("X-Added", "1"), 0),
        rule("block", RuleMatcher::any(), RuleAction::block(), 10),
    ]))
    .await;
    let mut events = proxy.events();

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 403);
    assert!(origin.requests().is_empty());
    let exchange = next_exchange(&mut events).await;
    assert_eq!(captured(&exchange, "x-added"), None);

    proxy.stop().await;
}

#[tokio::test]
async fn a_transformation_below_the_deciding_rule_is_not_applied() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([
        rule("allow", RuleMatcher::any(), RuleAction::Allow, 0),
        rule(
            "auth",
            RuleMatcher::any(),
            adding_header("X-Added", "1"),
            10,
        ),
    ]))
    .await;

    request(
        proxy.addr(),
        "GET",
        &format!("http://{}/", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(origin.only_request().header("x-added"), None);

    proxy.stop().await;
}

#[tokio::test]
async fn a_transformed_request_is_mapped_somewhere_else_with_its_changes() {
    let named = Origin::responding_with(OK).await;
    let mapped = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([
        rule("auth", RuleMatcher::any(), adding_header("X-Added", "1"), 0),
        rule(
            "staging",
            RuleMatcher::any(),
            RuleAction::map_remote(&format!("http://{}", mapped.addr())).expect("a destination"),
            10,
        ),
    ]))
    .await;

    request(
        proxy.addr(),
        "GET",
        &format!("http://{}/users", named.addr()),
        &[],
    )
    .await;

    assert!(named.requests().is_empty());
    let received = mapped.only_request();
    assert_eq!(received.target, "/users");
    assert_eq!(received.header("x-added"), Some("1"));

    proxy.stop().await;
}

// MARK: - HTTPS and HTTP/2

#[tokio::test]
async fn an_intercepted_request_and_its_response_are_transformed() {
    let origin = TlsOrigin::responding_with("localhost", OK).await;
    let proxy = Proxy::ruled(
        origin.trust(),
        Capture::with_defaults(),
        rules([
            rule(
                "auth",
                RuleMatcher::any(),
                adding_header("Authorization", "Bearer t"),
                0,
            ),
            rule(
                "teapot",
                RuleMatcher::any(),
                modify_response(ModifyResponse {
                    status: Some(418),
                    ..ModifyResponse::default()
                }),
                1,
            ),
        ]),
    )
    .await;
    let client = HttpsClient::trusting(proxy.trust());

    let response = client
        .exchange(
            proxy.addr(),
            &origin.authority(),
            origin.host(),
            &origin_form_request("GET", "/secure", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 418);
    assert_eq!(
        origin.only_request().header("authorization"),
        Some("Bearer t")
    );

    proxy.stop().await;
}

#[tokio::test]
async fn an_http2_stream_and_its_response_are_transformed() {
    let origin = Http2Origin::responding_with("localhost", Http2Reply::ok("from h2")).await;
    let proxy = Proxy::ruled(
        origin.trust(),
        Capture::with_defaults(),
        rules([
            rule(
                "auth",
                RuleMatcher::any(),
                adding_header("X-Added", "h2"),
                0,
            ),
            rule(
                "body",
                RuleMatcher::any(),
                modify_response(ModifyResponse {
                    headers: vec![set_header("X-Rule", "1")],
                    body: Some(BodyEdit::Replace {
                        text: "replaced over h2".into(),
                    }),
                    ..ModifyResponse::default()
                }),
                1,
            ),
        ]),
    )
    .await;
    let client = Http2Client::trusting(proxy.trust());

    let connection = client
        .connect(proxy.addr(), &origin.authority(), origin.host())
        .await;
    let response = connection.send("GET", "/stream", &[], Vec::new()).await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"replaced over h2");
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "x-rule" && value == "1"));
    assert_eq!(origin.only_request().header("x-added"), Some("h2"));

    proxy.stop().await;
}

// MARK: - Transparent capture

#[tokio::test]
async fn a_transparently_captured_flow_is_transformed() {
    let origin = TlsOrigin::responding_with("localhost", OK).await;
    let bridge = Proxy::bridge(
        origin.trust(),
        rules([rule(
            "auth",
            RuleMatcher::any().with_host("localhost"),
            adding_header("Authorization", "Bearer transparent"),
            0,
        )]),
    )
    .await;
    let client = HttpsClient::trusting(bridge.trust());

    let response = client
        .exchange(
            bridge.addr(),
            &origin.addr().to_string(),
            "localhost",
            &origin_form_request("GET", "/", &origin.authority(), &[]),
        )
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        origin.only_request().header("authorization"),
        Some("Bearer transparent")
    );

    bridge.stop().await;
}

// MARK: - Replay

#[tokio::test]
async fn a_replayed_request_is_transformed_like_any_other() {
    let origin = Origin::responding_with(OK).await;
    let capture = Capture::with_defaults();
    let mut events = capture.subscribe();
    let replayer = Replayer::new(
        capture,
        Arc::new(UpstreamManager::new(interceptor(
            UpstreamTrust::PublicRoots,
        ))),
        Some(Arc::new(RuleEngine::with(rules([
            rule(
                "auth",
                RuleMatcher::any(),
                adding_header("X-Replayed", "ruled"),
                0,
            ),
            rule(
                "status",
                RuleMatcher::any(),
                modify_response(ModifyResponse {
                    status: Some(202),
                    ..ModifyResponse::default()
                }),
                1,
            ),
        ])))),
    );

    let outcome = replayer
        .replay(RequestSpec::get(&format!("http://{}/again", origin.addr())).expect("a request"))
        .await;

    // The status a replay reports is the one the exchange is captured with:
    // the answer after the rules, as it is for every other request.
    assert_eq!(outcome.status, Some(202));
    assert_eq!(origin.only_request().header("x-replayed"), Some("ruled"));
    let exchange = next_exchange(&mut events).await;
    assert_eq!(captured(&exchange, "x-replayed"), Some("ruled"));
    assert_eq!(exchange.response.as_ref().map(|r| r.status), Some(202));
}

// MARK: - Breakpoints

#[tokio::test(flavor = "multi_thread")]
async fn a_response_breakpoint_shows_the_response_the_rules_made() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "status",
        RuleMatcher::any(),
        modify_response(ModifyResponse {
            status: Some(404),
            ..ModifyResponse::default()
        }),
        0,
    )]))
    .await;
    let mut watcher = PauseStream::attach(
        Arc::clone(proxy.controls().breakpoints()) as Arc<dyn BreakpointControl>
    );
    proxy.controls().breakpoints().create(NewBreakpoint::new(
        BreakpointDirection::Response,
        RuleMatcher::any(),
    ));

    let addr = proxy.addr();
    let url = format!("http://{}/held", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = loop {
        match timeout(TEST_TIMEOUT, watcher.recv())
            .await
            .expect("an exchange is paused")
            .expect("the pause stream is healthy")
        {
            PauseNotice::Paused(paused) => break paused,
            PauseNotice::Released { .. } => continue,
        }
    };
    assert_eq!(paused.response.as_ref().map(|r| r.status), Some(404));

    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::unchanged());

    let response = sending.await.expect("the client finishes");
    assert_eq!(response.status, 404);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_edited_at_a_breakpoint_is_the_one_the_rules_transform() {
    let origin = Origin::responding_with(OK).await;
    let proxy = plaintext_proxy(rules([rule(
        "auth",
        RuleMatcher::any().with_path("/edited"),
        adding_header("X-Added", "after-edit"),
        0,
    )]))
    .await;
    let mut watcher = PauseStream::attach(
        Arc::clone(proxy.controls().breakpoints()) as Arc<dyn BreakpointControl>
    );
    proxy.controls().breakpoints().create(NewBreakpoint::new(
        BreakpointDirection::Request,
        RuleMatcher::any(),
    ));

    let addr = proxy.addr();
    let url = format!("http://{}/original", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = loop {
        match timeout(TEST_TIMEOUT, watcher.recv())
            .await
            .expect("an exchange is paused")
            .expect("the pause stream is healthy")
        {
            PauseNotice::Paused(paused) => break paused,
            PauseNotice::Released { .. } => continue,
        }
    };
    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        PauseDecision::Continue {
            request: Some(proxy_ipc::RequestEdit {
                method: None,
                url: Some(format!("http://{}/edited", origin.addr())),
                headers: None,
                body: None,
            }),
            response: None,
            websocket: None,
        },
    );

    sending.await.expect("the client finishes");

    let received = origin.only_request();
    assert_eq!(received.target, "/edited");
    assert_eq!(received.header("x-added"), Some("after-edit"));

    proxy.stop().await;
}
