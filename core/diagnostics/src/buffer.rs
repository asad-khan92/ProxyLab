//! The recent problems, kept in memory and nowhere else.
//!
//! Newest entries are the ones worth having when something has just gone
//! wrong, so a full buffer drops the oldest. Nothing is written to disk:
//! quitting the process is what clears it, which is what "clear the
//! application" already does.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::Level;

/// How many warnings and errors are kept.
pub const LIMIT: usize = 32;

/// One diagnostic event, already sanitized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// How serious it is.
    pub level: Level,
    /// Which part of ProxyLab it came from.
    pub component: &'static str,
    /// What that part was doing.
    pub operation: &'static str,
    /// What kind of failure it was.
    pub category: &'static str,
    /// The sentence, with secrets removed.
    pub message: String,
    /// A host and port, when the failure was about one.
    pub endpoint: Option<String>,
    /// The exchange it belonged to, when there was one.
    pub exchange_id: Option<String>,
    /// How long the operation had been running, when that was measured.
    pub duration_ms: Option<u64>,
    /// When it happened, as milliseconds since the Unix epoch.
    pub at_ms: u64,
}

/// A bounded list of recent warnings and errors.
#[derive(Debug)]
pub struct Buffer {
    entries: Mutex<VecDeque<Record>>,
}

impl Buffer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(LIMIT)),
        }
    }

    /// Keeps `record` when it is a warning or an error.
    ///
    /// Info and debug are printed and then forgotten: the buffer is for
    /// what went wrong, and a lifecycle line would push a real failure out.
    pub fn push(&self, record: Record) {
        if record.level > Level::Warn {
            return;
        }

        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if entries.len() == LIMIT {
            entries.pop_front();
        }
        entries.push_back(record);
    }

    /// The records kept so far, newest first.
    pub fn recent(&self) -> Vec<Record> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        entries.iter().rev().cloned().collect()
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(level: Level, message: &str) -> Record {
        Record {
            level,
            component: "upstream",
            operation: "connect",
            category: "connection",
            message: message.to_owned(),
            endpoint: None,
            exchange_id: None,
            duration_ms: None,
            at_ms: 0,
        }
    }

    #[test]
    fn warnings_and_errors_are_kept_and_info_is_not() {
        let buffer = Buffer::new();
        buffer.push(record(Level::Info, "started"));
        buffer.push(record(Level::Debug, "detail"));
        buffer.push(record(Level::Warn, "retrying"));
        buffer.push(record(Level::Error, "refused"));

        let recent = buffer.recent();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message, "refused");
        assert_eq!(recent[1].message, "retrying");
    }

    #[test]
    fn the_oldest_entry_is_dropped_when_the_buffer_is_full() {
        let buffer = Buffer::new();
        for index in 0..(LIMIT + 5) {
            buffer.push(record(Level::Error, &index.to_string()));
        }

        let recent = buffer.recent();
        assert_eq!(recent.len(), LIMIT);
        assert_eq!(recent[0].message, (LIMIT + 4).to_string());
        assert_eq!(
            recent.last().map(|record| record.message.as_str()),
            Some("5")
        );
        assert!(recent.iter().all(|record| record.message != "0"));
        assert!(recent.iter().all(|record| record.message != "4"));
    }
}
