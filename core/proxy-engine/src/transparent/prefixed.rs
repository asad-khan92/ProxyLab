//! A stream with bytes already read off the front of it put back.
//!
//! The bridge has to look at a flow's first byte to know whether it is TLS,
//! and whatever reads the flow next — a TLS handshake, or an HTTP parser —
//! has to see that byte too. [`Prefixed`] hands the bytes that were read
//! back first and then reads the stream itself, so nothing downstream knows
//! a byte was ever looked at. Writing is untouched.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// `inner`, with `prefix` read before it.
#[derive(Debug)]
pub struct Prefixed<S> {
    prefix: Vec<u8>,
    position: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            position: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let remaining = &this.prefix[this.position..];

        if remaining.is_empty() {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }

        let taken = remaining.len().min(buf.remaining());
        buf.put_slice(&remaining[..taken]);
        this.position += taken;

        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn the_prefix_is_read_before_the_stream() {
        let (mut near, far) = tokio::io::duplex(64);
        near.write_all(b"ello").await.expect("write");
        drop(near);

        let mut read = Vec::new();
        Prefixed::new(b"h".to_vec(), far)
            .read_to_end(&mut read)
            .await
            .expect("read");

        assert_eq!(read, b"hello");
    }

    #[tokio::test]
    async fn a_small_buffer_takes_the_prefix_a_piece_at_a_time() {
        let (near, far) = tokio::io::duplex(64);
        drop(near);
        let mut stream = Prefixed::new(b"abc".to_vec(), far);

        let mut one = [0u8; 1];
        for expected in *b"abc" {
            stream.read_exact(&mut one).await.expect("read");
            assert_eq!(one[0], expected);
        }
        assert_eq!(stream.read(&mut one).await.expect("read"), 0);
    }

    #[tokio::test]
    async fn writing_goes_straight_to_the_stream() {
        let (mut near, far) = tokio::io::duplex(64);
        let mut stream = Prefixed::new(b"ignored".to_vec(), far);

        stream.write_all(b"out").await.expect("write");
        let mut read = [0u8; 3];
        near.read_exact(&mut read).await.expect("read");

        assert_eq!(&read, b"out");
    }
}
