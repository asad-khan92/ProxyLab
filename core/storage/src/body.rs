//! Capturing request and response bodies, up to a limit.
//!
//! A body travelling through the proxy is forwarded as it streams; nothing
//! here changes that. Capture is a copy taken on the way past, and the copy
//! is what has to be bounded: a 4 GB download must reach the client in full
//! and must not put 4 GB in ProxyLab's memory.
//!
//! ```text
//! Upstream body: 20 MB     Forwarded: 20 MB
//! Capture limit:  1 MB     Captured:   1 MB, truncated
//! ```
//!
//! So [`BodyCollector`] keeps the first [`CaptureConfig::max_capture_body_size`]
//! bytes and counts the rest. The count is the real size of the body, which is
//! worth knowing even when the bytes are not kept.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How much of a body is captured when nothing says otherwise.
///
/// One mebibyte is large enough for the API responses a developer is usually
/// looking at, and small enough that a page of concurrent downloads does not
/// become a memory problem.
pub const DEFAULT_MAX_CAPTURE_BODY_SIZE: usize = 1024 * 1024;

/// What the capture is allowed to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// The most of any one body that is kept in memory, in bytes.
    ///
    /// Bodies longer than this are forwarded in full and captured up to here,
    /// with the capture marked truncated. Zero captures no body bytes at all,
    /// which is a way to keep the headers and timing of traffic that is too
    /// large or too private to hold on to.
    pub max_capture_body_size: usize,
}

impl CaptureConfig {
    /// A configuration that captures at most `max_capture_body_size` bytes of
    /// each body.
    pub fn new(max_capture_body_size: usize) -> Self {
        Self {
            max_capture_body_size,
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CAPTURE_BODY_SIZE)
    }
}

/// As much of a body as was captured, and how much there was.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyCapture {
    /// The captured bytes, written as base64 in JSON.
    ///
    /// Never decoded as text by the model: a body is bytes, and a great many
    /// of them are images, archives, or protocol buffers.
    #[serde(with = "crate::base64_bytes")]
    data: Vec<u8>,
    /// How long the body was, whether or not that much was captured.
    size: u64,
    /// Whether the body was longer than the capture limit.
    truncated: bool,
}

impl BodyCapture {
    /// A body with nothing in it.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A body captured in full.
    pub fn complete(data: impl Into<Vec<u8>>) -> Self {
        let data = data.into();
        let size = data.len() as u64;

        Self {
            data,
            size,
            truncated: false,
        }
    }

    /// The first `data` bytes of a body that was `size` bytes long.
    pub fn truncated(data: impl Into<Vec<u8>>, size: u64) -> Self {
        Self {
            data: data.into(),
            size,
            truncated: true,
        }
    }

    /// Captures `payload` up to `limit` bytes, marking the rest truncated.
    ///
    /// A stream message uses this the same way an HTTP body does: the rest is
    /// still forwarded, and only this much is remembered.
    pub fn limited(payload: &[u8], limit: usize) -> Self {
        if payload.len() <= limit {
            Self::complete(payload.to_vec())
        } else {
            Self::truncated(payload[..limit].to_vec(), payload.len() as u64)
        }
    }

    /// The bytes that were captured.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// How long the whole body was, in bytes.
    ///
    /// This is the length of what was forwarded, which is larger than
    /// [`captured_len`](Self::captured_len) when the body was truncated.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How many bytes of the body were captured.
    pub fn captured_len(&self) -> usize {
        self.data.len()
    }

    /// Whether the body was longer than the capture limit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Whether there was no body at all.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// The captured bytes as text, or `None` if they are not valid UTF-8.
    ///
    /// A truncated capture can cut a multi-byte character in half, so this
    /// says `None` for a truncated text body whose last character did not
    /// survive rather than guessing at what was there.
    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

/// Sizes only, never the body. See the crate documentation for why.
impl fmt::Debug for BodyCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyCapture")
            .field("captured", &self.captured_len())
            .field("size", &self.size)
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// Collects a body as it streams past, up to a limit.
///
/// Every byte forwarded is counted; only the first `limit` are kept.
pub struct BodyCollector {
    limit: usize,
    data: Vec<u8>,
    size: u64,
}

impl BodyCollector {
    /// A collector that keeps at most `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            data: Vec::new(),
            size: 0,
        }
    }

    /// A collector configured by `config`.
    pub fn with_config(config: &CaptureConfig) -> Self {
        Self::new(config.max_capture_body_size)
    }

    /// Records bytes that have just gone past.
    ///
    /// Nothing is allocated beyond the limit: once the capture is full the
    /// bytes are counted and dropped.
    pub fn push(&mut self, bytes: &[u8]) {
        self.size += bytes.len() as u64;

        let room = self.limit.saturating_sub(self.data.len());
        if room == 0 {
            return;
        }

        let keep = room.min(bytes.len());
        self.data.extend_from_slice(&bytes[..keep]);
    }

    /// How much of the body has gone past so far.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How much of the body has been kept.
    pub fn captured_len(&self) -> usize {
        self.data.len()
    }

    /// The capture as it stands.
    pub fn capture(&self) -> BodyCapture {
        BodyCapture {
            data: self.data.clone(),
            size: self.size,
            truncated: self.size > self.data.len() as u64,
        }
    }
}

/// Sizes only, never the body. See the crate documentation for why.
impl fmt::Debug for BodyCollector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BodyCollector")
            .field("limit", &self.limit)
            .field("captured", &self.data.len())
            .field("size", &self.size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collected(limit: usize, chunks: &[&[u8]]) -> BodyCapture {
        let mut collector = BodyCollector::new(limit);
        for chunk in chunks {
            collector.push(chunk);
        }

        collector.capture()
    }

    #[test]
    fn the_default_limit_is_one_mebibyte() {
        assert_eq!(CaptureConfig::default().max_capture_body_size, 1_048_576);
        assert_eq!(DEFAULT_MAX_CAPTURE_BODY_SIZE, 1_048_576);
    }

    #[test]
    fn a_body_that_never_arrived_is_empty() {
        let capture = collected(1024, &[]);

        assert!(capture.is_empty());
        assert_eq!(capture.size(), 0);
        assert_eq!(capture.data(), b"");
        assert!(!capture.is_truncated());
    }

    #[test]
    fn a_body_under_the_limit_is_captured_whole() {
        let capture = collected(1024, &[b"hello ", b"world"]);

        assert_eq!(capture.data(), b"hello world");
        assert_eq!(capture.size(), 11);
        assert_eq!(capture.captured_len(), 11);
        assert!(!capture.is_truncated());
        assert_eq!(capture.as_text(), Some("hello world"));
    }

    #[test]
    fn a_body_exactly_at_the_limit_is_not_truncated() {
        let capture = collected(5, &[b"hello"]);

        assert_eq!(capture.data(), b"hello");
        assert!(!capture.is_truncated());
    }

    #[test]
    fn a_body_over_the_limit_keeps_its_first_bytes_and_its_real_size() {
        let capture = collected(5, &[b"hello", b" world"]);

        assert_eq!(capture.data(), b"hello");
        assert_eq!(capture.captured_len(), 5);
        assert_eq!(capture.size(), 11);
        assert!(capture.is_truncated());
    }

    #[test]
    fn a_chunk_that_straddles_the_limit_is_cut_at_it() {
        let capture = collected(8, &[b"hel", b"lo world"]);

        assert_eq!(capture.data(), b"hello wo");
        assert_eq!(capture.size(), 11);
        assert!(capture.is_truncated());
    }

    #[test]
    fn a_limit_of_zero_captures_nothing_but_still_counts() {
        let capture = collected(0, &[b"hello world"]);

        assert_eq!(capture.data(), b"");
        assert_eq!(capture.size(), 11);
        assert!(capture.is_truncated());
    }

    #[test]
    fn nothing_beyond_the_limit_is_held_on_to() {
        let mut collector = BodyCollector::new(16);
        for _ in 0..1024 {
            collector.push(&[0_u8; 1024]);
        }

        assert_eq!(collector.size(), 1024 * 1024);
        assert!(collector.capture().captured_len() <= 16);
    }

    #[test]
    fn binary_bytes_are_captured_exactly() {
        let bytes: Vec<u8> = (0..=255).collect();

        let capture = collected(1024, &[&bytes]);

        assert_eq!(capture.data(), bytes);
        assert_eq!(capture.as_text(), None);
    }

    #[test]
    fn text_is_captured_exactly() {
        let capture = collected(1024, &["héllo — wörld".as_bytes()]);

        assert_eq!(capture.as_text(), Some("héllo — wörld"));
    }

    #[test]
    fn a_capture_that_cut_a_character_in_half_is_not_claimed_to_be_text() {
        // "é" is two bytes; the limit falls between them.
        let capture = collected(2, &["aé".as_bytes()]);

        assert_eq!(capture.as_text(), None);
        assert!(capture.is_truncated());
    }

    #[test]
    fn a_payload_shorter_than_the_limit_is_kept_whole() {
        let capture = BodyCapture::limited(b"hello", 8);

        assert_eq!(capture.data(), b"hello");
        assert_eq!(capture.size(), 5);
        assert!(!capture.is_truncated());
    }

    #[test]
    fn a_payload_longer_than_the_limit_is_cut_and_counted() {
        let capture = BodyCapture::limited(b"hello world", 5);

        assert_eq!(capture.data(), b"hello");
        assert_eq!(capture.size(), 11);
        assert!(capture.is_truncated());
    }

    #[test]
    fn a_capture_round_trips_through_json() {
        for capture in [
            BodyCapture::empty(),
            BodyCapture::complete(b"hello".to_vec()),
            BodyCapture::complete((0..=255).collect::<Vec<u8>>()),
            BodyCapture::truncated(b"hel".to_vec(), 4096),
        ] {
            let json = serde_json::to_string(&capture).expect("serializes");
            let back: BodyCapture = serde_json::from_str(&json).expect("deserializes");

            assert_eq!(back, capture);
        }
    }

    #[test]
    fn a_truncated_capture_says_so_in_json() {
        let json =
            serde_json::to_string(&BodyCapture::truncated(b"hel".to_vec(), 4096)).expect("json");

        assert_eq!(json, r#"{"data":"aGVs","size":4096,"truncated":true}"#);
    }

    #[test]
    fn debug_never_renders_a_body() {
        let capture = BodyCapture::complete(b"password=hunter2".to_vec());

        let rendered = format!("{capture:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("size: 16"), "{rendered}");
    }

    #[test]
    fn debug_never_renders_a_body_being_collected() {
        let mut collector = BodyCollector::new(64);
        collector.push(b"password=hunter2");

        let rendered = format!("{collector:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("size: 16"), "{rendered}");
    }
}
