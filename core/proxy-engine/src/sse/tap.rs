//! A response body that is parsed as SSE on the way past.
//!
//! The inner body is forwarded unchanged. Events are captured as they
//! complete, up to the stream-message limit, and never held back.

use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::body::{Body, Bytes, Frame, SizeHint};
use hyper::header::{HeaderMap, CONTENT_TYPE};

use super::parse::SseParser;
use crate::capture::stream::StreamTap;
use proxy_storage::{StreamDirection, StreamKind};

/// Whether `headers` name a Server-Sent Event stream.
pub fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .eq_ignore_ascii_case("text/event-stream")
        })
        .unwrap_or(false)
}

/// Wraps a response body so each SSE event is captured as it goes past.
pub struct SseTap<B: Body<Data = Bytes>> {
    inner: B,
    parser: SseParser,
    tap: StreamTap,
}

impl<B> SseTap<B>
where
    B: Body<Data = Bytes>,
{
    /// Wraps `inner`, capturing events onto `tap`.
    pub fn new(inner: B, tap: StreamTap, limit: usize) -> Self {
        Self {
            inner,
            parser: SseParser::new(limit),
            tap,
        }
    }

    fn emit(&mut self, bytes: &[u8]) {
        for event in self.parser.push(bytes) {
            self.tap.capture_sse(event);
        }
    }
}

impl<B> Body for SseTap<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match Pin::new(&mut this.inner).poll_frame(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.emit(data);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl StreamTap {
    /// Captures one parsed SSE event.
    pub fn capture_sse(&self, event: super::parse::SseEvent) {
        let payload = if event.truncated {
            proxy_storage::BodyCapture::truncated(event.data, event.size)
        } else {
            proxy_storage::BodyCapture::complete(event.data)
        };
        self.capture(
            StreamDirection::ServerToClient,
            StreamKind::Sse,
            event.event_name,
            event.event_id,
            event.retry_ms,
            payload,
        );
    }
}
