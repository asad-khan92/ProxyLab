//! Structured diagnostics for the ProxyLab core.
//!
//! ```text
//! event ──▶ sanitize ──▶ print
//!                  └──▶ recent buffer   (warnings and errors only)
//! ```
//!
//! The buffer is process memory. It is bounded, it keeps the newest
//! entries, and it disappears when the process does. Nothing here writes a
//! file, opens a socket, or reads captured traffic.
//!
//! # What is logged
//!
//! Lifecycle and failures: a listener that did not bind, a client that
//! disconnected, a database that did not open, an origin that refused the
//! connection. Not the traffic itself. A message is sanitized before it is
//! printed or kept, so a URL's query, a cookie, a bearer token, a private
//! key, or a home-directory path does not survive into the line.
//!
//! # Levels
//!
//! `error`, `warn`, `info`, and `debug`. The default is `info`. Debug is
//! off unless `PROXYLAB_LOG` names a level, and nothing on the path that
//! copies a body calls into here.
//!
//! The buffer is installed by the proxy binary. A test, or a failure that
//! happens before that, still prints; it is not kept, because there is
//! nowhere waiting to show it.

mod buffer;
mod sanitize;

use std::io::{self, Write};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub use buffer::{Buffer, Record, LIMIT};
pub use sanitize::{endpoint, exchange_id, sanitize};

/// How serious a diagnostic is.
///
/// Ordered so that a higher threshold means a quieter process: `error` is
/// the most serious and the smallest value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The operation failed.
    Error,
    /// The operation continued, and something about it needs a person.
    Warn,
    /// A normal lifecycle change.
    Info,
    /// Detail that is off unless explicitly asked for.
    Debug,
}

impl Level {
    /// The name used in a log line and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" | "trace" => Some(Self::Debug),
            _ => None,
        }
    }
}

/// Optional context that is only attached when it is safe to attach.
#[derive(Clone, Debug, Default)]
pub struct Detail {
    /// A host and port.
    pub endpoint: Option<String>,
    /// An exchange id.
    pub exchange_id: Option<String>,
    /// How long the operation took, in milliseconds.
    pub duration_ms: Option<u64>,
}

impl Detail {
    /// No extra context.
    pub fn none() -> Self {
        Self::default()
    }

    /// `endpoint`, when it is a host and a port.
    pub fn endpoint(endpoint: &str) -> Self {
        Self {
            endpoint: sanitize::endpoint(endpoint),
            ..Self::default()
        }
    }

    /// The same detail, also naming an exchange.
    pub fn with_exchange(mut self, id: &str) -> Self {
        self.exchange_id = sanitize::exchange_id(id);
        self
    }
}

/// Starts keeping warnings and errors for the diagnostics report.
///
/// Called once, from the proxy binary. A second call returns the buffer
/// the first call installed.
pub fn install() -> std::sync::Arc<Buffer> {
    std::sync::Arc::clone(installed())
}

/// The buffer [`install`] created, if it has been called.
pub fn installed_buffer() -> Option<std::sync::Arc<Buffer>> {
    SINK.get().map(std::sync::Arc::clone)
}

/// Records an error and prints it.
pub fn error(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
) {
    emit(
        Level::Error,
        component,
        operation,
        category,
        message,
        Detail::none(),
    );
}

/// Records a warning and prints it.
pub fn warn(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
) {
    emit(
        Level::Warn,
        component,
        operation,
        category,
        message,
        Detail::none(),
    );
}

/// Prints a lifecycle line. It is not kept in the recent-error buffer.
pub fn info(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
) {
    emit(
        Level::Info,
        component,
        operation,
        category,
        message,
        Detail::none(),
    );
}

/// Prints a debug line when debug logging is on. It is not kept.
pub fn debug(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
) {
    emit(
        Level::Debug,
        component,
        operation,
        category,
        message,
        Detail::none(),
    );
}

/// [`error`] with a host, an exchange, or a duration.
pub fn error_with(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
    detail: Detail,
) {
    emit(
        Level::Error,
        component,
        operation,
        category,
        message,
        detail,
    );
}

/// [`warn`] with a host, an exchange, or a duration.
pub fn warn_with(
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
    detail: Detail,
) {
    emit(Level::Warn, component, operation, category, message, detail);
}

fn emit(
    level: Level,
    component: &'static str,
    operation: &'static str,
    category: &'static str,
    message: &str,
    detail: Detail,
) {
    if level > threshold() {
        return;
    }

    let record = Record {
        level,
        component,
        operation,
        category,
        message: sanitize(message),
        endpoint: detail.endpoint.as_deref().and_then(endpoint),
        exchange_id: detail.exchange_id.as_deref().and_then(exchange_id),
        duration_ms: detail.duration_ms,
        at_ms: unix_ms(),
    };

    print_record(&record);

    if level <= Level::Warn {
        if let Some(buffer) = installed_buffer() {
            buffer.push(record);
        }
    }
}

fn print_record(record: &Record) {
    let mut line = format!(
        "{} {} {} {}: {}",
        record.level.as_str(),
        record.component,
        record.operation,
        record.category,
        record.message
    );
    if let Some(endpoint) = &record.endpoint {
        line.push_str(" endpoint=");
        line.push_str(endpoint);
    }
    if let Some(id) = &record.exchange_id {
        line.push_str(" exchange=");
        line.push_str(id);
    }
    if let Some(duration) = record.duration_ms {
        line.push_str(" duration_ms=");
        line.push_str(&duration.to_string());
    }

    if record.level <= Level::Warn {
        let _ = writeln!(io::stderr(), "{line}");
    } else {
        let _ = writeln!(io::stdout(), "{line}");
    }
}

fn threshold() -> Level {
    *THRESHOLD.get_or_init(|| {
        std::env::var("PROXYLAB_LOG")
            .ok()
            .as_deref()
            .and_then(Level::parse)
            .unwrap_or(Level::Info)
    })
}

fn installed() -> &'static std::sync::Arc<Buffer> {
    SINK.get_or_init(|| std::sync::Arc::new(Buffer::new()))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

static SINK: OnceLock<std::sync::Arc<Buffer>> = OnceLock::new();
static THRESHOLD: OnceLock<Level> = OnceLock::new();

/// Held so a test can prove the level parser without reading the environment.
pub fn level_from(text: &str) -> Option<Level> {
    Level::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_names_parse() {
        assert_eq!(level_from("info"), Some(Level::Info));
        assert_eq!(level_from("WARN"), Some(Level::Warn));
        assert_eq!(level_from("debug"), Some(Level::Debug));
        assert_eq!(level_from("trace"), Some(Level::Debug));
        assert_eq!(level_from("loud"), None);
    }

    #[test]
    fn an_error_with_a_secret_is_stored_without_it() {
        let buffer = install();

        error(
            "upstream",
            "connect",
            "connection",
            "failed https://alice:hunter2@example.com/a?token=secret",
        );

        let stored = buffer
            .recent()
            .into_iter()
            .find(|record| record.component == "upstream" && record.operation == "connect")
            .expect("the error is kept");
        assert_eq!(stored.level, Level::Error);
        assert!(!stored.message.contains("hunter2"), "{}", stored.message);
        assert!(!stored.message.contains("token"), "{}", stored.message);
        assert!(!stored.message.contains("secret"), "{}", stored.message);
    }

    #[test]
    fn a_detail_endpoint_that_is_a_url_is_dropped() {
        let buffer = install();
        error_with(
            "tls",
            "intercept",
            "certificate",
            "the handshake failed",
            Detail::endpoint("https://user:pw@example.com/path?x=1"),
        );

        let stored = buffer
            .recent()
            .into_iter()
            .find(|record| record.component == "tls" && record.operation == "intercept")
            .expect("the error is kept");
        assert_eq!(stored.message, "the handshake failed");
        assert_eq!(stored.endpoint, None);
    }
}
