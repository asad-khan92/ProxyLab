//! Speaking HTTP/2 to a client, on a connection ALPN settled on.
//!
//! One call to [`serve`] is one HTTP/2 connection. The service it is given is
//! called once per stream, concurrently, and each call is an exchange of its
//! own — so what a caller writes is a function from one request to one
//! response, exactly as it writes for HTTP/1.1, and multiplexing is something
//! it never has to think about.
//!
//! Two things are different from the HTTP/1.1 side, and both are the
//! protocol's doing rather than a choice made here.
//!
//! The connection outlives its exchanges. It stays open for as long as the
//! client wants it, which is why [`serve`] takes a [`Shutdown`]: when the
//! proxy stops, the connection is told, and it finishes the streams it is
//! carrying and starts no more. Without that, stopping the proxy would mean
//! waiting for a client that has no reason to hang up.
//!
//! A response must not carry the headers that describe a connection. The
//! proxy writes `Connection: close` on the responses it generates, because on
//! HTTP/1.1 that is true and worth saying; in HTTP/2 it is forbidden, and the
//! same is true of `Transfer-Encoding`, `Keep-Alive` and `Upgrade`. Rather
//! than teach every place that builds a response which protocol it will
//! travel on, the responses are put through [`for_http2`] here, on their way
//! out — one place, using the proxy's existing header hygiene.

use std::error::Error as StdError;
use std::future::Future;

use hyper::body::{Body, Incoming};
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper::{Request, Response, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::http1::headers;
use crate::shutdown::Shutdown;

/// Serves HTTP/2 on `stream` until the client is finished or the proxy stops.
///
/// `exchange` is called once per stream and must not fail: every request gets
/// an answer, so a request that cannot be carried is answered with the error
/// response saying so rather than by resetting the stream. That is what keeps
/// one stream's trouble to itself — a blocked request, an origin that would
/// not answer, and a request that was carried are three streams that can
/// happily share a connection.
///
/// Returns when the connection is over, with whatever ended it.
pub async fn serve<S, E, F, B>(
    stream: S,
    shutdown: &Shutdown,
    exchange: E,
) -> Result<(), hyper::Error>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    E: Fn(Request<Incoming>) -> F + Send + 'static,
    F: Future<Output = Response<B>> + Send + 'static,
    B: Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    let service = service_fn(move |request| {
        let answering = exchange(request);

        async move { Ok::<_, std::convert::Infallible>(for_http2(answering.await)) }
    });

    let connection =
        http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);

    // Whichever comes first. A connection that ends on its own is the usual
    // case; one that is still going when the proxy stops is told to wind
    // down, and then awaited — `graceful_shutdown` lets the streams already
    // in flight finish, so a response half-written is not a response cut off.
    tokio::select! {
        ended = &mut connection => return ended,
        () = shutdown.signalled() => {}
    }

    connection.as_mut().graceful_shutdown();

    connection.await
}

/// Prepares a response the proxy built for HTTP/1.1 to travel on HTTP/2.
///
/// The headers that describe a connection are removed — they are illegal in
/// HTTP/2, where the frame types carry what they used to say — using the
/// proxy's own [`headers::strip_hop_by_hop`], so there is one list of what
/// must not cross a hop rather than a second one written out here. The
/// version is corrected to match the connection the response is going out on.
///
/// Nothing about the message itself is touched: the status, the end-to-end
/// headers, and the body are what they were.
fn for_http2<B>(mut response: Response<B>) -> Response<B> {
    headers::strip_hop_by_hop(response.headers_mut());
    *response.version_mut() = Version::HTTP_2;

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::Empty;
    use hyper::body::Bytes;
    use hyper::header::{HeaderValue, CONNECTION, CONTENT_TYPE, TRANSFER_ENCODING};
    use hyper::StatusCode;

    fn response(headers: &[(&str, &str)]) -> Response<Empty<Bytes>> {
        let mut response = Response::new(Empty::new());

        for (name, value) in headers {
            response.headers_mut().insert(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }

        response
    }

    #[test]
    fn the_headers_that_describe_a_connection_do_not_reach_an_http_2_client() {
        let prepared = for_http2(response(&[
            ("connection", "close"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
        ]));

        assert!(prepared.headers().get(CONNECTION).is_none());
        assert!(prepared.headers().get("keep-alive").is_none());
        assert!(prepared.headers().get(TRANSFER_ENCODING).is_none());
        assert!(prepared.headers().get("upgrade").is_none());
    }

    #[test]
    fn the_headers_the_message_is_made_of_are_left_alone() {
        let prepared = for_http2(response(&[
            ("connection", "close"),
            ("content-type", "application/json"),
            ("content-length", "12"),
            ("set-cookie", "a=1"),
        ]));

        assert_eq!(prepared.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(prepared.headers()["content-length"], "12");
        assert_eq!(prepared.headers()["set-cookie"], "a=1");
    }

    #[test]
    fn a_response_going_out_on_http_2_says_so() {
        let prepared = for_http2(response(&[]));

        assert_eq!(prepared.version(), Version::HTTP_2);
    }

    #[test]
    fn the_status_is_the_one_the_proxy_decided_on() {
        let mut built = response(&[("connection", "close")]);
        *built.status_mut() = StatusCode::FORBIDDEN;

        assert_eq!(for_http2(built).status(), StatusCode::FORBIDDEN);
    }
}
