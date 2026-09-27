//! Making the proxy's own link as slow as the one the app will ship on.
//!
//! ```text
//!            latency          upload                download
//! client ──▶ waits once ──▶ paced upstream ──▶ … ──▶ paced back
//! ```
//!
//! Three fixed numbers and a switch: a delay added once per exchange, a
//! ceiling on how fast a request body goes upstream, and a ceiling on how
//! fast a response body comes back. Nothing adapts, nothing is scheduled, and
//! there are no presets — a developer who wants a different shape types
//! different numbers.
//!
//! This module is the *boundary*, not the implementation, for the same reason
//! [`breakpoint`](crate::breakpoint) is: pacing a body means holding frames
//! back inside the proxy engine, which depends on this crate and cannot be
//! depended on by it. So the reach is declared here as a trait and
//! implemented in `proxy-engine`.
//!
//! # Nothing is kept
//!
//! Throttling is held in memory by the running proxy and written nowhere. A
//! ProxyLab that has just started carries traffic at the speed of the network
//! it is on, exactly as one that has just started is not on the local network
//! and has no breakpoints. A setting that survived a restart is a proxy that
//! is mysteriously slow a week later.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::protocol::{ErrorBody, ErrorCode};

/// The longest delay that may be added to an exchange, in milliseconds.
///
/// A minute is far longer than any link anyone is simulating, and short
/// enough that a mistyped number cannot hold a request for the rest of the
/// afternoon.
pub const MAX_THROTTLE_LATENCY_MS: u64 = 60_000;

/// The slowest rate that may be asked for, in bytes per second.
///
/// A rate below this is a rate at which nothing finishes: a modest JSON
/// response would take minutes. The floor is not zero because zero already
/// means something else — see [`ThrottleSettings`].
pub const MIN_THROTTLE_BYTES_PER_SECOND: u64 = 256;

/// How the proxy is shaping the traffic it carries.
///
/// A rate of `0` is *no ceiling* rather than *no bytes*: it is how one
/// direction is left alone while the other is shaped, and it is what both are
/// when throttling has never been configured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThrottleSettings {
    /// Whether any of it is being applied.
    ///
    /// Held separately from the numbers so that turning throttling off and on
    /// again does not mean retyping them.
    pub enabled: bool,
    /// How long each exchange waits before its request is sent, in
    /// milliseconds. `0` for no wait.
    #[serde(default)]
    pub latency_ms: u64,
    /// The ceiling on a response body coming back, in bytes per second. `0`
    /// for no ceiling.
    #[serde(default)]
    pub download_bytes_per_second: u64,
    /// The ceiling on a request body going upstream, in bytes per second.
    /// `0` for no ceiling.
    #[serde(default)]
    pub upload_bytes_per_second: u64,
}

impl ThrottleSettings {
    /// Throttling as a proxy that has just started has it: off, and set to
    /// nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether anything about this shapes traffic at all.
    ///
    /// Enabled with every number at zero is not throttling; it is throttling
    /// that has been asked for and has nothing to do.
    pub fn is_shaping(&self) -> bool {
        self.enabled
            && (self.latency_ms > 0
                || self.download_bytes_per_second > 0
                || self.upload_bytes_per_second > 0)
    }

    /// The ceiling on a response body, or `None` for no ceiling.
    pub fn download(&self) -> Option<u64> {
        self.rate(self.download_bytes_per_second)
    }

    /// The ceiling on a request body, or `None` for no ceiling.
    pub fn upload(&self) -> Option<u64> {
        self.rate(self.upload_bytes_per_second)
    }

    /// The delay each exchange waits, or `None` for no delay.
    pub fn latency(&self) -> Option<u64> {
        match self.enabled && self.latency_ms > 0 {
            true => Some(self.latency_ms),
            false => None,
        }
    }

    /// One of the two rates, if throttling is on and the rate is set.
    fn rate(&self, bytes_per_second: u64) -> Option<u64> {
        match self.enabled && bytes_per_second > 0 {
            true => Some(bytes_per_second),
            false => None,
        }
    }

    /// Whether these are settings the proxy can be given.
    pub fn validate(&self) -> Result<(), ThrottleRefusal> {
        if self.latency_ms > MAX_THROTTLE_LATENCY_MS {
            return Err(ThrottleRefusal::LatencyTooLong);
        }
        for rate in [self.download_bytes_per_second, self.upload_bytes_per_second] {
            if rate > 0 && rate < MIN_THROTTLE_BYTES_PER_SECOND {
                return Err(ThrottleRefusal::RateTooSlow);
            }
        }

        Ok(())
    }

    /// The one-line form that is safe to write to a log.
    ///
    /// Numbers a developer typed about their own proxy, and nothing any
    /// exchange carried.
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "off".to_owned();
        }

        format!(
            "{} ms latency, {} down, {} up",
            self.latency_ms,
            rate_summary(self.download_bytes_per_second),
            rate_summary(self.upload_bytes_per_second)
        )
    }
}

/// One rate as a log line names it.
fn rate_summary(bytes_per_second: u64) -> String {
    match bytes_per_second {
        0 => "unlimited".to_owned(),
        rate => format!("{rate} B/s"),
    }
}

/// Why throttling was not set to what was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThrottleRefusal {
    /// The delay is longer than [`MAX_THROTTLE_LATENCY_MS`].
    LatencyTooLong,
    /// A rate is slower than [`MIN_THROTTLE_BYTES_PER_SECOND`].
    RateTooSlow,
}

impl ThrottleRefusal {
    /// The error a client is told, which is a phrase chosen here.
    pub fn error(self) -> ErrorBody {
        match self {
            Self::LatencyTooLong => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "added latency must be at most 60000 milliseconds",
            ),
            Self::RateTooSlow => ErrorBody::new(
                ErrorCode::InvalidParameters,
                "a bandwidth ceiling must be 0 for unlimited, or at least 256 bytes per second",
            ),
        }
    }
}

/// Something that can shape the traffic the proxy carries.
///
/// Implemented in `proxy-engine`. Both methods are synchronous: what is
/// behind them is four numbers in memory, and reading or replacing them waits
/// for nothing.
pub trait ThrottleControl: Send + Sync + fmt::Debug {
    /// How traffic is being shaped right now.
    fn settings(&self) -> ThrottleSettings;

    /// Shapes it this way from the next exchange on, and says how it now is.
    ///
    /// Exchanges already in flight keep the shaping they started with: a
    /// response half delivered does not change speed part-way.
    fn apply(&self, settings: ThrottleSettings) -> ThrottleSettings;
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn shaped() -> ThrottleSettings {
        ThrottleSettings {
            enabled: true,
            latency_ms: 200,
            download_bytes_per_second: 131_072,
            upload_bytes_per_second: 32_768,
        }
    }

    #[test]
    fn a_proxy_that_has_just_started_shapes_nothing() {
        let settings = ThrottleSettings::none();

        assert!(!settings.is_shaping());
        assert_eq!(settings.latency(), None);
        assert_eq!(settings.download(), None);
        assert_eq!(settings.upload(), None);
    }

    #[test]
    fn settings_that_are_set_and_switched_off_shape_nothing() {
        let settings = ThrottleSettings {
            enabled: false,
            ..shaped()
        };

        assert!(!settings.is_shaping());
        assert_eq!(settings.latency(), None);
        assert_eq!(settings.download(), None);
    }

    #[test]
    fn a_rate_of_zero_is_no_ceiling_rather_than_no_bytes() {
        let settings = ThrottleSettings {
            enabled: true,
            latency_ms: 0,
            download_bytes_per_second: 0,
            upload_bytes_per_second: 32_768,
        };

        assert!(settings.is_shaping());
        assert_eq!(settings.download(), None);
        assert_eq!(settings.upload(), Some(32_768));
    }

    #[test]
    fn settings_switched_on_with_nothing_set_are_not_shaping() {
        let settings = ThrottleSettings {
            enabled: true,
            ..ThrottleSettings::none()
        };

        assert!(!settings.is_shaping());
    }

    #[test]
    fn a_delay_longer_than_a_minute_is_refused() {
        let settings = ThrottleSettings {
            latency_ms: MAX_THROTTLE_LATENCY_MS + 1,
            ..shaped()
        };

        assert_eq!(settings.validate(), Err(ThrottleRefusal::LatencyTooLong));
    }

    #[test]
    fn a_rate_nothing_would_finish_at_is_refused() {
        let settings = ThrottleSettings {
            download_bytes_per_second: MIN_THROTTLE_BYTES_PER_SECOND - 1,
            ..shaped()
        };

        assert_eq!(settings.validate(), Err(ThrottleRefusal::RateTooSlow));
        assert_eq!(shaped().validate(), Ok(()));
        assert_eq!(ThrottleSettings::none().validate(), Ok(()));
    }

    #[test]
    fn settings_name_every_number_on_the_wire() {
        let json = serde_json::to_value(shaped()).expect("serializes");

        assert_eq!(
            json,
            json!({
                "enabled": true,
                "latency_ms": 200,
                "download_bytes_per_second": 131_072,
                "upload_bytes_per_second": 32_768,
            })
        );
    }

    #[test]
    fn settings_round_trip_through_json() {
        let line = serde_json::to_string(&shaped()).expect("serializes");
        let back: ThrottleSettings = serde_json::from_str(&line).expect("deserializes");

        assert_eq!(back, shaped());
    }

    #[test]
    fn a_summary_says_what_is_being_shaped() {
        assert_eq!(ThrottleSettings::none().summary(), "off");
        assert_eq!(
            shaped().summary(),
            "200 ms latency, 131072 B/s down, 32768 B/s up"
        );
    }
}
