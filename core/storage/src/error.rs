//! Why an exchange has no response.
//!
//! Not every request produces one. The origin may be unreachable, its
//! certificate may not verify, the client may go away in the middle of the
//! answer. None of those is an HTTP status: inventing a `502` for a captured
//! exchange would tell a developer the origin answered when it did not. A
//! failed exchange has `response = None` and an error instead.
//!
//! # What an error may say
//!
//! A curated phrase, and nothing else. [`TrafficError::new`] takes a
//! `&'static str`, so the summary of a failure cannot be a formatted internal
//! error: there is no way to pass one in. That is deliberate. Underlying
//! errors name file paths, TLS internals, and whatever the peer sent, and a
//! captured exchange is a thing that gets shown, exported, and stored.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What kind of failure ended an exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficErrorKind {
    /// The proxy would not carry the request at all.
    ///
    /// The request itself was the problem: a target that named no host, a
    /// protocol version this proxy does not speak.
    RequestRejected,
    /// The proxy could not carry out a request that was otherwise fine.
    ///
    /// The proxy's own failure rather than the client's or the origin's —
    /// a certificate it could not issue, for instance.
    ProxyFailure,
    /// The connection to the origin could not be opened.
    ///
    /// Covers a name that would not resolve as well as a TCP connection that
    /// was refused, timed out, or had nowhere to go: the proxy asks the
    /// operating system to do both in one step and is told only that it
    /// failed.
    UpstreamConnect,
    /// The TLS handshake with the origin failed.
    UpstreamTls,
    /// The TLS handshake with the client failed.
    ///
    /// Most often the client refusing ProxyLab's certificate, which is a
    /// client that has not been told to trust the development CA.
    ClientTls,
    /// The origin did not give a usable HTTP response.
    UpstreamHttp,
    /// The response could not be forwarded to the client in full.
    ResponseForwarding,
    /// The client's connection ended before the exchange did.
    ClientDisconnected,
    /// The exchange stopped part-way through for some other reason.
    Incomplete,
}

impl TrafficErrorKind {
    /// The default summary for this kind of failure.
    pub fn describe(self) -> &'static str {
        match self {
            Self::RequestRejected => "the proxy would not carry this request",
            Self::ProxyFailure => "the proxy could not carry out this request",
            Self::UpstreamConnect => "could not connect to the destination server",
            Self::UpstreamTls => "the TLS handshake with the destination server failed",
            Self::ClientTls => "the TLS handshake with the client failed",
            Self::UpstreamHttp => "no valid response from the destination server",
            Self::ResponseForwarding => "the response was not forwarded in full",
            Self::ClientDisconnected => "the client closed the connection",
            Self::Incomplete => "the exchange did not finish",
        }
    }
}

impl fmt::Display for TrafficErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// A failure, in the form it is safe to show, export, and store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficError {
    kind: TrafficErrorKind,
    summary: String,
}

impl TrafficError {
    /// A failure of `kind`, summarised by `summary`.
    ///
    /// `summary` is a `&'static str` on purpose: a summary has to be a phrase
    /// written for this purpose, not the `Display` of whatever went wrong.
    pub fn new(kind: TrafficErrorKind, summary: &'static str) -> Self {
        Self {
            kind,
            summary: summary.to_owned(),
        }
    }

    /// A failure of `kind`, summarised by that kind's own description.
    pub fn of(kind: TrafficErrorKind) -> Self {
        Self::new(kind, kind.describe())
    }

    /// A failure being read back from where it was stored.
    ///
    /// The one constructor that takes a summary which is not a
    /// `&'static str`, and it exists for one reason: an exchange that was
    /// persisted has to come back the way it went in, and the summary it
    /// went in with was a curated phrase written somewhere else in this
    /// process. Reading it back through [`new`](Self::new) is not possible,
    /// and rebuilding it from the kind alone would quietly replace a
    /// specific explanation with a generic one.
    ///
    /// This is not the way to record a failure that is happening now — use
    /// [`new`](Self::new) or [`of`](Self::of), which cannot be handed the
    /// `Display` of an internal error.
    pub fn restored(kind: TrafficErrorKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
        }
    }

    /// What kind of failure it was.
    pub fn kind(&self) -> TrafficErrorKind {
        self.kind
    }

    /// The phrase describing it.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl fmt::Display for TrafficError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_KIND: &[TrafficErrorKind] = &[
        TrafficErrorKind::RequestRejected,
        TrafficErrorKind::ProxyFailure,
        TrafficErrorKind::UpstreamConnect,
        TrafficErrorKind::UpstreamTls,
        TrafficErrorKind::ClientTls,
        TrafficErrorKind::UpstreamHttp,
        TrafficErrorKind::ResponseForwarding,
        TrafficErrorKind::ClientDisconnected,
        TrafficErrorKind::Incomplete,
    ];

    #[test]
    fn every_kind_describes_itself() {
        for kind in EVERY_KIND {
            assert!(!kind.describe().is_empty(), "{kind:?}");
            assert_eq!(kind.to_string(), kind.describe());
        }
    }

    #[test]
    fn a_failure_carries_its_kind_and_its_summary() {
        let error = TrafficError::new(
            TrafficErrorKind::UpstreamTls,
            "the origin's certificate did not verify",
        );

        assert_eq!(error.kind(), TrafficErrorKind::UpstreamTls);
        assert_eq!(error.summary(), "the origin's certificate did not verify");
        assert_eq!(error.to_string(), "the origin's certificate did not verify");
    }

    #[test]
    fn a_failure_can_take_its_kinds_own_description() {
        let error = TrafficError::of(TrafficErrorKind::UpstreamConnect);

        assert_eq!(
            error.summary(),
            "could not connect to the destination server"
        );
    }

    #[test]
    fn a_restored_failure_is_the_failure_that_was_stored() {
        let recorded = TrafficError::new(
            TrafficErrorKind::UpstreamTls,
            "the origin's certificate did not verify",
        );

        let restored = TrafficError::restored(recorded.kind(), recorded.summary());

        assert_eq!(restored, recorded);
    }

    #[test]
    fn every_kind_round_trips_through_json() {
        for kind in EVERY_KIND {
            let error = TrafficError::of(*kind);

            let json = serde_json::to_string(&error).expect("serializes");
            let back: TrafficError = serde_json::from_str(&json).expect("deserializes");

            assert_eq!(back, error);
        }
    }

    #[test]
    fn a_kind_is_named_in_json_rather_than_numbered() {
        let json = serde_json::to_string(&TrafficError::of(TrafficErrorKind::ClientTls))
            .expect("serializes");

        assert!(json.contains(r#""kind":"client_tls""#), "{json}");
    }
}
