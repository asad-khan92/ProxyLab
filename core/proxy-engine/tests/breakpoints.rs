//! Breakpoints, against the real proxy.
//!
//! Every test starts the actual listener, attaches a watcher the way the IPC
//! server does, sends an actual request through it, and answers the pause
//! from a second task. What is being proved is not that the registry matches
//! correctly — `proxy_engine::controls::breakpoint` proves that with no proxy
//! in the picture — but that a pause actually stops traffic: that an edited
//! request is the one the origin receives, that a cancelled one never reaches
//! the origin at all, and that an exchange nobody comes back to is released
//! rather than held for ever.
//!
//! Nothing here reaches the network. The origins are servers the test starts
//! on loopback, and every certificate comes from a CA generated per test.
//!
//! # Why the client runs in a task
//!
//! A paused request is a client waiting for an answer that is not coming yet.
//! So the client is spawned, the test plays the part of the UI — take the
//! pause, edit it, continue it — and only then is the client's answer awaited.
//! A test that sent the request and then looked for the pause would be a test
//! that deadlocked on itself.

mod support;

use std::sync::Arc;

use proxy_engine::capture::Capture;
use proxy_ipc::{
    BreakpointControl, BreakpointDirection, NewBreakpoint, PauseDecision, PauseNotice, PauseStream,
    PausedExchange, RequestEdit, ResponseEdit,
};
use proxy_storage::{HeaderValue, Headers, TrafficEvent};
use rules::RuleMatcher;
use support::http2::{Http2Client, Http2Origin, Http2Reply};
use support::{
    next_exchange, request, request_with_body, HttpsClient, Origin, Proxy, TlsOrigin, TEST_TIMEOUT,
};
use tokio::time::timeout;

/// A plain `200 OK` from the origin.
const UPSTREAM: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 8\r\n\r\nupstream";

/// The host every TLS end in these tests is known by.
const HOST: &str = "localhost";

/// Watches this proxy's breakpoints, as an IPC client does.
///
/// Holding the returned stream is what makes the proxy pause at all, and
/// dropping it is what releases everything it was holding — which is the
/// disconnect case one of these tests is about.
fn watching(proxy: &Proxy) -> PauseStream {
    PauseStream::attach(Arc::clone(proxy.controls().breakpoints()) as Arc<dyn BreakpointControl>)
}

/// Configures a breakpoint on `direction` for requests to `host`.
fn breakpoint(proxy: &Proxy, direction: BreakpointDirection, host: &str) {
    proxy.controls().breakpoints().create(NewBreakpoint::new(
        direction,
        RuleMatcher::any().with_host(host),
    ));
}

/// The next exchange a watcher is told about.
async fn next_pause(watcher: &mut PauseStream) -> PausedExchange {
    loop {
        match timeout(TEST_TIMEOUT, watcher.recv())
            .await
            .expect("an exchange is paused")
            .expect("the pause stream is healthy")
        {
            PauseNotice::Paused(paused) => return paused,
            // A release for a pause an earlier test step resolved.
            PauseNotice::Released { .. } => continue,
        }
    }
}

/// A header section holding one header.
fn one_header(name: &str, value: &str) -> Headers {
    let mut headers = Headers::new();
    headers.push(name, HeaderValue::Text(value.to_owned()));

    headers
}

/// A decision that changes a request.
fn edit_request(edit: RequestEdit) -> PauseDecision {
    PauseDecision::Continue {
        request: Some(edit),
        response: None,
        websocket: None,
    }
}

/// A decision that changes a response.
fn edit_response(edit: ResponseEdit) -> PauseDecision {
    PauseDecision::Continue {
        request: None,
        response: Some(edit),
        websocket: None,
    }
}

// MARK: - Request breakpoints

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_request_is_the_one_the_origin_receives() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/orders", origin.addr());
    let sending =
        tokio::spawn(async move { request_with_body(addr, "POST", &url, &[], "{\"id\":1}").await });

    let paused = next_pause(&mut watcher).await;
    let shown = paused.request.as_ref().expect("a request pause");
    assert_eq!(shown.method, "POST");
    assert!(shown.url.ends_with("/orders"), "{}", shown.url);
    assert_eq!(shown.body.data(), b"{\"id\":1}");

    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_request(RequestEdit {
            method: Some("PUT".to_owned()),
            url: Some(format!("http://{}/v2/orders?dry=1", origin.addr())),
            headers: Some(one_header("x-edited", "yes")),
            body: Some(b"{\"id\":2}".to_vec()),
        }),
    );

    let response = sending.await.expect("the client finishes");
    assert_eq!(response.status, 200);

    let received = origin.only_request();
    assert_eq!(received.method, "PUT");
    assert_eq!(received.target, "/v2/orders?dry=1");
    assert_eq!(received.header("x-edited"), Some("yes"));
    assert_eq!(received.body_text(), "{\"id\":2}");
    assert_eq!(received.header("content-length"), Some("8"));

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_continued_unedited_reaches_the_origin_unchanged() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::unchanged());

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "upstream");
    assert_eq!(origin.only_request().target, "/hello");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_request_never_reaches_the_origin() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::Cancel);

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 502);
    assert!(
        response.body_text().contains("breakpoint"),
        "{}",
        response.body_text()
    );
    assert!(
        origin.requests().is_empty(),
        "the origin should never have been contacted"
    );

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_breakpoint_for_another_host_does_not_pause_anything() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let _watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        "somewhere.else.invalid",
    );

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(proxy.controls().breakpoints().pending(), 0);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_paused_when_nobody_is_watching() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    // Configured, and deliberately not watched.
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let response = request(
        proxy.addr(),
        "GET",
        &format!("http://{}/hello", origin.addr()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.requests().len(), 1);
    assert_eq!(proxy.controls().breakpoints().pending(), 0);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_going_away_releases_the_request_it_was_holding() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    next_pause(&mut watcher).await;
    assert_eq!(proxy.controls().breakpoints().pending(), 1);

    // The UI disappears. Nothing is left to answer the pause, so the request
    // carries on exactly as if no breakpoint had matched.
    drop(watcher);

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "upstream");
    assert_eq!(origin.only_request().target, "/hello");
    assert_eq!(proxy.controls().breakpoints().pending(), 0);

    proxy.stop().await;
}

// MARK: - Response breakpoints

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_response_is_the_one_the_client_receives() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Response,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = next_pause(&mut watcher).await;
    let shown = paused.response.as_ref().expect("a response pause");
    assert_eq!(shown.status, 200);
    assert_eq!(shown.body.data(), b"upstream");
    assert!(paused.request.is_none());

    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_response(ResponseEdit {
            status: Some(503),
            headers: Some(one_header("retry-after", "30")),
            body: Some(b"go away".to_vec()),
        }),
    );

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 503);
    assert_eq!(response.header("retry-after"), Some("30"));
    assert_eq!(response.body_text(), "go away");
    // The origin was reached: a response breakpoint stops the answer, not the
    // question.
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_response_is_not_delivered() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Response,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::Cancel);

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 502);
    assert!(!response.body_text().contains("upstream"));
    assert_eq!(origin.requests().len(), 1);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_going_away_releases_the_response_it_was_holding() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::start().await;
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Response,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    next_pause(&mut watcher).await;
    drop(watcher);

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "upstream");

    proxy.stop().await;
}

// MARK: - What is captured

#[tokio::test(flavor = "multi_thread")]
async fn the_captured_exchange_is_the_edited_request_and_the_edited_response() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::with_capture(Capture::with_defaults()).await;
    let mut events = proxy.events();
    let mut watcher = watching(&proxy);
    let host = origin.addr().ip().to_string();
    breakpoint(&proxy, BreakpointDirection::Request, &host);
    breakpoint(&proxy, BreakpointDirection::Response, &host);

    let addr = proxy.addr();
    let url = format!("http://{}/original", origin.addr());
    let edited_url = format!("http://{}/edited", origin.addr());
    let sending =
        tokio::spawn(async move { request_with_body(addr, "POST", &url, &[], "before").await });

    let paused = next_pause(&mut watcher).await;
    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_request(RequestEdit {
            url: Some(edited_url),
            body: Some(b"after".to_vec()),
            ..RequestEdit::default()
        }),
    );

    let paused = next_pause(&mut watcher).await;
    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_response(ResponseEdit {
            status: Some(418),
            body: Some(b"teapot".to_vec()),
            ..ResponseEdit::default()
        }),
    );

    let response = sending.await.expect("the client finishes");
    assert_eq!(response.status, 418);

    let exchange: TrafficEvent = next_exchange(&mut events).await;

    // One exchange, and it is the one that actually happened: there is no
    // original-versus-modified history here, deliberately.
    assert!(exchange.request.url.ends_with("/edited"), "{:?}", exchange);
    assert_eq!(exchange.request.body.data(), b"after");

    let captured = exchange.response.as_ref().expect("a response was captured");
    assert_eq!(captured.status, 418);
    assert_eq!(captured.body.data(), b"teapot");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_request_is_still_in_the_traffic_list() {
    let origin = Origin::responding_with(UPSTREAM).await;
    let proxy = Proxy::with_capture(Capture::with_defaults()).await;
    let mut events = proxy.events();
    let mut watcher = watching(&proxy);
    breakpoint(
        &proxy,
        BreakpointDirection::Request,
        &origin.addr().ip().to_string(),
    );

    let addr = proxy.addr();
    let url = format!("http://{}/hello", origin.addr());
    let sending = tokio::spawn(async move { request(addr, "GET", &url, &[]).await });

    let paused = next_pause(&mut watcher).await;
    proxy
        .controls()
        .breakpoints()
        .resolve(paused.pause_id, PauseDecision::Cancel);
    sending.await.expect("the client finishes");

    let exchange = next_exchange(&mut events).await;

    assert!(exchange.request.url.ends_with("/hello"));
    assert!(exchange.response.is_none());
    assert!(
        exchange
            .error
            .as_ref()
            .expect("a cancelled request records why")
            .summary()
            .contains("breakpoint"),
        "{:?}",
        exchange.error
    );

    proxy.stop().await;
}

// MARK: - HTTPS and HTTP/2

#[tokio::test(flavor = "multi_thread")]
async fn an_intercepted_request_can_be_paused_and_edited() {
    let origin = TlsOrigin::responding_with(HOST, UPSTREAM).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let mut watcher = watching(&proxy);
    breakpoint(&proxy, BreakpointDirection::Request, HOST);

    let client = HttpsClient::trusting(proxy.trust());
    let addr = proxy.addr();
    let authority = origin.authority();
    let sending = tokio::spawn(async move {
        client
            .exchange(
                addr,
                &authority,
                HOST,
                b"GET /original HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
    });

    let paused = next_pause(&mut watcher).await;
    let shown = paused.request.as_ref().expect("a request pause");
    assert!(shown.url.starts_with("https://"), "{}", shown.url);

    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_request(RequestEdit {
            // Only the path and query are taken inside an intercepted
            // session; the host is the one the session was opened to.
            url: Some("https://elsewhere.invalid/edited".to_owned()),
            ..RequestEdit::default()
        }),
    );

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 200);
    assert_eq!(origin.only_request().target, "/edited");

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_http_2_stream_can_be_paused_and_its_response_edited() {
    let origin = Http2Origin::responding_with(HOST, Http2Reply::ok("upstream")).await;
    let proxy = Proxy::trusting(origin.trust()).await;
    let mut watcher = watching(&proxy);
    breakpoint(&proxy, BreakpointDirection::Response, HOST);

    let client = Http2Client::trusting(proxy.trust());
    let addr = proxy.addr();
    let authority = origin.authority();
    let sending = tokio::spawn(async move {
        client
            .exchange(addr, &authority, HOST, "GET", "/streamed")
            .await
    });

    let paused = next_pause(&mut watcher).await;
    assert_eq!(paused.method, "GET");
    assert_eq!(
        paused
            .response
            .as_ref()
            .expect("a response pause")
            .body
            .data(),
        b"upstream"
    );

    proxy.controls().breakpoints().resolve(
        paused.pause_id,
        edit_response(ResponseEdit {
            status: Some(404),
            body: Some(b"gone".to_vec()),
            ..ResponseEdit::default()
        }),
    );

    let response = sending.await.expect("the client finishes");

    assert_eq!(response.status, 404);
    assert_eq!(response.body_text(), "gone");

    proxy.stop().await;
}
