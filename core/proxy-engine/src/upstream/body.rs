//! The bodies that cross the pool.
//!
//! A pooled connection outlives the request that opened it, so it cannot be
//! typed by that request's body: one connection carries a forwarded body one
//! minute and a replayed one the next. Both directions are therefore given a
//! single type here.
//!
//! ```text
//! request  ──▶ UpstreamBody   one boxed body type, whatever produced it
//! response ──▶ PooledBody     the origin's body, plus what to do when it ends
//! ```
//!
//! [`UpstreamBody`] is a box, and a box only: the body inside it is the
//! capture's own wrapper around whatever the client sent, so nothing is
//! copied, buffered, or read early to put it in one.
//!
//! [`PooledBody`] is where connection reuse is actually decided. A connection
//! goes back to the pool when its response body has been read to the end and
//! not a moment sooner — see below.

use std::error::Error as StdError;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};

use super::pool::Reservation;

/// The error any body the proxy sends upstream may fail with.
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// The body an upstream request is sent with.
///
/// One type for every caller, so a pooled connection is not tied to whichever
/// request happened to open it.
pub type UpstreamBody = BoxBody<Bytes, BoxError>;

/// Puts `body` in the type upstream connections carry.
pub fn boxed<B>(body: B) -> UpstreamBody
where
    B: Body<Data = Bytes> + Send + Sync + 'static,
    B::Error: Into<BoxError>,
{
    body.map_err(Into::into).boxed()
}

/// An origin's response body, which decides its connection's fate as it ends.
///
/// The connection this body came in on is held here, not in the pool, for as
/// long as the body is still arriving. That is the whole point: an HTTP/1.1
/// connection is not free once the response *head* has been read — the body
/// is still being framed on it — so returning it to the pool then would hand
/// a second request a connection with a first response still coming down it.
///
/// So the connection goes back when, and only when, one of these happens:
///
/// ```text
/// body ends                       ──▶ returned, if it may be reused
/// body fails                      ──▶ discarded
/// body dropped before it ends     ──▶ discarded
/// response said Connection: close ──▶ discarded (never reserved at all)
/// ```
///
/// A body with a declared length ends by having nothing left rather than by
/// being polled once more, and whoever is forwarding it may stop polling as
/// soon as it says so — so the end is noticed on the frame that reaches it,
/// exactly as [`CapturingBody`](crate::capture::body::CapturingBody) does.
///
/// An HTTP/2 response body carries no reservation: nothing is returned to a
/// pool, because an HTTP/2 connection was never taken out of one.
pub struct PooledBody {
    inner: Incoming,
    /// The connection to give back, until it has been given back or given up.
    reservation: Option<Reservation>,
}

impl PooledBody {
    /// A body whose end returns `reservation`'s connection to the pool.
    pub(super) fn reserving(inner: Incoming, reservation: Reservation) -> Self {
        Self {
            inner,
            reservation: Some(reservation),
        }
    }

    /// A body with no connection to give back.
    pub(super) fn plain(inner: Incoming) -> Self {
        Self {
            inner,
            reservation: None,
        }
    }

    /// Gives the connection back, if it is still reusable.
    fn release(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            reservation.release();
        }
    }

    /// Gives the connection up: it is not fit for another request.
    fn discard(&mut self) {
        self.reservation = None;
    }
}

impl Body for PooledBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match Pin::new(&mut this.inner).poll_frame(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(frame))) => {
                if this.inner.is_end_stream() {
                    this.release();
                }

                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                // The body broke part-way. Whatever is left on the connection
                // is the rest of a response nobody read, so it is not a
                // connection another request may have.
                this.discard();

                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.release();

                Poll::Ready(None)
            }
        }
    }

    /// Whatever the inner body says: pooling must not change the framing.
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    /// Whatever the inner body says, so a `Content-Length` still survives the
    /// proxy.
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Whether a connection is still owed back, and nothing of the response.
///
/// A body is the one thing in an exchange most likely to be carrying
/// something private, so it is not described here even in outline.
impl fmt::Debug for PooledBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledBody")
            .field("reserved", &self.reservation.is_some())
            .finish()
    }
}

/// A body dropped before it ended leaves an unread response on the wire, so
/// the connection it came in on is dropped with it rather than pooled.
impl Drop for PooledBody {
    fn drop(&mut self) {
        self.discard();
    }
}
