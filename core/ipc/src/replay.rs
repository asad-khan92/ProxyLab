//! Sending a request the UI asked for, through the proxy the UI is watching.
//!
//! ```text
//! IPC client ──▶ replay_request ──▶ TrafficService ──▶ RequestReplayer
//!                                                            │
//!                                                            ▼
//!                                                     the proxy engine
//! ```
//!
//! This module is the *boundary*, not the implementation. A replay has to go
//! through the rule engine, ProxyLab's TLS, the capture, the storage, and the
//! traffic event stream — all of which live in `proxy-engine`, which depends
//! on this crate and cannot be depended on by it. So the reach is declared
//! here as a trait and implemented there, the same way
//! [`ExchangeSink`](proxy_storage::ExchangeSink) is declared in
//! `proxy-storage` and implemented by the storage worker.
//!
//! The effect is that this crate knows a replay can be asked for and knows
//! nothing whatever about how one is carried out: there is no socket, no TLS,
//! and no HTTP here, and a server started without a replayer simply says it
//! is not serving replays.
//!
//! # A replay is traffic
//!
//! [`ReplayOutcome`] is deliberately thin — an id, a status, and the error
//! that explains a missing status. Everything else about the replay is in the
//! exchange the proxy captured for it, which arrives on the traffic event
//! stream and is in the traffic list like any other. So there is no second
//! shape for a replayed request, no second store, and nothing for a client to
//! reconcile: the id in this outcome is the id of a row it already has.
//!
//! # Failure is not an error
//!
//! A replay that could not reach the origin does not fail this operation. It
//! produces an exchange whose response is missing and whose
//! [`TrafficError`] says why — exactly what a request the proxy carried for a
//! browser produces when the origin is unreachable. The request was replayed;
//! what came back was a failure, and a failure is a result.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use proxy_http::RequestSpec;
use proxy_storage::{ExchangeId, TrafficError};
use serde::{Deserialize, Serialize};

/// What a replay produced.
///
/// A future, because sending a request is one. Boxed because the trait is
/// used behind `dyn`: the IPC server holds one replayer of a type it does not
/// know, which is the whole point of the trait.
pub type ReplayFuture<'a> = Pin<Box<dyn Future<Output = ReplayOutcome> + Send + 'a>>;

/// Something that can send a request through ProxyLab's own pipeline.
///
/// Implemented in `proxy-engine`. Whoever implements it must carry the
/// request the way the proxy carries a request from a client — rules, TLS,
/// capture, storage, events — because a replay that skipped any of those
/// would be a request the proxy did not really make.
pub trait RequestReplayer: Send + Sync + fmt::Debug {
    /// Sends `request`, and reports what it came to.
    ///
    /// Never fails: a request that could not be sent is an exchange with an
    /// error in it, which is what a request the proxy could not carry has
    /// always been.
    fn replay(&self, request: RequestSpec) -> ReplayFuture<'_>;
}

/// The result of one replay, as a client is told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayOutcome {
    /// The exchange the replay was captured as.
    ///
    /// Always a new one. The exchange a replay was started from — if it was
    /// started from one — is not touched, read back, or amended.
    pub exchange_id: ExchangeId,
    /// The status the destination answered with, if it answered.
    pub status: Option<u16>,
    /// Why there is no status, when there is none.
    pub error: Option<TrafficError>,
}

impl ReplayOutcome {
    /// A replay that got an answer.
    pub fn answered(exchange_id: ExchangeId, status: u16) -> Self {
        Self {
            exchange_id,
            status: Some(status),
            error: None,
        }
    }

    /// A replay that did not.
    pub fn failed(exchange_id: ExchangeId, error: TrafficError) -> Self {
        Self {
            exchange_id,
            status: None,
            error: Some(error),
        }
    }

    /// Whether the destination answered.
    pub fn succeeded(&self) -> bool {
        self.status.is_some() && self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_storage::TrafficErrorKind;

    fn id() -> ExchangeId {
        "0199c0de-0000-7000-8000-00000000002a"
            .parse()
            .expect("a valid exchange id")
    }

    #[test]
    fn an_answered_replay_carries_its_status() {
        let outcome = ReplayOutcome::answered(id(), 201);

        assert!(outcome.succeeded());
        assert_eq!(outcome.status, Some(201));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn a_failed_replay_carries_why() {
        let outcome =
            ReplayOutcome::failed(id(), TrafficError::of(TrafficErrorKind::UpstreamConnect));

        assert!(!outcome.succeeded());
        assert_eq!(outcome.status, None);
        assert_eq!(
            outcome.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::UpstreamConnect)
        );
    }

    #[test]
    fn an_outcome_names_the_parts_a_client_would_look_for() {
        let json = serde_json::to_value(ReplayOutcome::answered(id(), 200)).expect("serializes");

        assert_eq!(json["exchange_id"], "0199c0de-0000-7000-8000-00000000002a");
        assert_eq!(json["status"], 200);
        assert_eq!(json["error"], serde_json::Value::Null);
    }

    #[test]
    fn an_outcome_round_trips_through_json() {
        let outcome = ReplayOutcome::failed(
            id(),
            TrafficError::new(TrafficErrorKind::UpstreamTls, "the TLS handshake failed"),
        );

        let json = serde_json::to_string(&outcome).expect("serializes");
        let back: ReplayOutcome = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, outcome);
    }
}
