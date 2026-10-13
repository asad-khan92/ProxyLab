//! The core's own status, as a client may ask for it.
//!
//! ```text
//! diagnostics ──▶ listener facts, recorded when they were bound
//!             ──▶ LanControl::status, asked at this moment
//!             ──▶ the recent-error buffer
//! ```
//!
//! Nothing here is a second copy of that state. The listener facts are the
//! outcome of the binds the process already did, LAN exposure is read from
//! the same control `lan_status` uses, and storage is open exactly when this
//! server is running — the process does not serve IPC without it.
//!
//! The answer is safe to paste into an issue. Messages are sanitized again
//! on the way out, so a path, a query string, or a credential that reached
//! the buffer does not reach the client.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::lan::LanStatus;

/// What a client is told about one listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointStatus {
    /// `listening`, `unavailable`, or `unknown`.
    pub state: String,
    /// The address it is bound to, when it is listening.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Why it is not listening, when that is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl EndpointStatus {
    /// A listener whose address this server was not told.
    pub fn unknown() -> Self {
        Self {
            state: UNKNOWN.to_owned(),
            address: None,
            message: None,
        }
    }

    /// A listener bound at `address`.
    pub fn listening(address: String) -> Self {
        Self {
            state: LISTENING.to_owned(),
            address: Some(address),
            message: None,
        }
    }

    /// A listener that could not be bound, for `message`.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            state: UNAVAILABLE.to_owned(),
            address: None,
            message: Some(message.into()),
        }
    }
}

/// What a client is told about one subsystem that is not a listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentStatus {
    /// `open`, `serving`, `unavailable`, or `unknown`.
    pub state: String,
    /// Why, when there is a reason a person can act on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ComponentStatus {
    /// The subsystem is doing its job.
    pub fn ok(state: &str) -> Self {
        Self {
            state: state.to_owned(),
            message: None,
        }
    }

    /// The subsystem is not, for `message`.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            state: UNAVAILABLE.to_owned(),
            message: Some(message.into()),
        }
    }
}

/// One recent warning or error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    /// `warn` or `error`.
    pub level: String,
    /// Which part of ProxyLab it came from.
    pub component: String,
    /// What that part was doing.
    pub operation: String,
    /// What kind of failure it was.
    pub category: String,
    /// The sentence, with secrets removed.
    pub message: String,
    /// A host and port, when the failure was about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The exchange it belonged to, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_id: Option<String>,
    /// How long the operation had been running, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// When it happened, as milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// The facts the proxy recorded about itself, before LAN exposure is read.
///
/// LAN is not in here. It changes while the process runs, and
/// [`crate::LanControl::status`] is the one place that knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsFacts {
    /// The core's version.
    pub version: String,
    /// The loopback proxy listener.
    pub localhost: EndpointStatus,
    /// The transparent-capture bridge.
    pub transparent: EndpointStatus,
    /// Whether captured traffic is being stored.
    pub storage: ComponentStatus,
    /// Recent warnings and errors, newest first.
    pub recent: Vec<DiagnosticEvent>,
}

impl DiagnosticsFacts {
    /// What is known merely from the fact that this server answered.
    ///
    /// Storage is open — IPC is not served without it — and the listeners
    /// are unknown until the process that bound them says where.
    pub fn serving(version: &str) -> Self {
        Self {
            version: version.to_owned(),
            localhost: EndpointStatus::unknown(),
            transparent: EndpointStatus::unknown(),
            storage: ComponentStatus::ok(OPEN),
            recent: Vec::new(),
        }
    }
}

/// The whole answer to `diagnostics`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    /// The core's version.
    pub version: String,
    /// `running` when this answer could be produced.
    pub core: String,
    /// The loopback proxy listener.
    pub localhost: EndpointStatus,
    /// The transparent-capture bridge.
    pub transparent: EndpointStatus,
    /// LAN exposure, from [`crate::LanControl`], or absent when this server
    /// is not managing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lan: Option<LanStatus>,
    /// Whether captured traffic is being stored.
    pub storage: ComponentStatus,
    /// Whether this socket is being served. Answering is what `serving` means.
    pub ipc: ComponentStatus,
    /// Recent warnings and errors, newest first.
    pub recent: Vec<DiagnosticEvent>,
}

/// Something that can describe the proxy process.
///
/// Implemented by the proxy binary, which is where the listeners were bound.
/// This crate never binds one.
pub trait DiagnosticsControl: Send + Sync + fmt::Debug {
    /// The facts as they are at this moment.
    fn facts(&self) -> DiagnosticsFacts;
}

/// A listener that is bound.
pub const LISTENING: &str = "listening";
/// A subsystem that could not do its job.
pub const UNAVAILABLE: &str = "unavailable";
/// This server was not told.
pub const UNKNOWN: &str = "unknown";
/// Captured traffic is being stored.
pub const OPEN: &str = "open";
/// The IPC socket is being served.
pub const SERVING: &str = "serving";
/// The proxy process is running.
pub const RUNNING: &str = "running";

/// How many recent events an answer will carry.
const MAX_RECENT: usize = proxy_diagnostics::LIMIT;

/// Builds the answer from `facts` and the LAN status just read.
pub fn report(mut facts: DiagnosticsFacts, lan: Option<LanStatus>) -> DiagnosticsReport {
    facts.recent.truncate(MAX_RECENT);

    DiagnosticsReport {
        version: scrub_text(&facts.version),
        core: RUNNING.to_owned(),
        localhost: scrub_endpoint(facts.localhost),
        transparent: scrub_endpoint(facts.transparent),
        lan,
        storage: scrub_component(facts.storage),
        ipc: ComponentStatus::ok(SERVING),
        recent: facts.recent.into_iter().map(scrub_event).collect(),
    }
}

fn scrub_endpoint(status: EndpointStatus) -> EndpointStatus {
    EndpointStatus {
        state: status.state,
        address: status
            .address
            .and_then(|address| proxy_diagnostics::endpoint(&address)),
        message: status.message.map(|message| scrub_text(&message)),
    }
}

fn scrub_component(status: ComponentStatus) -> ComponentStatus {
    ComponentStatus {
        state: status.state,
        message: status.message.map(|message| scrub_text(&message)),
    }
}

fn scrub_event(event: DiagnosticEvent) -> DiagnosticEvent {
    DiagnosticEvent {
        level: event.level,
        component: scrub_label(&event.component),
        operation: scrub_label(&event.operation),
        category: scrub_label(&event.category),
        message: scrub_text(&event.message),
        endpoint: event
            .endpoint
            .as_deref()
            .and_then(proxy_diagnostics::endpoint),
        exchange_id: event
            .exchange_id
            .as_deref()
            .and_then(proxy_diagnostics::exchange_id),
        duration_ms: event.duration_ms,
        at_ms: event.at_ms,
    }
}

fn scrub_text(text: &str) -> String {
    proxy_diagnostics::sanitize(text)
}

/// A component name is a short label. Anything longer, or anything with a
/// space, is not one we wrote and is not forwarded.
fn scrub_label(text: &str) -> String {
    if text.len() <= 32
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        text.to_owned()
    } else {
        "unknown".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_drops_a_secret_that_reached_the_buffer() {
        let facts = DiagnosticsFacts {
            version: "0.1.0".to_owned(),
            localhost: EndpointStatus::listening("127.0.0.1:9090".to_owned()),
            transparent: EndpointStatus::unavailable(
                "could not bind /Users/ada/.proxylab/not-a-port",
            ),
            storage: ComponentStatus::ok(OPEN),
            recent: vec![DiagnosticEvent {
                level: "error".to_owned(),
                component: "upstream".to_owned(),
                operation: "connect".to_owned(),
                category: "connection".to_owned(),
                message: "failed https://alice:hunter2@example.com/a?token=secret".to_owned(),
                endpoint: Some("https://alice:pw@example.com/a?x=1".to_owned()),
                exchange_id: Some("not an id".to_owned()),
                duration_ms: Some(12),
                at_ms: 1,
            }],
        };

        let report = report(facts, None);
        let json = serde_json::to_string(&report).expect("serializes");

        assert!(!json.contains("hunter2"), "{json}");
        assert!(!json.contains("token"), "{json}");
        assert!(!json.contains("secret"), "{json}");
        assert!(!json.contains("ada"), "{json}");
        assert!(!json.contains("alice"), "{json}");
        assert_eq!(report.localhost.address.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(report.recent[0].endpoint, None);
        assert_eq!(report.recent[0].exchange_id, None);
        assert_eq!(report.recent[0].duration_ms, Some(12));
        assert_eq!(report.ipc.state, SERVING);
        assert_eq!(report.core, RUNNING);
    }

    #[test]
    fn a_label_that_is_not_one_is_not_forwarded() {
        assert_eq!(scrub_label("upstream"), "upstream");
        assert_eq!(scrub_label("Authorization: Bearer abc"), "unknown");
    }
}
