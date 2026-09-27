//! Capturing a body as it is forwarded.
//!
//! [`CapturingBody`] wraps a body and copies what goes through it into the
//! recording. It is a passthrough: every frame the inner body produces is
//! handed on unchanged, in the same order, at the same time, and the framing
//! the inner body declares — its size hint and its end-of-stream — is
//! declared unchanged too. The proxy streams exactly as it did before.
//!
//! What is *kept* is bounded. The capture stops at the configured limit while
//! forwarding carries on, so a 20 MB download reaches the client in full and
//! leaves 1 MB behind:
//!
//! ```text
//! poll_frame ─┬─> the client   (all 20 MB)
//!             └─> the capture  (the first 1 MB, marked truncated)
//! ```
//!
//! A wrapped body also reports how it ended, because the response body is
//! what says the exchange is over: read to the end, it publishes; broken
//! part-way, it publishes a failure; dropped before it finished — a client
//! that went away — it publishes what did arrive. That last case is why the
//! ending is decided in [`Drop`] as well as while polling: a body that is
//! never polled again still has to end somewhere.

use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::body::{Body, Bytes, Frame, SizeHint};

use super::Recorder;

/// Which half of the exchange a body belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// The client's request body, on its way to the origin.
    Request,
    /// The origin's response body, on its way to the client.
    Response,
}

/// How a body finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// It was read to the end.
    Complete,
    /// It stopped with an error part-way through.
    Failed,
    /// It was dropped before it ended.
    Abandoned,
}

/// A body that is captured as it is forwarded.
pub struct CapturingBody<B: Body<Data = Bytes>> {
    inner: B,
    /// The recording, until the body has finished. `None` afterwards, so a
    /// body cannot finish twice.
    tap: Option<(Recorder, Role)>,
    /// Whether the inner body has reported that it has nothing more.
    ended: bool,
}

impl<B> CapturingBody<B>
where
    B: Body<Data = Bytes>,
{
    /// Wraps `inner` so that what passes through it is recorded.
    pub(super) fn new(inner: B, recorder: Recorder, role: Role) -> Self {
        // A body that is already at its end will never be polled, so whether
        // it ended has to be answered before the first poll rather than by
        // one.
        let ended = inner.is_end_stream();

        Self {
            inner,
            tap: Some((recorder, role)),
            ended,
        }
    }

    /// Records bytes on their way past.
    fn record(&self, data: &Bytes) {
        if let Some((recorder, role)) = &self.tap {
            recorder.record(*role, data);
        }
    }

    /// Ends the capture, once.
    fn finish(&mut self, outcome: Outcome) {
        if let Some((recorder, role)) = self.tap.take() {
            recorder.finish(role, outcome);
        }
    }
}

impl<B> Body for CapturingBody<B>
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
                    this.record(data);
                }

                // A body with a declared length ends by having nothing left
                // rather than by being polled once more, and whoever is
                // forwarding it may stop polling as soon as it says so.
                if this.inner.is_end_stream() {
                    this.ended = true;
                    this.finish(Outcome::Complete);
                }

                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish(Outcome::Failed);

                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.ended = true;
                this.finish(Outcome::Complete);

                Poll::Ready(None)
            }
        }
    }

    /// Whatever the inner body says: capture must not change the framing.
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    /// Whatever the inner body says, so a `Content-Length` still survives the
    /// proxy.
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// A body dropped before it ended is an exchange that did not finish, and is
/// recorded as one.
impl<B: Body<Data = Bytes>> Drop for CapturingBody<B> {
    fn drop(&mut self) {
        let outcome = if self.ended {
            Outcome::Complete
        } else {
            Outcome::Abandoned
        };

        self.finish(outcome);
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::io;
    use std::net::SocketAddr;

    use http_body_util::{BodyExt, Empty, Full};
    use hyper::body::Bytes;
    use proxy_storage::{
        CaptureConfig, CapturedRequest, ConnectionInfo, Destination, EventPublisher,
        EventSubscriber, HttpVersion, TrafficError, TrafficErrorKind, TrafficEvent,
    };

    use super::*;
    use crate::capture::Capture;

    /// A body that hands over the frames it was given and then never ends.
    ///
    /// The stall is the point: a body that is dropped part-way is a client
    /// that went away while the response was still arriving, and there is no
    /// other way to be part-way through one.
    struct Stalling {
        frames: Vec<Bytes>,
    }

    impl Body for Stalling {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            if self.frames.is_empty() {
                return Poll::Pending;
            }

            Poll::Ready(Some(Ok(Frame::data(self.frames.remove(0)))))
        }

        fn is_end_stream(&self) -> bool {
            false
        }
    }

    /// A body that fails instead of producing a frame.
    struct Breaking;

    impl Body for Breaking {
        type Data = Bytes;
        type Error = io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
            Poll::Ready(Some(Err(io::Error::from(io::ErrorKind::ConnectionReset))))
        }

        fn is_end_stream(&self) -> bool {
            false
        }
    }

    fn client() -> SocketAddr {
        "127.0.0.1:54321".parse().expect("valid socket address")
    }

    fn capture(limit: usize) -> Capture {
        Capture::new(CaptureConfig::new(limit), EventPublisher::new())
    }

    fn recorder(capture: &Capture) -> Recorder {
        capture.begin(
            ConnectionInfo::http(client(), Some(Destination::new("example.com", 80))),
            CapturedRequest::new("POST", "http://example.com/x", HttpVersion::Http11),
        )
    }

    fn published(subscriber: &mut EventSubscriber) -> TrafficEvent {
        subscriber
            .try_recv()
            .expect("an exchange was published")
            .expect("the event stream is healthy")
    }

    /// Reads a body to the end, returning what a client would have received.
    async fn forwarded<B>(body: B) -> Vec<u8>
    where
        B: Body<Data = Bytes> + Unpin,
        B::Error: std::fmt::Debug,
    {
        body.collect()
            .await
            .expect("the body is complete")
            .to_bytes()
            .to_vec()
    }

    #[tokio::test]
    async fn a_body_is_forwarded_unchanged_and_captured() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        let body = recorder.capture_request(Full::<Bytes>::new(Bytes::from_static(b"hello")));
        assert_eq!(forwarded(body).await, b"hello");

        drop(recorder);
        assert_eq!(
            published(&mut subscriber).request.body.as_text(),
            Some("hello")
        );
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_forwarded_in_full_and_captured_in_part() {
        let capture = capture(4);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        let body = recorder.capture_request(Full::<Bytes>::new(Bytes::from_static(b"hello world")));

        // The whole body still reaches the far end.
        assert_eq!(forwarded(body).await, b"hello world");

        drop(recorder);
        let captured = published(&mut subscriber).request.body.clone();
        assert_eq!(captured.data(), b"hell");
        assert_eq!(captured.size(), 11);
        assert!(captured.is_truncated());
    }

    #[tokio::test]
    async fn binary_bytes_are_forwarded_and_captured_exactly() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);
        let bytes: Vec<u8> = (0..=255).collect();

        let body = recorder.capture_request(Full::<Bytes>::new(Bytes::from(bytes.clone())));
        assert_eq!(forwarded(body).await, bytes);

        drop(recorder);
        let captured = published(&mut subscriber).request.body.clone();
        assert_eq!(captured.data(), bytes);
        assert_eq!(captured.as_text(), None);
    }

    #[tokio::test]
    async fn an_empty_body_is_captured_as_empty() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);

        let body = recorder.capture_request(Empty::<Bytes>::new());
        assert_eq!(forwarded(body).await, b"");

        drop(recorder);
        assert!(published(&mut subscriber).request.body.is_empty());
    }

    #[tokio::test]
    async fn a_response_body_read_to_the_end_publishes_the_exchange() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);
        recorder.upstream_responded(
            &hyper::Response::builder()
                .status(200)
                .body(())
                .expect("valid response"),
        );

        let body = recorder.capture_response(Full::<Bytes>::new(Bytes::from_static(b"answered")));
        assert_eq!(forwarded(body).await, b"answered");

        let event = published(&mut subscriber);
        assert!(event.succeeded());
        assert_eq!(
            event.response.as_ref().expect("a response").body.as_text(),
            Some("answered")
        );
    }

    #[tokio::test]
    async fn an_empty_response_body_still_publishes_the_exchange() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);
        recorder.upstream_responded(
            &hyper::Response::builder()
                .status(204)
                .body(())
                .expect("valid response"),
        );

        // Never polled at all, which is what happens to a body that says up
        // front that it has nothing.
        drop(recorder.capture_response(Empty::<Bytes>::new()));

        assert!(published(&mut subscriber).succeeded());
    }

    #[tokio::test]
    async fn a_response_body_dropped_part_way_publishes_what_arrived() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);
        recorder.upstream_responded(
            &hyper::Response::builder()
                .status(200)
                .body(())
                .expect("valid response"),
        );

        let mut body = recorder.capture_response(Stalling {
            frames: vec![Bytes::from_static(b"half")],
        });

        // One frame read, then the reader goes away with the body unfinished.
        let frame = body.frame().await.expect("a frame").expect("a data frame");
        assert_eq!(
            frame.into_data().expect("data"),
            Bytes::from_static(b"half")
        );
        drop(body);

        let event = published(&mut subscriber);
        assert_eq!(
            event.error.as_ref().map(TrafficError::kind),
            Some(TrafficErrorKind::ClientDisconnected)
        );
        assert_eq!(
            event.response.as_ref().expect("a response").body.as_text(),
            Some("half")
        );
    }

    #[tokio::test]
    async fn a_response_body_that_fails_publishes_a_forwarding_failure() {
        let capture = capture(1024);
        let mut subscriber = capture.subscribe();
        let recorder = recorder(&capture);
        recorder.upstream_responded(
            &hyper::Response::builder()
                .status(200)
                .body(())
                .expect("valid response"),
        );

        let mut body = recorder.capture_response(Breaking);
        assert!(body.frame().await.expect("a frame").is_err());
        drop(body);

        assert_eq!(
            published(&mut subscriber)
                .error
                .as_ref()
                .map(TrafficError::kind),
            Some(TrafficErrorKind::ResponseForwarding)
        );
    }

    #[tokio::test]
    async fn the_framing_of_the_inner_body_is_passed_through_unchanged() {
        let capture = capture(1024);
        let recorder = recorder(&capture);

        let full = Full::<Bytes>::new(Bytes::from_static(b"hello"));
        let expected = full.size_hint();
        let body = recorder.capture_request(full);

        assert_eq!(body.size_hint().exact(), expected.exact());
        assert_eq!(body.size_hint().lower(), expected.lower());

        let empty = recorder.capture_request(Empty::<Bytes>::new());
        assert!(empty.is_end_stream());
    }
}
