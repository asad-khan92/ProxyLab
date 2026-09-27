//! Letting the UI put the proxy on the local network, on purpose.
//!
//! ```text
//! IPC client ──▶ enable_lan ──▶ TrafficService ──▶ LanControl
//!                                                      │
//!                                                      ▼
//!                                            a second proxy listener,
//!                                            on one chosen LAN address
//! ```
//!
//! This module is the *boundary*, not the implementation, for the same
//! reason [`replay`](crate::replay) is: binding a listener needs the proxy
//! engine, which depends on this crate and cannot be depended on by it. So
//! the reach is declared here as a trait and implemented in `proxy-engine`.
//!
//! # ProxyLab listens on loopback, and that is the default
//!
//! The proxy's own listener is `127.0.0.1` and never moves. Enabling LAN
//! exposure adds a second listener on one address the developer picked, over
//! the same pipeline; disabling it closes that listener again. Nothing here
//! can change where the loopback listener is, and nothing turns this on by
//! itself — there is no setting that persists it, no environment variable,
//! and no "remember this": a ProxyLab that has just started is a ProxyLab
//! only this Mac can reach.
//!
//! # It is a deliberate widening of a security boundary
//!
//! A LAN listener is an open HTTP proxy for everything on the same Wi-Fi.
//! There is no authentication, because [the protocol has none to
//! borrow](crate#it-is-a-security-boundary) — the boundary ProxyLab has is a
//! Unix socket's file permissions, and that boundary does not stretch over a
//! network. So the address is chosen by a person, one address at a time,
//! rather than `0.0.0.0`; it is never a publicly routable address; and the
//! status a client is given says plainly that it is on, so an interface can
//! say so too.
//!
//! # Nothing about the network is kept
//!
//! [`LanStatus`] is answered from the machine as it is at that moment: the
//! addresses are read from the interfaces when asked, and what is enabled is
//! held in memory by the running proxy. No address is written to the
//! database, to a preferences file, or to the log.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::protocol::{ErrorBody, ErrorCode};

/// What a LAN operation produced.
///
/// A future, because binding and closing a listener are. Boxed because the
/// trait is used behind `dyn`: the IPC server holds one controller of a type
/// it does not know, which is the whole point of the trait.
pub type LanFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Something that can put the proxy on the local network, and take it off.
///
/// Implemented in `proxy-engine`. Whoever implements it must carry LAN
/// traffic through the pipeline the proxy already runs — rules, TLS, capture,
/// storage, events — because a second listener that did anything else would
/// be a second proxy.
pub trait LanControl: Send + Sync + fmt::Debug {
    /// Whether LAN exposure is on, and which addresses it could use.
    fn status(&self) -> LanFuture<'_, LanStatus>;

    /// Starts listening on `address` as well as on loopback.
    ///
    /// `address` is the text a client sent. Whether it is an address at all,
    /// and whether it is one of this Mac's own, is decided by the
    /// implementation and not by the client.
    fn enable(&self, address: String) -> LanFuture<'_, Result<LanStatus, LanRefusal>>;

    /// Stops listening on the LAN address, leaving loopback alone.
    ///
    /// Resolves once the listening socket is closed, so a client that has
    /// been told LAN exposure is off can rely on it being off.
    fn disable(&self) -> LanFuture<'_, LanStatus>;
}

/// Where the proxy can be reached, and where it could be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanStatus {
    /// Whether the proxy is listening on a LAN address as well as loopback.
    pub enabled: bool,
    /// The LAN address it is listening on, when it is.
    pub address: Option<String>,
    /// The port the proxy listens on, on loopback and on the LAN alike.
    ///
    /// Always present, because it is what an iPhone's proxy settings need
    /// beside the address, and it does not depend on LAN exposure being on.
    pub port: u16,
    /// The addresses of this Mac a LAN listener could be bound to, as they
    /// are right now.
    pub available: Vec<LanAddress>,
}

/// One address of this Mac that a LAN listener could use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanAddress {
    /// The interface it belongs to, as the operating system names it.
    pub interface: String,
    /// The address itself.
    pub address: String,
}

/// Why LAN exposure was not turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanRefusal {
    /// What was sent is not an address this proxy can bind.
    NotAnAddress,
    /// It is an address, but not one of this Mac's usable LAN addresses.
    ///
    /// Loopback, a link-local address, an interface that is down, and
    /// anything routable from outside the local network are all this: the
    /// choice is made from [`LanStatus::available`], and nothing else is
    /// accepted.
    NotAvailable,
    /// The operating system would not give the proxy that address and port.
    BindFailed,
}

impl LanRefusal {
    /// The error a client is told, which is a phrase chosen here.
    pub fn error(self) -> ErrorBody {
        match self {
            Self::NotAnAddress => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "a LAN address must be an IP address of this machine",
            ),
            Self::NotAvailable => ErrorBody::new(
                ErrorCode::LanUnavailable,
                "that address is not one of this machine's usable local network addresses",
            ),
            Self::BindFailed => ErrorBody::new(
                ErrorCode::LanUnavailable,
                "the proxy could not listen on that address and port",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> LanStatus {
        LanStatus {
            enabled: true,
            address: Some("192.168.1.42".to_owned()),
            port: 9090,
            available: vec![LanAddress {
                interface: "en0".to_owned(),
                address: "192.168.1.42".to_owned(),
            }],
        }
    }

    #[test]
    fn a_status_names_the_address_the_port_and_the_choices() {
        let json = serde_json::to_value(status()).expect("serializes");

        assert_eq!(json["enabled"], true);
        assert_eq!(json["address"], "192.168.1.42");
        assert_eq!(json["port"], 9090);
        assert_eq!(json["available"][0]["interface"], "en0");
        assert_eq!(json["available"][0]["address"], "192.168.1.42");
    }

    #[test]
    fn a_disabled_status_still_names_the_port() {
        let off = LanStatus {
            enabled: false,
            address: None,
            port: 9090,
            ..status()
        };
        let json = serde_json::to_value(off).expect("serializes");

        assert_eq!(json["enabled"], false);
        assert_eq!(json["address"], serde_json::Value::Null);
        assert_eq!(json["port"], 9090);
    }

    #[test]
    fn a_status_round_trips_through_json() {
        let json = serde_json::to_string(&status()).expect("serializes");
        let back: LanStatus = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, status());
    }

    #[test]
    fn an_address_that_is_not_one_is_a_parameter_problem() {
        assert_eq!(
            LanRefusal::NotAnAddress.error().code,
            ErrorCode::InvalidParameters
        );
    }

    #[test]
    fn an_address_this_machine_does_not_have_is_a_lan_problem() {
        assert_eq!(
            LanRefusal::NotAvailable.error().code,
            ErrorCode::LanUnavailable
        );
        assert_eq!(
            LanRefusal::BindFailed.error().code,
            ErrorCode::LanUnavailable
        );
    }

    #[test]
    fn a_refusal_never_repeats_what_the_client_sent() {
        for refusal in [
            LanRefusal::NotAnAddress,
            LanRefusal::NotAvailable,
            LanRefusal::BindFailed,
        ] {
            assert!(!refusal.error().message.contains("192.168"));
        }
    }
}
