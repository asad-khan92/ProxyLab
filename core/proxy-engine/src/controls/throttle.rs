//! Pretending the proxy's own link is slower than it is.
//!
//! ```text
//!  frame of 64 KiB, ceiling 32 KiB/s
//!
//!  ├─ 3 KiB ─┤ wait ├─ 3 KiB ─┤ wait ├─ 3 KiB ─┤ …
//!   ≈ 100 ms         ≈ 100 ms         ≈ 100 ms
//! ```
//!
//! Three things are shaped and nothing else is: a delay before each exchange
//! is sent, a ceiling on the request body going upstream, and a ceiling on
//! the response body coming back. The settings are
//! [`ThrottleSettings`](proxy_ipc::ThrottleSettings) and they come from the
//! UI; this module is what carries them out.
//!
//! # How a ceiling is kept
//!
//! [`Throttled`] holds each data frame back and hands it on in pieces, waiting
//! for as long as each piece would have taken at the ceiling. The piece is
//! sized from the ceiling itself — about a twentieth of a second's worth — so
//! a slow link is paced in small steps rather than in one long stall, and a
//! fast one is not chopped up more finely than it needs to be.
//!
//! Frames that are not data pass straight through. A trailer section is not
//! bandwidth in any sense worth simulating.
//!
//! # It is per body, not per proxy
//!
//! Each body is paced against the ceiling on its own, so two downloads at a
//! ceiling of 32 KiB/s get 32 KiB/s each rather than 16 KiB/s each. That is
//! deliberate: what a developer is simulating is one device's link to one
//! server, and a shared budget across every connection in the proxy would
//! make a page of images behave like nothing any real network does.
//!
//! # Every protocol, the same way
//!
//! An HTTP/1.1 response body, an HTTP/2 stream's response body, and a
//! server-sent event stream are all bodies, and all three are paced by
//! [`Throttled`]. A WebSocket is not a body — after the `101` it is two
//! streams of bytes — so it is paced by [`copy`], which is the same
//! arithmetic applied to a socket instead of to a frame.
//!
//! # Nothing is kept
//!
//! The settings live in memory for as long as the process does. See
//! [`throttle`](proxy_ipc::throttle) for why they are not persisted.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;
use std::task::{ready, Context, Poll};

use hyper::body::{Body, Bytes, Frame, SizeHint};
use proxy_ipc::{ThrottleControl, ThrottleSettings};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{sleep, Duration, Sleep};

/// How much of a second's worth of bytes is handed on at a time.
///
/// A twentieth: small enough that a slow ceiling is a steady trickle rather
/// than a stall and a burst, and large enough that a fast one is not paced
/// one packet at a time.
const PACE_DIVISOR: u64 = 20;

/// The smallest piece a frame is broken into.
///
/// Below this the waiting costs more than the bytes are worth, and a body
/// paced in 64-byte pieces would spend its life in the timer wheel.
const MIN_PACE_BYTES: u64 = 1024;

/// The largest piece a frame is broken into.
const MAX_PACE_BYTES: u64 = 64 * 1024;

/// The size of the copy buffer a throttled WebSocket tunnel uses.
///
/// The same buffer size [`tokio::io::copy_bidirectional`] uses, so a tunnel
/// that is being shaped reads the socket in the same steps as one that is
/// not.
const TUNNEL_BUFFER_BYTES: usize = 8 * 1024;

/// How the running proxy is shaping the traffic it carries.
///
/// One of these exists per process and every connection reads it. Replacing
/// the settings is one write under a lock; reading them is one read, taken
/// once per body rather than once per frame, so a body already being paced
/// keeps the ceiling it started with.
#[derive(Debug, Default)]
pub struct Throttle {
    settings: RwLock<ThrottleSettings>,
}

impl Throttle {
    /// A proxy that is shaping nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// How traffic is being shaped right now.
    ///
    /// A poisoned lock is recovered from rather than panicked on: something
    /// else has already panicked, and refusing to carry traffic over a
    /// throttling setting would turn one failure into every failure.
    pub fn settings(&self) -> ThrottleSettings {
        *self
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Shapes traffic this way from the next exchange on.
    pub fn set(&self, settings: ThrottleSettings) -> ThrottleSettings {
        *self
            .settings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings;

        settings
    }

    /// Waits for as long as this exchange is meant to take to get going.
    ///
    /// Resolves immediately when there is no delay set, which is the usual
    /// case and costs one read of the settings.
    pub async fn delay(&self) {
        if let Some(latency) = self.settings().latency() {
            sleep(Duration::from_millis(latency)).await;
        }
    }

    /// Paces `body` as a response coming back to a client.
    pub fn download<B>(&self, body: B) -> Throttled<B> {
        Throttled::new(body, self.settings().download())
    }

    /// Paces `body` as a request going upstream.
    pub fn upload<B>(&self, body: B) -> Throttled<B> {
        Throttled::new(body, self.settings().upload())
    }
}

impl ThrottleControl for Throttle {
    fn settings(&self) -> ThrottleSettings {
        Throttle::settings(self)
    }

    fn apply(&self, settings: ThrottleSettings) -> ThrottleSettings {
        self.set(settings)
    }
}

/// How big a piece to hand on at a time, at `bytes_per_second`.
fn pace_bytes(bytes_per_second: u64) -> usize {
    (bytes_per_second / PACE_DIVISOR).clamp(MIN_PACE_BYTES, MAX_PACE_BYTES) as usize
}

/// How long `bytes` would take at `bytes_per_second`.
fn pace_delay(bytes: usize, bytes_per_second: u64) -> Duration {
    Duration::from_secs_f64(bytes as f64 / bytes_per_second as f64)
}

/// A body handed on no faster than a ceiling allows.
///
/// A `rate` of `None` is no ceiling, and then this is the body it wraps with
/// one enum check in front of it. That is the shape every unthrottled
/// exchange takes, which is why there is one type here rather than two.
pub struct Throttled<B> {
    inner: B,
    /// The ceiling, in bytes per second, or `None` for no ceiling.
    rate: Option<u64>,
    /// Data read from the body and not handed on yet.
    held: Option<Bytes>,
    /// The wait before the next piece of `held` may be handed on.
    delay: Option<Pin<Box<Sleep>>>,
}

impl<B> Throttled<B> {
    /// `body`, handed on at no more than `rate` bytes per second.
    pub fn new(body: B, rate: Option<u64>) -> Self {
        Self {
            inner: body,
            rate,
            held: None,
            delay: None,
        }
    }

    /// How much of the body is held back and still to be handed on.
    fn held_len(&self) -> u64 {
        self.held.as_ref().map_or(0, |held| held.len() as u64)
    }
}

impl<B> Body for Throttled<B>
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

        let Some(rate) = this.rate else {
            return Pin::new(&mut this.inner).poll_frame(context);
        };

        // Nothing held back, so the next piece comes from the body. A frame
        // that is not data is not bandwidth, and goes straight on.
        if this.held.is_none() {
            let frame = match ready!(Pin::new(&mut this.inner).poll_frame(context)) {
                Some(Ok(frame)) => frame,
                other => return Poll::Ready(other),
            };

            match frame.into_data() {
                Ok(data) => {
                    let piece = data.len().min(pace_bytes(rate));
                    this.delay = Some(Box::pin(sleep(pace_delay(piece, rate))));
                    this.held = Some(data);
                }
                Err(frame) => return Poll::Ready(Some(Ok(frame))),
            }
        }

        // What is held has already been waited for, or is being waited for
        // now. Either way nothing leaves until the wait is over.
        if let Some(delay) = &mut this.delay {
            ready!(delay.as_mut().poll(context));
            this.delay = None;
        }

        let mut held = this.held.take().expect("a piece is being handed on");
        let piece = held.split_to(held.len().min(pace_bytes(rate)));

        if !held.is_empty() {
            this.delay = Some(Box::pin(sleep(pace_delay(
                held.len().min(pace_bytes(rate)),
                rate,
            ))));
            this.held = Some(held);
        }

        Poll::Ready(Some(Ok(Frame::data(piece))))
    }

    fn is_end_stream(&self) -> bool {
        self.held.is_none() && self.inner.is_end_stream()
    }

    /// Whatever the body says, plus what is held back: pacing must not change
    /// how long the message is.
    fn size_hint(&self) -> SizeHint {
        let held = self.held_len();
        let inner = self.inner.size_hint();

        if held == 0 {
            return inner;
        }

        let mut hint = SizeHint::new();
        hint.set_lower(inner.lower().saturating_add(held));
        if let Some(upper) = inner.upper() {
            hint.set_upper(upper.saturating_add(held));
        }

        hint
    }
}

/// The ceiling, and nothing the body is carrying.
impl<B> fmt::Debug for Throttled<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Throttled")
            .field("rate", &self.rate)
            .finish()
    }
}

pub async fn write_paced<W>(writer: &mut W, bytes: &[u8], rate: u64) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut offset = 0;
    while offset < bytes.len() {
        let n = (bytes.len() - offset).min(TUNNEL_BUFFER_BYTES);
        sleep(pace_delay(n, rate)).await;
        writer.write_all(&bytes[offset..offset + n]).await?;
        offset += n;
    }

    Ok(())
}

/// Copies one direction of a tunnel, no faster than `rate` allows.
///
/// The same arithmetic [`Throttled`] applies to a frame, applied to a socket:
/// read a bufferful, wait for as long as it would have taken, write it. What
/// this is for is a WebSocket, which stops being a body at the `101` and
/// becomes two of these.
///
/// The write half is shut down when the read half reaches its end, which is
/// what lets one side finish sending and still be sent the whole answer —
/// the half-close [`tokio::io::copy_bidirectional`] does, kept here because
/// a tunnel that is being shaped must close exactly like one that is not.
pub async fn copy<R, W>(reader: &mut R, writer: &mut W, rate: u64) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut buffer = vec![0u8; TUNNEL_BUFFER_BYTES];
    let mut copied = 0u64;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            writer.shutdown().await?;

            return Ok(copied);
        }

        sleep(pace_delay(read, rate)).await;
        writer.write_all(&buffer[..read]).await?;
        copied += read as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::{BodyExt, Full};
    use tokio::time::Instant;

    fn settings(latency_ms: u64, download: u64, upload: u64) -> ThrottleSettings {
        ThrottleSettings {
            enabled: true,
            latency_ms,
            download_bytes_per_second: download,
            upload_bytes_per_second: upload,
        }
    }

    /// Everything a body hands on, and how long it took.
    async fn drain<B>(body: B) -> (Vec<u8>, Duration)
    where
        B: Body<Data = Bytes> + Unpin,
        B::Error: fmt::Debug,
    {
        let started = Instant::now();
        let collected = body.collect().await.expect("the body completed");

        (collected.to_bytes().to_vec(), started.elapsed())
    }

    #[test]
    fn a_new_throttle_is_shaping_nothing() {
        let throttle = Throttle::new();

        assert_eq!(throttle.settings(), ThrottleSettings::none());
        assert!(!throttle.settings().is_shaping());
    }

    #[test]
    fn settings_replace_the_ones_before_them() {
        let throttle = Throttle::new();

        assert_eq!(
            throttle.set(settings(50, 1024, 512)),
            settings(50, 1024, 512)
        );
        assert_eq!(throttle.settings().download(), Some(1024));
        assert_eq!(throttle.settings().upload(), Some(512));
    }

    #[test]
    fn a_piece_is_about_a_twentieth_of_a_seconds_worth() {
        assert_eq!(pace_bytes(20 * 32_768), 32_768);
        // Clamped at both ends, so neither a crawl nor a firehose is paced
        // in a size that makes no sense.
        assert_eq!(pace_bytes(256), MIN_PACE_BYTES as usize);
        assert_eq!(pace_bytes(100 * 1024 * 1024), MAX_PACE_BYTES as usize);
    }

    #[tokio::test]
    async fn no_ceiling_hands_a_body_straight_on() {
        let body = Throttled::new(
            Full::<Bytes>::new(Bytes::from_static(&[b'x'; 65_536])),
            None,
        );
        let (data, took) = drain(body).await;

        assert_eq!(data.len(), 65_536);
        assert!(took < Duration::from_millis(200), "{took:?}");
    }

    #[tokio::test]
    async fn a_ceiling_makes_a_body_take_about_as_long_as_it_should() {
        // 8 KiB at 8 KiB/s is a second's worth, paced in twentieths.
        let body = Throttled::new(
            Full::<Bytes>::new(Bytes::from_static(&[b'x'; 8_192])),
            Some(8_192),
        );
        let (data, took) = drain(body).await;

        assert_eq!(data.len(), 8_192);
        assert!(took >= Duration::from_millis(800), "{took:?}");
        assert!(took < Duration::from_millis(3_000), "{took:?}");
    }

    #[tokio::test]
    async fn a_paced_body_hands_on_every_byte_it_was_given() {
        let original: Vec<u8> = (0..40_000u32).map(|byte| byte as u8).collect();
        let body = Throttled::new(
            Full::<Bytes>::new(Bytes::from(original.clone())),
            Some(1024 * 1024),
        );
        let (data, _) = drain(body).await;

        assert_eq!(data, original);
    }

    #[tokio::test]
    async fn a_paced_body_still_knows_how_long_it_is() {
        let body = Throttled::new(
            Full::<Bytes>::new(Bytes::from_static(&[b'x'; 4_096])),
            Some(4_096),
        );

        assert_eq!(body.size_hint().exact(), Some(4_096));
    }

    #[tokio::test]
    async fn no_delay_is_no_wait() {
        let throttle = Throttle::new();
        throttle.set(settings(0, 1024, 1024));

        let started = Instant::now();
        throttle.delay().await;

        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn a_delay_is_waited_for_once() {
        let throttle = Throttle::new();
        throttle.set(settings(150, 0, 0));

        let started = Instant::now();
        throttle.delay().await;

        assert!(started.elapsed() >= Duration::from_millis(140));
    }

    #[tokio::test]
    async fn a_delay_that_is_switched_off_is_not_waited_for() {
        let throttle = Throttle::new();
        throttle.set(ThrottleSettings {
            enabled: false,
            ..settings(5_000, 0, 0)
        });

        let started = Instant::now();
        throttle.delay().await;

        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn a_throttled_copy_carries_every_byte_and_closes_the_far_end() {
        let (mut source, mut sink) = tokio::io::duplex(64 * 1024);
        let (mut reader, mut writer) = tokio::io::duplex(64 * 1024);

        let sent = b"the whole of it".to_vec();
        source
            .write_all(&sent)
            .await
            .expect("the test socket accepts it");
        source.shutdown().await.expect("the test socket closes");

        let copied = copy(&mut sink, &mut writer, 1024 * 1024)
            .await
            .expect("the copy finished");

        let mut received = Vec::new();
        reader
            .read_to_end(&mut received)
            .await
            .expect("the far end reads to the close");

        assert_eq!(copied, sent.len() as u64);
        assert_eq!(received, sent);
    }

    #[test]
    fn a_throttled_body_never_renders_what_it_is_carrying() {
        let body = Throttled::new(
            Full::<Bytes>::new(Bytes::from_static(b"hunter2")),
            Some(512),
        );
        let rendered = format!("{body:?}");

        assert!(!rendered.contains("hunter2"), "{rendered}");
    }
}
