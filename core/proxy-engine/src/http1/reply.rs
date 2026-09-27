//! The responses the proxy writes itself.
//!
//! Most of what a client gets back came from an origin server. These are the
//! rest: the answer to a request that could not be carried, and the answer
//! that opens an intercepted connection. They are here rather than beside
//! either caller because a plain HTTP request and an intercepted HTTPS
//! request fail in the same ways, and should not fail in two different
//! wordings.
//!
//! What a client is told is deliberately coarse. A status and a short reason,
//! never the underlying error: whether an origin refused the connection,
//! could not be resolved, or presented a certificate that did not verify is
//! a matter for the log, not for the response.

use std::convert::Infallible;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Response, StatusCode};

use super::headers;

/// Body of a response the proxy sends to a client.
///
/// Boxed because it is either the origin's body, still streaming, or a short
/// body the proxy generated itself.
pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

/// Wraps text the proxy generated in the response body type.
pub fn text(text: String) -> ProxyBody {
    Full::new(Bytes::from(text))
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// Wraps bytes the proxy read from a file in the response body type.
pub fn bytes(bytes: Vec<u8>) -> ProxyBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// A body with nothing in it and no framing of its own.
pub fn empty() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// Builds the response for a request the proxy would not carry.
///
/// The body repeats the status and `reason`, so a client that shows a body
/// says something useful. The connection is declared finished, because it is.
pub fn error(status: StatusCode, reason: &str) -> Response<ProxyBody> {
    let mut response = Response::new(text(format!("{status}: {reason}\n")));

    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers::set_close(response.headers_mut());

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use hyper::header::CONNECTION;

    async fn body_of(response: Response<ProxyBody>) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("generated body is complete")
            .to_bytes();

        String::from_utf8(bytes.to_vec()).expect("generated body is text")
    }

    #[tokio::test]
    async fn text_becomes_a_body() {
        assert_eq!(
            body_of(Response::new(text("hello".to_owned()))).await,
            "hello"
        );
    }

    #[tokio::test]
    async fn an_empty_body_is_empty() {
        assert_eq!(body_of(Response::new(empty())).await, "");
    }

    #[tokio::test]
    async fn an_error_states_the_status_and_the_reason() {
        let response = error(StatusCode::BAD_GATEWAY, "could not reach the origin");

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            body_of(response).await,
            "502 Bad Gateway: could not reach the origin\n"
        );
    }

    #[test]
    fn an_error_closes_the_client_connection() {
        let response = error(StatusCode::BAD_REQUEST, "no");

        assert_eq!(response.headers()[CONNECTION], "close");
    }
}
