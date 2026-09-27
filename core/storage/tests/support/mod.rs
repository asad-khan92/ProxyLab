//! Harness for the persistence tests.
//!
//! Every test opens a store in a temporary directory of its own, which is
//! removed when the test ends. Nothing here writes to the repository, to the
//! developer's own traffic database, or to any fixed path, and nothing
//! reaches the network.

#![allow(dead_code)]

use std::net::SocketAddr;

use proxy_storage::{
    BodyCapture, CapturedRequest, CapturedResponse, ConnectionInfo, Destination, ExchangeId,
    HeaderValue, HttpVersion, Storage, StorageConfig, Timestamp, Timing, TrafficError,
    TrafficErrorKind, TrafficExchange,
};
use tempfile::TempDir;

/// A store in a directory of its own, and the directory it lives in.
///
/// The directory is held so that it outlives the store: dropping it removes
/// the database, the write-ahead log, and every body file with it.
pub struct TestStore {
    pub storage: Storage,
    pub directory: TempDir,
}

impl TestStore {
    /// A store with the default body-inlining limit.
    pub fn open() -> Self {
        Self::with_inline_limit(proxy_storage::DEFAULT_INLINE_BODY_MAX_BYTES)
    }

    /// A store that keeps captured bodies up to `inline` bytes in the
    /// database and puts anything larger in a file.
    pub fn with_inline_limit(inline: usize) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let storage = Storage::open(
            StorageConfig::in_directory(directory.path()).with_inline_body_max_bytes(inline),
        )
        .expect("a store opens");

        Self { storage, directory }
    }

    /// Where this store keeps things.
    pub fn config(&self) -> StorageConfig {
        self.storage.config().clone()
    }

    /// Another handle to the same database and body directory.
    pub fn reopen(&self) -> Storage {
        Storage::open(self.config()).expect("the store reopens")
    }

    /// Runs one statement against the database directly.
    ///
    /// For the tests that have to put something in a row that the storage
    /// API would never write — a rule whose action a later ProxyLab
    /// introduced, or JSON somebody edited by hand — so that reading it back
    /// can be tested at all.
    pub fn execute(&self, sql: &str, parameter: &str) {
        let connection =
            rusqlite::Connection::open(&self.config().database).expect("the database opens");

        connection
            .execute(sql, [parameter])
            .expect("the statement runs");
    }

    /// The names of the body files on disk, sorted.
    pub fn body_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.config().bodies)
            .expect("a readable body directory")
            .map(|entry| {
                entry
                    .expect("a readable entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();

        names
    }
}

/// The address every test's client connects from.
pub fn client() -> SocketAddr {
    "127.0.0.1:54321".parse().expect("a valid address")
}

/// A finished plaintext exchange to `path` on `host`, answered with `status`.
pub fn http_exchange(host: &str, path: &str, status: u16) -> TrafficExchange {
    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo::http(client(), Some(Destination::new(host, 80))),
        CapturedRequest::new("GET", format!("http://{host}{path}"), HttpVersion::Http11),
    );
    exchange.response = Some(CapturedResponse::new(status, HttpVersion::Http11));
    exchange.timing = timing(Timestamp::now());

    exchange
}

/// A finished exchange read out of an intercepted TLS session.
pub fn https_exchange(host: &str, path: &str, status: u16) -> TrafficExchange {
    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo::https(client(), Some(Destination::new(host, 443))),
        CapturedRequest::new("GET", format!("https://{host}{path}"), HttpVersion::Http11),
    );
    exchange.response = Some(CapturedResponse::new(status, HttpVersion::Http11));
    exchange.timing = timing(Timestamp::now());

    exchange
}

/// An exchange that never got a response, and says why.
pub fn failed_exchange(host: &str, kind: TrafficErrorKind) -> TrafficExchange {
    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo::http(client(), Some(Destination::new(host, 80))),
        CapturedRequest::new("GET", format!("http://{host}/x"), HttpVersion::Http11),
    );
    exchange.error = Some(TrafficError::of(kind));
    exchange.timing = timing(Timestamp::now());

    exchange
}

/// An exchange with neither a response nor an error: a recording that ended
/// without either.
pub fn unanswered_exchange(host: &str) -> TrafficExchange {
    let mut exchange = TrafficExchange::new(
        ExchangeId::next(),
        ConnectionInfo::http(client(), Some(Destination::new(host, 80))),
        CapturedRequest::new("POST", format!("http://{host}/x"), HttpVersion::Http11),
    );
    exchange.timing = Timing {
        started_at: Timestamp::now(),
        upstream_connect_micros: Some(120),
        upstream_response_micros: None,
        completed_micros: None,
    };

    exchange
}

/// Timing for an exchange that started at `started_at` and finished.
pub fn timing(started_at: Timestamp) -> Timing {
    Timing {
        started_at,
        upstream_connect_micros: Some(120),
        upstream_response_micros: Some(2_000),
        completed_micros: Some(3_500),
    }
}

/// The same exchange, started at `moment`.
pub fn started_at(mut exchange: TrafficExchange, moment: Timestamp) -> TrafficExchange {
    exchange.timing = timing(moment);

    exchange
}

/// The same exchange, with `method` as its request method.
pub fn with_method(mut exchange: TrafficExchange, method: &str) -> TrafficExchange {
    exchange.request.method = method.to_owned();

    exchange
}

/// The same exchange, with `request` and `response` as its captured bodies.
pub fn with_bodies(
    mut exchange: TrafficExchange,
    request: BodyCapture,
    response: BodyCapture,
) -> TrafficExchange {
    exchange.request.body = request;
    if let Some(captured) = exchange.response.as_mut() {
        captured.body = response;
    }

    exchange
}

/// Headers a test can recognise, on both halves of `exchange`.
///
/// Deliberately awkward ones: a name that appears twice, a value that is not
/// text, and the credentials a debugging proxy is expected to keep.
pub fn with_headers(mut exchange: TrafficExchange) -> TrafficExchange {
    exchange
        .request
        .headers
        .push("Accept", HeaderValue::Text("application/json".into()));
    exchange.request.headers.push(
        "Authorization",
        HeaderValue::Text("Bearer sk-secret".into()),
    );
    exchange
        .request
        .headers
        .push("X-Repeated", HeaderValue::Text("first".into()));
    exchange
        .request
        .headers
        .push("X-Repeated", HeaderValue::Text("second".into()));
    exchange
        .request
        .headers
        .push("X-Raw", HeaderValue::from_bytes(&[0x00, 0x80, 0xff]));

    if let Some(response) = exchange.response.as_mut() {
        response
            .headers
            .push("Content-Type", HeaderValue::Text("application/json".into()));
        response
            .headers
            .push("Set-Cookie", HeaderValue::Text("session=abc".into()));
        response
            .headers
            .push("Set-Cookie", HeaderValue::Text("theme=dark".into()));
    }

    exchange
}

/// A timestamp `seconds` after the epoch.
pub fn at(seconds: u64) -> Timestamp {
    Timestamp { seconds, nanos: 0 }
}

/// Bytes that are not text, covering every byte value.
pub fn binary(length: usize) -> Vec<u8> {
    (0..=255).cycle().take(length).collect()
}
