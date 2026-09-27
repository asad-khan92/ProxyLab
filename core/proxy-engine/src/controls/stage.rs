//! Reading the start of a body without stopping the rest of it arriving.
//!
//! A breakpoint has to show a developer what a body says, and a body is a
//! stream. The two are reconciled here:
//!
//! ```text
//!              ┌──────── read up front ────────┐
//! body ──────▶ │ frame  frame  frame │ rest… ──┴──▶ still streaming
//!              └─────────────────────┘
//!                shown, and editable
//!                when there is no rest
//! ```
//!
//! [`StagedBody`] holds the frames that were read and the body they were read
//! from, and hands both on in order — so staging a body changes what a client
//! or an origin receives in no way at all. What it changes is that ProxyLab
//! has seen the first part of it.
//!
//! # Why it stops
//!
//! Two limits, and either one ends the reading:
//!
//! | Limit | What it is for |
//! | --- | --- |
//! | bytes | A body may be a 4 GB download. ProxyLab reads the first part. |
//! | time | A body may never end at all. An SSE stream is exactly that. |
//!
//! The time limit is what makes a response breakpoint safe to put on an event
//! stream: reading stops when the window passes, the pause appears with
//! whatever had arrived, and the rest of the stream is delivered after it
//! rather than waited for.
//!
//! # Editing is only offered for a body that ended
//!
//! A body that hit either limit is *incomplete*, and incomplete is the whole
//! of why an edit is refused for it: the bytes a client was shown are not all
//! the bytes there are, so replacing them would be silently throwing away the
//! rest. Continuing without a body edit forwards everything, in full and
//! still streaming.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame, SizeHint};
use tokio::time::{timeout_at, Duration, Instant};

/// A body whose first frames have already been read.
///
/// Wrapping a body in one is what a paused exchange does to it, and it is
/// also what every unpaused exchange on the same path gets: the frames are
/// empty and the whole body is still in `rest`, which costs one enum check
/// per frame and nothing else.
pub struct StagedBody<B: Body> {
    /// What has been read, in the order it was read, ready to be handed on.
    frames: VecDeque<Frame<Bytes>>,
    /// The failure the body produced while it was being read, if it did.
    ///
    /// Boxed so that a staged body is `Unpin` whatever the body inside it
    /// fails with. A `Box` is `Unpin` however its contents are, and nothing
    /// else about this type needs pinning.
    error: Option<Box<B::Error>>,
    /// What has not been read, or `None` for a body that ended.
    rest: Option<B>,
}

impl<B> StagedBody<B>
where
    B: Body<Data = Bytes> + Unpin,
{
    /// A body with nothing read from it yet.
    pub fn unread(body: B) -> Self {
        Self {
            frames: VecDeque::new(),
            error: None,
            rest: Some(body),
        }
    }

    /// Reads `body` until it ends, until `limit` bytes of it have arrived, or
    /// until `window` has passed — whichever happens first.
    pub async fn read(mut body: B, limit: usize, window: Duration) -> Self {
        let deadline = Instant::now() + window;
        let mut frames = VecDeque::new();
        let mut read = 0usize;

        loop {
            if read >= limit {
                return Self {
                    frames,
                    error: None,
                    rest: Some(body),
                };
            }

            match timeout_at(deadline, body.frame()).await {
                // The window passed. What arrived is shown; the rest is
                // delivered after it, in full.
                Err(_) => {
                    return Self {
                        frames,
                        error: None,
                        rest: Some(body),
                    }
                }
                Ok(None) => {
                    return Self {
                        frames,
                        error: None,
                        rest: None,
                    }
                }
                Ok(Some(Err(error))) => {
                    return Self {
                        frames,
                        error: Some(Box::new(error)),
                        rest: None,
                    }
                }
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        read += data.len();
                    }
                    frames.push_back(frame);
                }
            }
        }
    }

    /// Reads on from wherever this body was left, on the same terms as
    /// [`read`](Self::read): until it ends, until `limit` bytes are staged,
    /// or until `window` has passed.
    ///
    /// For a body that was wrapped unread and then turns out to be needed —
    /// a Modify rule's JSON edit, which cannot be applied to half a document.
    /// A body that already ended, or already failed, is left as it is.
    pub async fn read_more(&mut self, limit: usize, window: Duration) {
        let deadline = Instant::now() + window;
        let mut read = usize::try_from(self.staged_len()).unwrap_or(usize::MAX);

        while read < limit && self.error.is_none() {
            let Some(rest) = self.rest.as_mut() else {
                return;
            };

            match timeout_at(deadline, rest.frame()).await {
                Err(_) => return,
                Ok(None) => self.rest = None,
                Ok(Some(Err(error))) => {
                    self.error = Some(Box::new(error));
                    self.rest = None;
                }
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        read += data.len();
                    }
                    self.frames.push_back(frame);
                }
            }
        }
    }

    /// The bytes that were read, concatenated.
    ///
    /// The first [`limit`](Self::read) bytes of the body, give or take the
    /// frame the limit was reached in, which is kept whole.
    pub fn data(&self) -> Vec<u8> {
        let mut data = Vec::new();

        for frame in &self.frames {
            if let Some(chunk) = frame.data_ref() {
                data.extend_from_slice(chunk);
            }
        }

        data
    }

    /// Whether the whole body was read, so what was read is all there is.
    pub fn is_complete(&self) -> bool {
        self.rest.is_none() && self.error.is_none()
    }

    /// Replaces the body with `bytes`.
    ///
    /// Only meaningful for a body that ended — see the module documentation —
    /// and the caller checks that before offering the edit. Frames that are
    /// not data are kept and still come last, so a message that carried
    /// trailers still carries them.
    pub fn replace(&mut self, bytes: Vec<u8>) {
        let trailers: VecDeque<Frame<Bytes>> = self
            .frames
            .drain(..)
            .filter(|frame| frame.data_ref().is_none())
            .collect();

        self.frames = trailers;
        if !bytes.is_empty() {
            self.frames.push_front(Frame::data(Bytes::from(bytes)));
        }
        self.rest = None;
    }

    /// How many bytes of body are staged and still to be handed on.
    fn staged_len(&self) -> u64 {
        self.frames
            .iter()
            .filter_map(Frame::data_ref)
            .map(|data| data.len() as u64)
            .sum()
    }
}

impl<B> Body for StagedBody<B>
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

        if let Some(frame) = this.frames.pop_front() {
            return Poll::Ready(Some(Ok(frame)));
        }
        // After everything that was read, because that is when it happened.
        if let Some(error) = this.error.take() {
            return Poll::Ready(Some(Err(*error)));
        }

        match &mut this.rest {
            Some(rest) => Pin::new(rest).poll_frame(context),
            None => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
            && self.error.is_none()
            && self.rest.as_ref().is_none_or(Body::is_end_stream)
    }

    /// The staged bytes plus whatever the rest of the body says about itself,
    /// so a `Content-Length` still survives being staged.
    fn size_hint(&self) -> SizeHint {
        let staged = self.staged_len();

        let Some(rest) = &self.rest else {
            return SizeHint::with_exact(staged);
        };

        let inner = rest.size_hint();
        let mut hint = SizeHint::new();
        hint.set_lower(inner.lower().saturating_add(staged));
        if let Some(upper) = inner.upper() {
            hint.set_upper(upper.saturating_add(staged));
        }

        hint
    }
}

/// Whether anything is still to come, and how much was read. Never the bytes:
/// a staged body is exactly where a password is.
impl<B: Body> std::fmt::Debug for StagedBody<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedBody")
            .field("frames", &self.frames.len())
            .field("complete", &(self.rest.is_none() && self.error.is_none()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::convert::Infallible;

    use http_body_util::Full;
    use tokio::sync::mpsc;

    /// Long enough that nothing in these tests reaches it by accident.
    const WINDOW: Duration = Duration::from_secs(5);

    /// A body whose frames arrive only when a test sends them.
    ///
    /// The one thing a test needs that no body in the tree provides: a body
    /// that is still open and has nothing to hand over, which is what the
    /// time limit exists for.
    struct Trickle {
        frames: mpsc::Receiver<Frame<Bytes>>,
    }

    impl Body for Trickle {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            self.get_mut().frames.poll_recv(context).map(|f| f.map(Ok))
        }
    }

    /// A body that arrives all at once.
    fn whole(bytes: &[u8]) -> Full<Bytes> {
        Full::new(Bytes::copy_from_slice(bytes))
    }

    /// Everything a body hands on, in order.
    async fn drain<B>(body: B) -> Vec<u8>
    where
        B: Body<Data = Bytes> + Unpin,
        B::Error: std::fmt::Debug,
    {
        body.collect()
            .await
            .expect("the body completed")
            .to_bytes()
            .to_vec()
    }

    /// A body whose frames arrive when the test says so.
    fn trickle() -> (mpsc::Sender<Frame<Bytes>>, Trickle) {
        let (sender, frames) = mpsc::channel(8);

        (sender, Trickle { frames })
    }

    #[tokio::test]
    async fn a_body_that_fits_is_read_whole() {
        let staged = StagedBody::read(whole(b"hello"), 32, WINDOW).await;

        assert!(staged.is_complete());
        assert_eq!(staged.data(), b"hello");
        assert_eq!(drain(staged).await, b"hello");
    }

    #[tokio::test]
    async fn a_body_longer_than_the_limit_is_read_as_far_as_the_limit() {
        let staged = StagedBody::read(whole(&[b'x'; 100]), 10, WINDOW).await;

        // The frame the limit was reached in is kept whole, so what was read
        // is the first frame and not exactly ten bytes.
        assert!(!staged.is_complete());
        assert_eq!(staged.data().len(), 100);
    }

    #[tokio::test]
    async fn everything_read_is_still_handed_on_in_order() {
        let (sender, body) = trickle();
        for chunk in [&b"one"[..], &b"two"[..], &b"three"[..]] {
            sender
                .send(Frame::data(Bytes::copy_from_slice(chunk)))
                .await
                .expect("the body accepts frames");
        }
        drop(sender);

        let staged = StagedBody::read(body, 4, WINDOW).await;

        assert!(!staged.is_complete());
        assert_eq!(drain(staged).await, b"onetwothree");
    }

    #[tokio::test]
    async fn a_body_that_stops_arriving_is_staged_as_far_as_it_got() {
        let (sender, body) = trickle();
        sender
            .send(Frame::data(Bytes::from_static(b"first")))
            .await
            .expect("the body accepts frames");

        let staged = StagedBody::read(body, 1024, Duration::from_millis(50)).await;

        assert!(!staged.is_complete());
        assert_eq!(staged.data(), b"first");

        sender
            .send(Frame::data(Bytes::from_static(b"second")))
            .await
            .expect("the body accepts frames");
        drop(sender);

        assert_eq!(drain(staged).await, b"firstsecond");
    }

    #[tokio::test]
    async fn an_unread_body_is_handed_on_untouched() {
        let staged = StagedBody::unread(whole(b"nothing was read"));

        assert!(!staged.is_complete());
        assert_eq!(staged.data(), b"");
        assert_eq!(drain(staged).await, b"nothing was read");
    }

    #[tokio::test]
    async fn a_replaced_body_is_the_only_thing_handed_on() {
        let mut staged = StagedBody::read(whole(b"original"), 1024, WINDOW).await;
        staged.replace(b"edited".to_vec());

        assert_eq!(staged.size_hint().exact(), Some(6));
        assert_eq!(drain(staged).await, b"edited");
    }

    #[tokio::test]
    async fn a_body_replaced_with_nothing_hands_on_nothing() {
        let mut staged = StagedBody::read(whole(b"original"), 1024, WINDOW).await;
        staged.replace(Vec::new());

        assert_eq!(staged.size_hint().exact(), Some(0));
        assert_eq!(drain(staged).await, b"");
    }

    #[tokio::test]
    async fn a_replaced_body_still_carries_the_trailers_it_had() {
        let (sender, body) = trickle();
        let mut trailers = hyper::header::HeaderMap::new();
        trailers.insert("x-checksum", "abc".parse().expect("a valid value"));
        sender
            .send(Frame::data(Bytes::from_static(b"original")))
            .await
            .expect("the body accepts frames");
        sender
            .send(Frame::trailers(trailers))
            .await
            .expect("the body accepts frames");
        drop(sender);

        let mut staged = StagedBody::read(body, 1024, WINDOW).await;
        assert!(staged.is_complete());
        staged.replace(b"edited".to_vec());

        let collected = staged.collect().await.expect("the body completed");
        assert!(collected.trailers().is_some());
    }

    #[tokio::test]
    async fn a_staged_body_still_knows_how_long_it_is() {
        let staged = StagedBody::read(whole(&[b'x'; 100]), 10, WINDOW).await;

        assert_eq!(staged.size_hint().exact(), Some(100));
    }

    #[test]
    fn a_staged_body_never_renders_what_it_is_carrying() {
        let staged = StagedBody::unread(whole(b"hunter2"));
        let rendered = format!("{staged:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
    }
}
