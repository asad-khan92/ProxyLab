//! Matching a search box against stored traffic.
//!
//! ```text
//! host / URL / path / method / status
//! request and response headers
//! request and response bodies (captured bytes only)
//! WebSocket text payloads, SSE name / id / data
//! binary WebSocket metadata (size, direction) — never the bytes
//! ```
//!
//! The traffic list still returns summaries. Search runs in the database so a
//! megabyte body is read for the row being considered and then dropped, rather
//! than being collected for every exchange and then sieved in process.
//!
//! Plain text is a substring. Case folding follows SQLite's `LIKE`: ASCII
//! letters only. Optional regex is the `regex` crate, compiled once per query
//! and refused rather than guessed at when the pattern is not one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use regex::{Regex, RegexBuilder};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::Connection;

use super::error::StorageError;
use super::query::TrafficFilter;

/// Bit 0 of the SQL flag integer: the pattern is a regular expression.
pub const FLAG_REGEX: i64 = 1;
/// Bit 1: comparisons are case-sensitive.
pub const FLAG_CASE_SENSITIVE: i64 = 2;

/// Why a search pattern could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSearch;

/// A search pattern that has already been checked.
#[derive(Clone)]
pub struct Needle {
    pattern: String,
    regex: Option<Regex>,
    case_sensitive: bool,
}

impl Needle {
    /// Compiles `text` as a substring or a regular expression.
    ///
    /// A regular expression that will not compile is [`InvalidSearch`]: the
    /// pattern is not echoed, because it was typed by a person and may carry
    /// anything.
    pub fn compile(text: &str, regex: bool, case_sensitive: bool) -> Result<Self, InvalidSearch> {
        let compiled = if regex {
            Some(
                RegexBuilder::new(text)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|_| InvalidSearch)?,
            )
        } else {
            None
        };

        Ok(Self {
            pattern: text.to_owned(),
            regex: compiled,
            case_sensitive,
        })
    }

    /// Whether `haystack` contains this pattern.
    pub fn matches_text(&self, haystack: &str) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(haystack);
        }
        if self.case_sensitive {
            haystack.contains(&self.pattern)
        } else {
            contains_ascii_insensitive(haystack, &self.pattern)
        }
    }

    /// Whether the captured bytes contain this pattern.
    ///
    /// Bytes are read as UTF-8 with lossy replacement so a truncated capture
    /// that ends mid-character can still match the text it did keep. A binary
    /// WebSocket payload is never handed to this.
    pub fn matches_bytes(&self, haystack: &[u8]) -> bool {
        self.matches_text(&String::from_utf8_lossy(haystack))
    }
}

/// Whether `haystack` contains `needle`, folding only ASCII case.
///
/// SQLite's `LIKE` folds `A`–`Z` and nothing else. Matching that exactly is
/// what keeps a live filter and a reload agreeing.
fn contains_ascii_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }

    let hay = ascii_lower(haystack);
    let pin = ascii_lower(needle);
    hay.contains(&pin)
}

fn ascii_lower(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                character
            }
        })
        .collect()
}

/// The integer the SQL functions take for a filter's search flags.
pub fn flags(filter: &TrafficFilter) -> i64 {
    let mut bits = 0;
    if filter.search_regex {
        bits |= FLAG_REGEX;
    }
    if filter.search_case_sensitive {
        bits |= FLAG_CASE_SENSITIVE;
    }
    bits
}

/// Whether `pattern` is a regular expression this crate can compile.
pub fn regex_is_valid(pattern: &str) -> bool {
    Regex::new(pattern).is_ok()
}

/// Registers the functions a search clause calls.
///
/// `bodies` is the directory file-backed captures live in. A function reads
/// at most one file per call and drops it before returning, so a search never
/// holds every body at once.
pub fn register(connection: &Connection, bodies: PathBuf) -> Result<(), StorageError> {
    let cache = Arc::new(Mutex::new(NeedleCache::default()));
    let text_cache = Arc::clone(&cache);
    connection.create_scalar_function(
        "search_text",
        3,
        FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            let haystack: Option<String> = ctx.get(0)?;
            let Some(haystack) = haystack else {
                return Ok(0_i64);
            };
            let pattern: String = ctx.get(1)?;
            let flags: i64 = ctx.get(2)?;
            let needle = needle_from(&text_cache, &pattern, flags)?;
            Ok(i64::from(needle.matches_text(&haystack)))
        },
    )?;

    let capture_cache = Arc::clone(&cache);
    connection.create_scalar_function(
        "search_capture",
        4,
        FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            let inline: Option<Vec<u8>> = ctx.get(0)?;
            let file: Option<String> = ctx.get(1)?;
            let pattern: String = ctx.get(2)?;
            let flags: i64 = ctx.get(3)?;
            let needle = needle_from(&capture_cache, &pattern, flags)?;
            let bytes = capture_bytes(&bodies, inline.as_deref(), file.as_deref());
            match bytes {
                Some(data) => Ok(i64::from(needle.matches_bytes(&data))),
                None => Ok(0_i64),
            }
        },
    )?;

    Ok(())
}

/// The `WHERE` fragment that applies `filter`'s search box, and the values it
/// binds.
///
/// `None` when there is nothing typed. Every value is a bound parameter.
pub fn clause(filter: &TrafficFilter) -> Option<(String, Vec<Value>)> {
    let text = filter.text.as_deref()?;
    let flags = flags(filter);
    let flags_value = Value::Integer(flags);
    let pattern = Value::Text(text.to_owned());

    // One pattern, many columns. Positional parameters throughout: a numbered
    // one would refer to whichever parameter earlier clauses had already put
    // in that position.
    //
    // Binary WebSocket payloads are excluded from `search_capture`. Their
    // size and direction still sit in columns, so a search for `1024` or
    // `client_to_server` can find them as metadata.
    let sql = "(\
        search_text(host, ?, ?) OR \
        search_text(request_url, ?, ?) OR \
        search_text(request_method, ?, ?) OR \
        search_text(CAST(response_status AS TEXT), ?, ?) OR \
        search_text(request_content_type, ?, ?) OR \
        search_text(response_content_type, ?, ?) OR \
        EXISTS (\
            SELECT 1 FROM messages m \
            WHERE m.exchange_id = exchanges.id \
              AND (search_text(m.headers, ?, ?) \
                   OR search_capture(m.body_inline, m.body_file, ?, ?))\
        ) OR \
        EXISTS (\
            SELECT 1 FROM stream_messages s \
            WHERE s.exchange_id = exchanges.id \
              AND (search_text(s.event_name, ?, ?) \
                   OR search_text(s.event_id, ?, ?) \
                   OR search_text(CAST(s.payload_size AS TEXT), ?, ?) \
                   OR search_text(s.direction, ?, ?) \
                   OR (s.kind != 'websocket_binary' \
                       AND search_capture(s.payload_inline, s.payload_file, ?, ?)))\
        )\
    )"
    .to_owned();

    let mut values = Vec::new();
    // host, url, method, status, request content type, response content type
    for _ in 0..6 {
        values.push(pattern.clone());
        values.push(flags_value.clone());
    }
    // messages: headers + body
    for _ in 0..2 {
        values.push(pattern.clone());
        values.push(flags_value.clone());
    }
    // stream: event_name, event_id, size, direction, payload
    for _ in 0..5 {
        values.push(pattern.clone());
        values.push(flags_value.clone());
    }

    Some((sql, values))
}

/// The captured bytes of one message, or nothing if there are none.
fn capture_bytes(bodies: &Path, inline: Option<&[u8]>, file: Option<&str>) -> Option<Vec<u8>> {
    if let Some(data) = inline {
        return Some(data.to_vec());
    }
    let name = file?;
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    let path = bodies.join(name);
    std::fs::read(path).ok()
}

#[derive(Default)]
struct NeedleCache {
    key: Option<(String, i64)>,
    needle: Option<Needle>,
}

fn needle_from(cache: &Mutex<NeedleCache>, pattern: &str, flags: i64) -> rusqlite::Result<Needle> {
    let regex = flags & FLAG_REGEX != 0;
    let case_sensitive = flags & FLAG_CASE_SENSITIVE != 0;
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard
        .key
        .as_ref()
        .is_some_and(|(held, held_flags)| held == pattern && *held_flags == flags)
    {
        if let Some(needle) = &guard.needle {
            return Ok(needle.clone());
        }
    }

    let needle = Needle::compile(pattern, regex, case_sensitive).map_err(|_| {
        rusqlite::Error::UserFunctionError("the search is not a regular expression".into())
    })?;
    guard.key = Some((pattern.to_owned(), flags));
    guard.needle = Some(needle.clone());
    Ok(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_substring_matches_without_regard_to_ascii_case() {
        let needle = Needle::compile("Users", false, false).expect("compiles");

        assert!(needle.matches_text("/v1/users"));
        assert!(needle.matches_text("/V1/USERS"));
        assert!(!needle.matches_text("/v1/orders"));
    }

    #[test]
    fn a_case_sensitive_substring_keeps_the_letters_it_was_given() {
        let needle = Needle::compile("Users", false, true).expect("compiles");

        assert!(needle.matches_text("/v1/Users"));
        assert!(!needle.matches_text("/v1/users"));
    }

    #[test]
    fn a_regex_matches_the_pattern() {
        let needle = Needle::compile(r"user-\d+", true, false).expect("compiles");

        assert!(needle.matches_text("user-42"));
        assert!(needle.matches_text("USER-42"));
        assert!(!needle.matches_text("user-x"));
    }

    #[test]
    fn a_regex_that_is_not_one_is_refused() {
        assert!(Needle::compile("(", true, false).is_err());
        assert!(Needle::compile("(", false, false).is_ok());
    }

    #[test]
    fn ascii_case_folding_does_not_fold_latin1() {
        // SQLite's LIKE folds `A`–`Z` and nothing else. `Ä` and `ä` stay two
        // characters, which is what a live filter has to do as well.
        assert!(!Needle::compile("är", false, false)
            .expect("compiles")
            .matches_text("Ärger"));
        assert!(Needle::compile("Är", false, false)
            .expect("compiles")
            .matches_text("Ärger"));
    }

    #[test]
    fn captured_bytes_are_read_as_text() {
        let needle = Needle::compile("ada", false, false).expect("compiles");

        assert!(needle.matches_bytes(br#"{"name":"Ada"}"#));
        assert!(!needle.matches_bytes(b"\x00\xff"));
    }
}
