//! When an exchange happened, and how long each part of it took.
//!
//! Two clocks are involved and they are not interchangeable.
//!
//! The wall clock says *when*: a [`Timestamp`] is what a UI puts in a column
//! next to a request. It can jump — an NTP correction, a daylight saving
//! change, a laptop waking up — so it is never subtracted from another one.
//!
//! The monotonic clock says *how long*. Every duration here is measured with
//! [`Instant`], which only ever moves forward, so a clock correction in the
//! middle of a request cannot produce a negative or wildly wrong duration.
//! [`TimingRecorder`] holds the `Instant` and hands out [`Timing`], which
//! carries the offsets it measured and none of the machinery.
//!
//! The offsets are deliberately few. Connection reuse, DNS resolution, and
//! the rest of a browser-style waterfall are a later concern; what is here is
//! what a debugging proxy can honestly measure today.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in time on the wall clock, as an offset from the Unix epoch.
///
/// Kept as seconds and nanoseconds rather than as a formatted string so that
/// it round-trips exactly and needs no date library to produce.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp {
    /// Whole seconds since 1970-01-01T00:00:00Z.
    pub seconds: u64,
    /// Nanoseconds within that second.
    pub nanos: u32,
}

impl Timestamp {
    /// The current wall-clock time.
    pub fn now() -> Self {
        Self::at(SystemTime::now())
    }

    /// The wall-clock time `time` names.
    ///
    /// A time before the epoch — which means the system clock is set to
    /// something implausible — becomes the epoch rather than an error, because
    /// a nonsensical clock is not a reason to lose the captured exchange.
    pub fn at(time: SystemTime) -> Self {
        let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();

        Self {
            seconds: since_epoch.as_secs(),
            nanos: since_epoch.subsec_nanos(),
        }
    }

    /// Milliseconds since the Unix epoch, which is what most UIs want.
    pub fn as_millis(self) -> u64 {
        self.seconds
            .saturating_mul(1_000)
            .saturating_add(u64::from(self.nanos) / 1_000_000)
    }
}

/// How long the parts of one exchange took.
///
/// Every field but [`started_at`](Self::started_at) is an offset in
/// microseconds from the moment the request was received, measured on the
/// monotonic clock. An offset is `None` when that moment did not happen: a
/// request that never reached an origin has no upstream response, and one
/// that was abandoned has no completion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    /// When the proxy received the request.
    pub started_at: Timestamp,
    /// When the proxy began looking for a connection to the origin.
    ///
    /// The offset is the same one it always was — measured at the moment the
    /// request is handed to the upstream side — but what happens next is no
    /// longer always a connection being opened: the request may be given a
    /// pooled connection instead, in which case the gap between this and
    /// [`upstream_response_micros`](Self::upstream_response_micros) is the
    /// round trip alone. Nothing is measured differently; there is simply
    /// less to measure when a connection is reused.
    ///
    /// `None` for an intercepted HTTPS request: its connection to the origin
    /// was opened by the `CONNECT` that preceded the request, before there
    /// was a request to attribute it to.
    pub upstream_connect_micros: Option<u64>,
    /// When the origin's response head arrived.
    pub upstream_response_micros: Option<u64>,
    /// When the response body finished being forwarded to the client.
    pub completed_micros: Option<u64>,
}

impl Timing {
    /// How long the whole exchange took, if it finished.
    pub fn total(&self) -> Option<Duration> {
        self.completed_micros.map(Duration::from_micros)
    }

    /// How long the whole exchange took, in milliseconds, if it finished.
    pub fn duration_ms(&self) -> Option<f64> {
        self.completed_micros.map(|micros| micros as f64 / 1_000.0)
    }

    /// How long the origin took to start answering, in milliseconds.
    pub fn upstream_response_ms(&self) -> Option<f64> {
        self.upstream_response_micros
            .map(|micros| micros as f64 / 1_000.0)
    }
}

/// Measures an exchange as it happens.
///
/// Holds the monotonic start point, which is not part of the model because it
/// means nothing outside this process.
#[derive(Debug)]
pub struct TimingRecorder {
    started_at: Timestamp,
    start: Instant,
    timing: Timing,
}

impl TimingRecorder {
    /// Starts measuring, now.
    pub fn started() -> Self {
        let started_at = Timestamp::now();

        Self {
            started_at,
            start: Instant::now(),
            timing: Timing {
                started_at,
                ..Timing::default()
            },
        }
    }

    /// Records that the connection to the origin is being opened.
    pub fn upstream_connect(&mut self) {
        self.timing.upstream_connect_micros = Some(self.elapsed_micros());
    }

    /// Records that the origin's response head has arrived.
    pub fn upstream_response(&mut self) {
        self.timing.upstream_response_micros = Some(self.elapsed_micros());
    }

    /// Records that the exchange is over.
    ///
    /// Only the first completion counts: an exchange finishes once, and a
    /// later call — a body dropped after it already ended — must not move the
    /// time it finished at.
    pub fn completed(&mut self) {
        if self.timing.completed_micros.is_none() {
            self.timing.completed_micros = Some(self.elapsed_micros());
        }
    }

    /// When the exchange started, on the wall clock.
    pub fn started_at(&self) -> Timestamp {
        self.started_at
    }

    /// The measurements so far.
    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// Microseconds since the start, on the monotonic clock.
    fn elapsed_micros(&self) -> u64 {
        // `as` saturates at `u64::MAX` for a duration no exchange can reach.
        self.start.elapsed().as_micros() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_an_offset_from_the_epoch() {
        let timestamp = Timestamp::at(UNIX_EPOCH + Duration::from_millis(1_500));

        assert_eq!(timestamp.seconds, 1);
        assert_eq!(timestamp.nanos, 500_000_000);
        assert_eq!(timestamp.as_millis(), 1_500);
    }

    #[test]
    fn a_clock_set_before_the_epoch_does_not_lose_the_exchange() {
        let timestamp = Timestamp::at(UNIX_EPOCH - Duration::from_secs(60));

        assert_eq!(timestamp, Timestamp::default());
    }

    #[test]
    fn the_wall_clock_start_is_a_plausible_time() {
        // Well after 2020 and comfortably before the clock could plausibly be
        // read as anything else: this is about the epoch being the right one,
        // not about any particular moment.
        assert!(Timestamp::now().seconds > 1_600_000_000);
    }

    #[test]
    fn a_timestamp_round_trips_through_json() {
        let timestamp = Timestamp::now();

        let json = serde_json::to_string(&timestamp).expect("serializes");
        let back: Timestamp = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, timestamp);
    }

    #[test]
    fn an_unfinished_exchange_has_no_duration() {
        let recorder = TimingRecorder::started();

        assert_eq!(recorder.timing().total(), None);
        assert_eq!(recorder.timing().duration_ms(), None);
        assert_eq!(recorder.timing().upstream_response_micros, None);
    }

    #[test]
    fn the_marks_are_recorded_in_the_order_they_happen() {
        let mut recorder = TimingRecorder::started();
        recorder.upstream_connect();
        recorder.upstream_response();
        recorder.completed();

        let timing = recorder.timing();
        let connect = timing.upstream_connect_micros.expect("connect was marked");
        let response = timing
            .upstream_response_micros
            .expect("response was marked");
        let completed = timing.completed_micros.expect("completion was marked");

        assert!(connect <= response, "{connect} > {response}");
        assert!(response <= completed, "{response} > {completed}");
        assert!(timing.duration_ms().is_some());
    }

    #[test]
    fn an_exchange_finishes_only_once() {
        let mut recorder = TimingRecorder::started();
        recorder.completed();
        let first = recorder.timing().completed_micros;

        recorder.completed();

        assert_eq!(recorder.timing().completed_micros, first);
    }

    #[test]
    fn the_start_timestamp_is_carried_into_the_timing() {
        let recorder = TimingRecorder::started();

        assert_eq!(recorder.timing().started_at, recorder.started_at());
    }

    #[test]
    fn timing_round_trips_through_json() {
        let mut recorder = TimingRecorder::started();
        recorder.upstream_connect();
        recorder.upstream_response();
        recorder.completed();
        let timing = recorder.timing();

        let json = serde_json::to_string(&timing).expect("serializes");
        let back: Timing = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(back, timing);
    }
}
