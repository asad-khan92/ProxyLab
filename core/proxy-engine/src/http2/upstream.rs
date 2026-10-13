//! The HTTP/2 conversation with an origin server.
//!
//! The mirror of [`crate::http1::upstream`], and it is deliberately the same
//! shape: hand it a stream the origin is reachable over, get back a sender,
//! send requests on the sender and read the responses as they stream. What
//! differs is what the sender is worth.
//!
//! An HTTP/1.1 sender carries one request at a time, so an intercepted
//! connection's sender has to be taken in turn. An HTTP/2 sender does not:
//! it is a handle onto a multiplexed connection, cloning it is cheap, and
//! several clones can have requests in flight at once. That is why nothing
//! here locks anything — see [`crate::origin`], which is where the two are
//! told apart.
//!
//! The connection itself is driven by a task, exactly as the HTTP/1.1 one is,
//! because it has to keep running while bodies are still arriving. For
//! HTTP/2 it also has to keep running between requests: it is the thing that
//! answers the origin's pings and its window updates.

use std::error::Error as StdError;

use hyper::body::Body;
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::http1::upstream::UpstreamError;

/// Starts an HTTP/2 conversation over `stream` and returns its sender.
///
/// `stream` is whatever the origin is reachable over — in practice the TLS
/// session whose ALPN said `h2`, because that is the only way ProxyLab
/// arrives at HTTP/2.
///
/// Flow control, header compression, and the connection preface are the HTTP
/// library's. Nothing is configured here: the defaults are the ones hyper
/// ships, including the window sizes that bound how much an origin can send
/// before the proxy has read it.
pub async fn connection<S, B>(stream: S) -> Result<SendRequest<B>, UpstreamError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B: Body + Send + Unpin + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    let (sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .map_err(UpstreamError::Handshake)?;

    // The connection outlives this function for as long as any stream on it
    // is still going, so it runs in its own task. It ends once every sender
    // has been dropped and the streams they opened are finished.
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            proxy_diagnostics::warn(
                "upstream",
                "connect",
                "connection",
                &format!("upstream HTTP/2 connection error: {error}"),
            );
        }
    });

    Ok(sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::Request;
    use tokio::net::{TcpListener, TcpStream};

    /// An origin that accepts the connection and then says nothing at all.
    ///
    /// An HTTP/2 handshake is not a round trip — the preface and the settings
    /// are written and the sender is usable at once — so this is the far end
    /// against which a *request* fails rather than the handshake.
    async fn silent_origin() -> SocketAddrPair {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                // Held, so the connection stays open and unanswered.
                let _ = stream.readable().await;
            }
        });

        SocketAddrPair { addr, _task: task }
    }

    struct SocketAddrPair {
        addr: std::net::SocketAddr,
        _task: tokio::task::JoinHandle<()>,
    }

    fn request() -> Request<Empty<Bytes>> {
        Request::builder()
            .uri("https://example.com/")
            .body(Empty::new())
            .expect("valid request")
    }

    #[tokio::test]
    async fn an_origin_that_answers_with_nothing_is_a_send_failure() {
        let origin = silent_origin().await;
        let stream = TcpStream::connect(origin.addr)
            .await
            .expect("connect to the origin");

        let mut sender = connection::<_, Empty<Bytes>>(stream)
            .await
            .expect("the preface is written");

        let error = sender
            .send_request(request())
            .await
            .expect_err("the origin never answers");

        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn the_sender_can_be_cloned_for_each_stream() {
        let origin = silent_origin().await;
        let stream = TcpStream::connect(origin.addr)
            .await
            .expect("connect to the origin");

        let sender = connection::<_, Empty<Bytes>>(stream)
            .await
            .expect("the preface is written");

        // Cloning is what lets streams run side by side: each has a sender of
        // its own and none of them waits for another.
        let clones: Vec<_> = (0..4).map(|_| sender.clone()).collect();

        assert_eq!(clones.len(), 4);
    }
}
