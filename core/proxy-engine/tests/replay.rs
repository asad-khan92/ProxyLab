//! End-to-end tests for replaying a request through the proxy's pipeline.
//!
//! Every test here sends a real request to a real in-process origin server on
//! an OS-assigned port, through [`Replayer`] — the entry point the IPC
//! boundary reaches. Nothing reaches the network, DNS, or any external
//! service, and no test runs a process.
//!
//! What is asserted throughout is that a replay is *ordinary traffic*: the
//! rules decide it, the capture records it, storage keeps it, and the event
//! stream publishes it, all through the same code a request from a browser
//! goes through.

mod support;

use std::path::Path;
use std::sync::Arc;

use support::{interceptor, next_exchange, Origin, TlsOrigin, TEST_TIMEOUT};

use proxy_engine::capture::Capture;
use proxy_engine::replay::Replayer;
use proxy_engine::upstream::UpstreamManager;
use proxy_http::RequestSpec;
use proxy_storage::{
    CaptureConfig, EventPublisher, EventSubscriber, ExchangeId, Header, Headers, Scheme, Storage,
    StorageConfig, StorageWorker, TrafficErrorKind, TrafficExchange, TrafficQuery,
};
use proxy_tls::UpstreamTrust;
use rules::{BlockAction, MapLocal, NewRule, RuleAction, RuleEngine, RuleMatcher, RuleSet};
use tempfile::TempDir;
use tokio::net::TcpListener;

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

/// What an origin answers a request that carried no usable credential with.
const UNAUTHORIZED: &[u8] =
    b"HTTP/1.1 401 Unauthorized\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\ndenied";

/// A replayer, the events it publishes, and the rules it consults.
struct Harness {
    replayer: Replayer,
    events: EventSubscriber,
    rules: Arc<RuleEngine>,
}

impl Harness {
    /// A replayer with no rules and no private trust.
    fn new() -> Self {
        Self::with(
            RuleSet::empty(),
            UpstreamTrust::PublicRoots,
            Capture::with_defaults(),
        )
    }

    /// A replayer trusting `trust`, which is what an HTTPS origin needs.
    fn trusting(trust: UpstreamTrust) -> Self {
        Self::with(RuleSet::empty(), trust, Capture::with_defaults())
    }

    /// A replayer with `rules` in force.
    fn ruled(rules: RuleSet) -> Self {
        Self::with(rules, UpstreamTrust::PublicRoots, Capture::with_defaults())
    }

    /// A replayer built exactly as the binary builds one.
    fn with(rules: RuleSet, trust: UpstreamTrust, capture: Capture) -> Self {
        let engine = Arc::new(RuleEngine::with(rules));
        let events = capture.subscribe();

        Self {
            replayer: Replayer::new(
                capture,
                Arc::new(UpstreamManager::new(interceptor(trust))),
                Some(Arc::clone(&engine)),
            ),
            events,
            rules: engine,
        }
    }

    /// The exchange the proxy published for the replay that just happened.
    async fn published(&mut self) -> Arc<TrafficExchange> {
        next_exchange(&mut self.events).await
    }
}

/// A `GET` of `url` with no headers and no body.
fn get(url: &str) -> RequestSpec {
    RequestSpec::get(url).expect("a valid request")
}

/// A request with headers and a body.
fn request(method: &str, url: &str, entries: &[(&str, &str)], body: &[u8]) -> RequestSpec {
    let headers: Headers = entries
        .iter()
        .map(|(name, value)| Header::text(*name, *value))
        .collect();

    RequestSpec::new(method, url, headers, body.to_vec()).expect("a valid request")
}

/// A port nothing is listening on.
///
/// Bound and then dropped, so the port was free a moment ago and nothing has
/// been put on it.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let port = listener.local_addr().expect("local address").port();
    drop(listener);

    port
}

// MARK: - Plain HTTP

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_reaches_the_origin_and_comes_back() {
    let origin = Origin::responding_with(HELLO).await;
    let harness = Harness::new();

    let outcome = harness
        .replayer
        .replay(get(&format!("{}/api/users", origin.base_url())))
        .await;

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.status, Some(200));

    let received = origin.only_request();
    assert_eq!(received.method, "GET");
    assert_eq!(received.target, "/api/users");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_carries_its_method_headers_and_body() {
    let origin = Origin::responding_with(HELLO).await;
    let harness = Harness::new();

    let outcome = harness
        .replayer
        .replay(request(
            "POST",
            &format!("{}/api/users", origin.base_url()),
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer TEST_TOKEN"),
                ("X-Trace", "one"),
                ("X-Trace", "two"),
            ],
            br#"{"name":"Ada"}"#,
        ))
        .await;

    assert!(outcome.succeeded(), "{outcome:?}");

    let received = origin.only_request();
    assert_eq!(received.method, "POST");
    assert_eq!(received.header("content-type"), Some("application/json"));
    assert_eq!(received.header("authorization"), Some("Bearer TEST_TOKEN"));
    assert_eq!(received.body_text(), r#"{"name":"Ada"}"#);
    // Repeats are sent as repeats, in order.
    let traces: Vec<&str> = received
        .headers
        .iter()
        .filter(|(name, _)| name == "x-trace")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(traces, ["one", "two"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_request_is_sent_with_the_host_the_url_names() {
    let origin = Origin::responding_with(HELLO).await;
    let harness = Harness::new();

    harness
        .replayer
        .replay(get(&format!("{}/thing", origin.base_url())))
        .await;

    let received = origin.only_request();
    assert_eq!(
        received.header("host"),
        Some(origin.addr().to_string().as_str())
    );
}

// MARK: - It is captured like everything else

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_is_published_as_an_exchange() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::new();
    let url = format!("{}/api/users", origin.base_url());

    let outcome = harness.replayer.replay(get(&url)).await;
    let exchange = harness.published().await;

    assert_eq!(exchange.id, outcome.exchange_id);
    assert_eq!(exchange.request.method, "GET");
    assert_eq!(exchange.request.url, url);
    assert_eq!(exchange.connection.scheme, Scheme::Http);
    assert_eq!(
        exchange.connection.destination.as_ref().map(|d| d.port),
        Some(origin.addr().port())
    );
    assert!(exchange.succeeded(), "{exchange:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_captures_both_bodies_and_its_timing() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::new();

    harness
        .replayer
        .replay(request(
            "POST",
            &format!("{}/api", origin.base_url()),
            &[("Content-Type", "application/json")],
            br#"{"a":1}"#,
        ))
        .await;
    let exchange = harness.published().await;

    assert_eq!(exchange.request.body.data(), br#"{"a":1}"#);
    let response = exchange.response.as_ref().expect("a response");
    assert_eq!(response.status, 200);
    assert_eq!(response.body.data(), b"hello");
    assert!(exchange.duration_ms().is_some(), "{exchange:?}");
    assert!(
        exchange.timing.upstream_response_micros.is_some(),
        "{exchange:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_obeys_the_capture_limit() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::with(
        RuleSet::empty(),
        UpstreamTrust::PublicRoots,
        Capture::new(CaptureConfig::new(2), EventPublisher::new()),
    );

    harness
        .replayer
        .replay(request(
            "POST",
            &format!("{}/api", origin.base_url()),
            &[],
            b"a much longer body than two bytes",
        ))
        .await;
    let exchange = harness.published().await;

    assert!(exchange.request.body.is_truncated());
    assert_eq!(exchange.request.body.captured_len(), 2);
    assert_eq!(exchange.request.body.size(), 33);
    let response = exchange.response.as_ref().expect("a response");
    assert!(response.body.is_truncated());
    assert_eq!(response.body.captured_len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_replay_gets_an_exchange_id_of_its_own() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::new();
    let url = format!("{}/api", origin.base_url());

    let first = harness.replayer.replay(get(&url)).await;
    let second = harness.replayer.replay(get(&url)).await;

    assert_ne!(first.exchange_id, second.exchange_id);

    let published: Vec<ExchangeId> =
        vec![harness.published().await.id, harness.published().await.id];
    assert_eq!(published, [first.exchange_id, second.exchange_id]);
}

// MARK: - It is stored like everything else

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_is_stored_and_leaves_the_exchange_it_came_from_alone() {
    let origin = Origin::responding_with(HELLO).await;
    let directory = TempDir::new().expect("a temporary directory");
    let storage =
        Storage::open(StorageConfig::in_directory(directory.path())).expect("a store opens");
    let worker = StorageWorker::spawn(storage.clone());

    let capture = Capture::with_defaults().storing_into(Arc::new(worker.handle()));
    let mut harness = Harness::with(RuleSet::empty(), UpstreamTrust::PublicRoots, capture);
    let url = format!("{}/api/users", origin.base_url());

    // The exchange a developer would have copied the request out of.
    let original = harness.replayer.replay(get(&url)).await;
    harness.published().await;
    assert!(worker.handle().flush().await);
    let before = storage
        .get(original.exchange_id)
        .expect("the store answers")
        .expect("the original is stored");

    // The replay of it.
    let replay = harness.replayer.replay(get(&url)).await;
    harness.published().await;
    assert!(worker.handle().flush().await);

    assert_ne!(replay.exchange_id, original.exchange_id);

    let after = storage
        .get(original.exchange_id)
        .expect("the store answers")
        .expect("the original is still stored");
    assert_eq!(after, before, "the original exchange was changed");

    let stored_replay = storage
        .get(replay.exchange_id)
        .expect("the store answers")
        .expect("the replay is stored");
    assert_eq!(stored_replay.request.url, url);
    assert_eq!(stored_replay.status(), Some(200));

    // Two rows, not one edited twice.
    let page = storage
        .list(&TrafficQuery::newest())
        .expect("the store answers");
    assert_eq!(page.exchanges.len(), 2);

    worker.shutdown().await;
}

// MARK: - HTTPS

#[tokio::test(flavor = "multi_thread")]
async fn an_https_replay_goes_through_the_proxys_own_tls() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    let mut harness = Harness::trusting(origin.trust());

    let url = format!("https://{}/secure", origin.authority());
    let outcome = harness.replayer.replay(get(&url)).await;

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.status, Some(200));
    assert_eq!(origin.only_request().target, "/secure");

    let exchange = harness.published().await;
    assert_eq!(exchange.connection.scheme, Scheme::Https);
    assert_eq!(exchange.request.url, url);
    assert_eq!(exchange.status(), Some(200));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_https_replay_refuses_a_certificate_it_does_not_trust() {
    let origin = TlsOrigin::responding_with("localhost", HELLO).await;
    // Trusting the public roots, which this origin's own CA is not among.
    let mut harness = Harness::new();

    let outcome = harness
        .replayer
        .replay(get(&format!("https://{}/secure", origin.authority())))
        .await;

    assert!(!outcome.succeeded(), "{outcome:?}");
    assert_eq!(
        outcome
            .error
            .as_ref()
            .map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::UpstreamTls)
    );
    assert!(origin.requests().is_empty());

    let exchange = harness.published().await;
    assert!(exchange.response.is_none());
    assert_eq!(
        exchange
            .error
            .as_ref()
            .map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::UpstreamTls)
    );
}

// MARK: - Failure

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_that_cannot_connect_is_an_exchange_with_an_error() {
    let port = closed_port().await;
    let mut harness = Harness::new();

    let outcome = harness
        .replayer
        .replay(get(&format!("http://127.0.0.1:{port}/api")))
        .await;

    assert!(!outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.status, None);
    assert_eq!(
        outcome
            .error
            .as_ref()
            .map(proxy_storage::TrafficError::kind),
        Some(TrafficErrorKind::UpstreamConnect)
    );

    let exchange = harness.published().await;
    assert_eq!(exchange.id, outcome.exchange_id);
    assert!(exchange.response.is_none());
    assert_eq!(exchange.error, outcome.error);
}

// MARK: - Rules

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_a_rule_blocks_never_reaches_the_origin() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::ruled(RuleSet::new([blocking("127.0.0.1", 418)]));

    let outcome = harness
        .replayer
        .replay(get(&format!("{}/api", origin.base_url())))
        .await;

    assert_eq!(outcome.status, Some(418));
    assert!(
        origin.requests().is_empty(),
        "a blocked replay reached the origin"
    );

    // Blocked traffic is traffic: the exchange says what the rule answered.
    let exchange = harness.published().await;
    assert_eq!(exchange.status(), Some(418));
    assert!(exchange.response.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_a_rule_maps_locally_is_answered_from_the_file() {
    let directory = TempDir::new().expect("a temporary directory");
    let file = directory.path().join("fixture.json");
    std::fs::write(&file, br#"{"fixture":true}"#).expect("writes the fixture");

    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::ruled(RuleSet::new([rule(
        "maps locally",
        RuleAction::MapLocal(MapLocal::new(&file)),
    )]));

    let outcome = harness
        .replayer
        .replay(get(&format!("{}/api", origin.base_url())))
        .await;

    assert_eq!(outcome.status, Some(200));
    assert!(origin.requests().is_empty());

    let exchange = harness.published().await;
    let response = exchange.response.as_ref().expect("a response");
    assert_eq!(response.body.data(), br#"{"fixture":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_a_rule_maps_remotely_goes_to_the_other_destination() {
    let intended = Origin::responding_with(HELLO).await;
    let elsewhere = Origin::responding_with(HELLO).await;

    let mut harness = Harness::ruled(RuleSet::new([rule(
        "maps remotely",
        RuleAction::map_remote(&elsewhere.base_url()).expect("a valid destination"),
    )]));

    let outcome = harness
        .replayer
        .replay(get(&format!("{}/api", intended.base_url())))
        .await;

    assert_eq!(outcome.status, Some(200));
    assert!(intended.requests().is_empty());
    assert_eq!(elsewhere.only_request().target, "/api");

    // The exchange keeps the URL that was asked for and records where it
    // actually went, exactly as a mapped request from a browser does.
    let exchange = harness.published().await;
    assert!(exchange.request.url.starts_with(&intended.base_url()));
    assert_eq!(
        exchange.connection.destination.as_ref().map(|d| d.port),
        Some(elsewhere.addr().port())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rule_added_after_the_replayer_was_built_still_applies() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::new();
    let url = format!("{}/api", origin.base_url());

    let allowed = harness.replayer.replay(get(&url)).await;
    assert_eq!(allowed.status, Some(200));
    harness.published().await;

    // The same thing an IPC rule change does to the engine the listener
    // holds: the replayer is reading that engine, not a copy of it.
    harness
        .rules
        .replace(RuleSet::new([blocking("127.0.0.1", 403)]));

    let blocked = harness.replayer.replay(get(&url)).await;
    assert_eq!(blocked.status, Some(403));
    assert_eq!(origin.requests().len(), 1);
}

// MARK: - Authentication is request data and nothing else

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_credential_comes_back_as_whatever_the_server_says() {
    let origin = Origin::start(|received| {
        if received.header("authorization") == Some("Bearer TEST_VALID") {
            HELLO.to_vec()
        } else {
            UNAUTHORIZED.to_vec()
        }
    })
    .await;
    let mut harness = Harness::new();
    let url = format!("{}/api", origin.base_url());

    let refused = harness
        .replayer
        .replay(request(
            "GET",
            &url,
            &[("Authorization", "Bearer TEST_EXPIRED")],
            b"",
        ))
        .await;

    // No retry, no refresh, no login: the server said 401 and that is the
    // answer.
    assert_eq!(refused.status, Some(401));
    assert_eq!(origin.requests().len(), 1);

    let exchange = harness.published().await;
    assert_eq!(exchange.status(), Some(401));
    assert_eq!(
        exchange.response.as_ref().expect("a response").body.data(),
        b"denied"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cookie_is_sent_exactly_as_it_was_given() {
    let origin = Origin::responding_with(HELLO).await;
    let harness = Harness::new();

    harness
        .replayer
        .replay(request(
            "GET",
            &format!("{}/api", origin.base_url()),
            &[("Cookie", "session=TEST_SESSION; theme=dark")],
            b"",
        ))
        .await;

    assert_eq!(
        origin.only_request().header("cookie"),
        Some("session=TEST_SESSION; theme=dark")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_about_a_replayed_credential_is_in_the_exchange_debug() {
    let origin = Origin::responding_with(HELLO).await;
    let mut harness = Harness::new();

    harness
        .replayer
        .replay(request(
            "POST",
            &format!("{}/api", origin.base_url()),
            &[
                ("Authorization", "Bearer TEST_TOKEN"),
                ("Cookie", "session=TEST_SESSION"),
            ],
            br#"{"password":"TEST_PASSWORD"}"#,
        ))
        .await;
    let exchange = harness.published().await;

    let rendered = format!("{exchange:?} {:?} {}", harness.replayer, exchange.summary());
    for secret in ["TEST_TOKEN", "TEST_SESSION", "TEST_PASSWORD"] {
        assert!(
            !rendered.contains(secret),
            "{secret} appeared in {rendered}"
        );
    }
}

// MARK: - No second HTTP stack, and no processes

#[test]
fn replay_never_runs_a_process() {
    // The strongest form this check can take without watching the kernel:
    // no crate on the replay path has any way to start a process. Not the
    // engine that sends the request, not the parser that read the command,
    // not the boundary between them. If a future change puts a `Command`
    // in any of them, this fails and the change has to say why.
    //
    // `std::process::ExitCode`, which the binary returns, is not a way to
    // start anything and is not what this looks for.
    for crate_source in [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../curl/src"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../http/src"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ipc/src"),
    ] {
        for path in rust_files(&crate_source) {
            let source = std::fs::read_to_string(&path).expect("the source is readable");

            for forbidden in [
                "Command::new",
                "process::Command",
                "std::process::Command",
                "tokio::process",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{} mentions {forbidden}",
                    path.display()
                );
            }
        }
    }
}

/// Every `.rs` file under `directory`, recursively.
fn rust_files(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return files;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }

    files
}

/// A rule that blocks everything for `host` with `status`.
fn blocking(host: &str, status: u16) -> rules::Rule {
    rules::Rule::create(NewRule::new(
        "blocks the replay",
        RuleMatcher::any().with_host(host),
        RuleAction::Block(BlockAction { status }),
    ))
    .expect("a valid rule")
}

/// A rule matching everything, with `action`.
fn rule(name: &str, action: RuleAction) -> rules::Rule {
    rules::Rule::create(NewRule::new(name, RuleMatcher::any(), action)).expect("a valid rule")
}

/// Keeps the timeout constant in use, so the harness's own import is not
/// dead when a test is removed.
#[test]
fn the_test_timeout_is_a_few_seconds() {
    assert!(TEST_TIMEOUT.as_secs() >= 1);
}
