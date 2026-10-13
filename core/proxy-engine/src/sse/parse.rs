//! Parsing Server-Sent Events as they stream past.
//!
//! An event stream is still an ordinary HTTP body. This parser sits beside
//! the capture, not in front of it: every byte still reaches the client in
//! the order it arrived, and what is kept here is a bounded copy of each
//! event, cut at the same limit a captured body is.
//!
//! The grammar is WHATWG's: fields named `event`, `data`, `id`, and `retry`,
//! events separated by a blank line. A field this parser does not know is
//! ignored rather than fatal. An incomplete event at the end of the stream
//! is dropped — that is the spec. A line that never ends is discarded past
//! the capture limit, so one endless line cannot grow the buffer with it.

/// One dispatched event, as the inspector shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, when the event named one.
    pub event_name: Option<String>,
    /// The `id:` field, when the event named one.
    pub event_id: Option<String>,
    /// The `retry:` field, milliseconds, when present and a number.
    pub retry_ms: Option<u64>,
    /// The `data:` fields, joined with a newline, as they were written.
    pub data: Vec<u8>,
    /// How long the data was, whether or not that much was kept.
    pub size: u64,
    /// Whether `data` was cut short of what the event carried.
    pub truncated: bool,
}

/// How many extra bytes of a line are kept beyond the capture limit.
///
/// The field prefix (`data: `) has to fit in front of the captured value.
/// Anything past the ceiling is counted, when it is data, and not stored.
const LINE_SLACK: usize = 64;

/// The most of an SSE `event` or `id` field that is kept.
///
/// Those fields travel on the summary, which is sent for every event. A
/// multi-megabyte name would be a summary that is not one. Ordinary names
/// are a few characters and are kept whole.
const MAX_FIELD_TEXT: usize = 8 * 1024;

/// Accumulates bytes and emits events as blank lines arrive.
pub struct SseParser {
    buffer: Vec<u8>,
    event_name: Option<String>,
    event_id: Option<String>,
    retry_ms: Option<u64>,
    data: Vec<u8>,
    data_size: u64,
    truncated: bool,
    skipping: bool,
    saw_bom: bool,
    /// The rest of a line that already hit the ceiling, until its ending.
    discarding: bool,
    /// The discarded bytes belong to a `data` field and count toward its size.
    discard_as_data: bool,
    /// The discarded line ended on a bare `\r` and the next byte may be `\n`.
    discard_saw_cr: bool,
    limit: usize,
}

impl SseParser {
    /// A parser that keeps at most `limit` bytes of one event's data.
    pub fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            event_name: None,
            event_id: None,
            retry_ms: None,
            data: Vec::new(),
            data_size: 0,
            truncated: false,
            skipping: false,
            saw_bom: false,
            discarding: false,
            discard_as_data: false,
            discard_saw_cr: false,
            limit,
        }
    }

    /// How many bytes of an unfinished line are currently retained.
    #[cfg(test)]
    fn retained(&self) -> usize {
        self.buffer.len()
    }

    /// The most of one unfinished line this parser will retain.
    fn line_ceiling(&self) -> usize {
        self.limit.saturating_add(LINE_SLACK).max(LINE_SLACK)
    }

    /// Consumes `bytes` and returns every event that completed in them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut input = bytes;
        if !self.saw_bom {
            if let Some(rest) = input.strip_prefix(&[0xef, 0xbb, 0xbf]) {
                input = rest;
            }
            self.saw_bom = true;
        }

        let mut events = Vec::new();
        self.absorb(input, &mut events);
        events
    }

    /// Takes `input` into the line buffer, or discards it once the line is
    /// already longer than the capture is allowed to remember.
    fn absorb(&mut self, mut input: &[u8], events: &mut Vec<SseEvent>) {
        loop {
            if self.discard_saw_cr {
                self.discard_saw_cr = false;
                if input.first() == Some(&b'\n') {
                    input = &input[1..];
                }
                self.discarding = false;
                self.discard_as_data = false;
            }

            if self.discarding {
                if input.is_empty() {
                    return;
                }
                match input
                    .iter()
                    .position(|byte| *byte == b'\n' || *byte == b'\r')
                {
                    Some(index) => {
                        self.note_discarded(&input[..index]);
                        if input[index] == b'\r' && index + 1 == input.len() {
                            self.discard_saw_cr = true;
                            return;
                        }
                        let skip = 1 + usize::from(
                            input[index] == b'\r' && input.get(index + 1) == Some(&b'\n'),
                        );
                        input = &input[index + skip..];
                        self.discarding = false;
                        self.discard_as_data = false;
                    }
                    None => {
                        self.note_discarded(input);
                        return;
                    }
                }
                continue;
            }

            self.drain_lines(events);
            if self.buffer.len() >= self.line_ceiling() {
                self.begin_discard();
                continue;
            }
            if input.is_empty() {
                return;
            }

            let room = self.line_ceiling() - self.buffer.len();
            let take = room.min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
        }
    }

    fn drain_lines(&mut self, events: &mut Vec<SseEvent>) {
        while let Some(line) = next_line(&mut self.buffer) {
            if let Some(event) = self.consume_line(&line) {
                events.push(event);
            }
        }
    }

    /// The buffer is one line that has already hit the ceiling. Keep the
    /// prefix, and discard until the line actually ends.
    fn begin_discard(&mut self) {
        let line = std::mem::take(&mut self.buffer);
        self.discarding = true;
        if self.skipping {
            return;
        }

        let (name, value) = split_field(&line);
        match name {
            b"data" => {
                self.push_data(value);
                self.discard_as_data = true;
                self.truncated = true;
            }
            b"event" => self.event_name = Some(bounded_field(value)),
            b"id" => {
                if !value.contains(&b'\0') {
                    self.event_id = Some(bounded_field(value));
                }
            }
            b"retry" => {
                if let Ok(text) = std::str::from_utf8(value) {
                    if text.bytes().all(|byte| byte.is_ascii_digit()) {
                        self.retry_ms = text.parse().ok();
                    }
                }
            }
            _ => {}
        }
    }

    fn note_discarded(&mut self, bytes: &[u8]) {
        if self.discard_as_data {
            self.data_size += bytes.len() as u64;
        }
    }

    /// Ends the stream. An event that never saw a blank line is dropped.
    pub fn finish(self) -> Option<SseEvent> {
        let _ = self;
        None
    }

    fn consume_line(&mut self, line: &[u8]) -> Option<SseEvent> {
        if self.skipping {
            if line.is_empty() {
                self.skipping = false;
                return self.dispatch();
            }
            return None;
        }

        if line.is_empty() {
            return self.dispatch();
        }

        if line.first() == Some(&b':') {
            return None;
        }

        let (name, value) = split_field(line);
        match name {
            b"event" => self.event_name = Some(bounded_field(value)),
            b"id" => {
                if !value.contains(&b'\0') {
                    self.event_id = Some(bounded_field(value));
                }
            }
            b"retry" => {
                if let Ok(text) = std::str::from_utf8(value) {
                    if text.bytes().all(|byte| byte.is_ascii_digit()) {
                        self.retry_ms = text.parse().ok();
                    }
                }
            }
            b"data" => self.push_data(value),
            _ => {}
        }

        None
    }

    fn push_data(&mut self, value: &[u8]) {
        if !self.data.is_empty() || self.data_size > 0 {
            self.push_bytes(b"\n");
        }
        self.push_bytes(value);
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        self.data_size += bytes.len() as u64;
        if self.truncated {
            return;
        }

        let remaining = self.limit.saturating_sub(self.data.len());
        if remaining == 0 {
            self.truncated = true;
            self.skipping = true;
            return;
        }

        if bytes.len() <= remaining {
            self.data.extend_from_slice(bytes);
        } else {
            self.data.extend_from_slice(&bytes[..remaining]);
            self.truncated = true;
            self.skipping = true;
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.data_size == 0 && !self.truncated {
            self.reset_event();
            return None;
        }

        let event = SseEvent {
            event_name: self.event_name.take(),
            event_id: self.event_id.take(),
            retry_ms: self.retry_ms.take(),
            data: std::mem::take(&mut self.data),
            size: self.data_size,
            truncated: self.truncated,
        };
        self.reset_event();
        Some(event)
    }

    fn reset_event(&mut self) {
        self.event_name = None;
        self.retry_ms = None;
        self.data.clear();
        self.data_size = 0;
        self.truncated = false;
        self.skipping = false;
        // `id` is kept across events in the spec as `lastEventId`, but the
        // inspector records the field on the event that named it, not as a
        // running default. Clearing it keeps one event's id off the next.
        self.event_id = None;
    }
}

/// The next line in `buffer`, without its terminator, or `None` if none has
/// ended yet.
fn next_line(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = buffer
        .iter()
        .position(|byte| *byte == b'\n' || *byte == b'\r')?;
    let mut line: Vec<u8> = buffer.drain(..=end).collect();
    let terminator = line.pop();
    if terminator == Some(b'\r') && buffer.first() == Some(&b'\n') {
        buffer.remove(0);
    }
    Some(line)
}

/// Splits `line` into a field name and its value, dropping the optional
/// single leading space on the value.
fn split_field(line: &[u8]) -> (&[u8], &[u8]) {
    match line.iter().position(|byte| *byte == b':') {
        Some(colon) => {
            let value = &line[colon + 1..];
            let value = value.strip_prefix(b" ").unwrap_or(value);
            (&line[..colon], value)
        }
        None => (line, b""),
    }
}

fn field_text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).into_owned()
}

/// `value` as text, cut at [`MAX_FIELD_TEXT`] on a character boundary.
fn bounded_field(value: &[u8]) -> String {
    let text = field_text(value);
    if text.len() <= MAX_FIELD_TEXT {
        return text;
    }

    let mut end = MAX_FIELD_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(limit: usize, chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut parser = SseParser::new(limit);
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(parser.push(chunk));
        }
        events
    }

    #[test]
    fn a_named_event_keeps_its_fields() {
        let events = parse(
            1_024,
            &[b"event: update\nid: 42\nretry: 3000\ndata: hello\n\n"],
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_name.as_deref(), Some("update"));
        assert_eq!(events[0].event_id.as_deref(), Some("42"));
        assert_eq!(events[0].retry_ms, Some(3_000));
        assert_eq!(events[0].data, b"hello");
        assert_eq!(events[0].size, 5);
        assert!(!events[0].truncated);
    }

    #[test]
    fn data_lines_are_joined_with_a_newline() {
        let events = parse(1_024, &[b"data: one\ndata: two\n\n"]);

        assert_eq!(events[0].data, b"one\ntwo");
        assert!(events[0].event_name.is_none());
    }

    #[test]
    fn comments_are_ignored() {
        let events = parse(1_024, &[b": keep-alive\ndata: still here\n\n"]);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, b"still here");
    }

    #[test]
    fn events_are_emitted_as_chunks_arrive() {
        let mut parser = SseParser::new(1_024);
        assert!(parser.push(b"data: hel").is_empty());
        assert!(parser.push(b"lo\n").is_empty());
        let events = parser.push(b"\n");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, b"hello");
    }

    #[test]
    fn a_long_event_is_truncated_and_the_stream_continues() {
        let events = parse(4, &[b"data: hello world\n\ndata: later\n\n"]);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, b"hell");
        assert_eq!(events[0].size, 11);
        assert!(events[0].truncated);
        assert_eq!(events[1].data, b"late");
        assert!(events[1].truncated);
    }

    #[test]
    fn crlf_and_cr_are_both_line_endings() {
        let events = parse(1_024, &[b"data: a\r\ndata: b\r\n\r\n"]);
        assert_eq!(events[0].data, b"a\nb");

        let events = parse(1_024, &[b"data: c\rdata: d\r\r"]);
        assert_eq!(events[0].data, b"c\nd");
    }

    #[test]
    fn an_unfinished_event_is_dropped() {
        let mut parser = SseParser::new(1_024);
        assert!(parser.push(b"data: hanging").is_empty());
        assert!(parser.finish().is_none());
    }

    #[test]
    fn a_retry_that_is_not_a_number_is_ignored() {
        let events = parse(1_024, &[b"retry: soon\ndata: x\n\n"]);
        assert_eq!(events[0].retry_ms, None);
    }

    #[test]
    fn a_line_without_an_ending_does_not_grow_without_limit() {
        let mut parser = SseParser::new(8);
        let mut huge = b"data: ".to_vec();
        huge.extend(std::iter::repeat_n(b'x', 2_000_000));

        assert!(parser.push(&huge).is_empty());
        assert!(
            parser.retained() <= parser.line_ceiling(),
            "retained {} bytes of a line with no ending",
            parser.retained()
        );

        let events = parser.push(b"\n\ndata: ok\n\n");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, b"xxxxxxxx");
        assert_eq!(events[0].size, 2_000_000);
        assert!(events[0].truncated);
        assert_eq!(events[1].data, b"ok");
        assert!(!events[1].truncated);
        assert!(parser.retained() <= parser.line_ceiling());
    }
}
