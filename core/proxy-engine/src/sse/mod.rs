//! SSE parsing, kept beside the body it reads and not in front of it.

mod parse;
mod tap;

pub use parse::{SseEvent, SseParser};
pub use tap::{is_event_stream, SseTap};
