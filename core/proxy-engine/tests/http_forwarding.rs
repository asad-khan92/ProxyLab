//! End-to-end tests for HTTP/1.1 forward proxying.
//!
//! Every test runs a proxy and an origin server in this process on
//! OS-assigned ports. Nothing here touches the network, DNS, or any external
//! service, and no test needs the proxy's production port.

mod support;

use support::{request, request_with_body, send_raw, Origin, Proxy, RawResponse};

use tokio::net::TcpListener;

/// A plain `200 OK` with a body of known length.
const HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";

#[tokio::test]
async fn a_get_request_is_forwarded_and_its_response_comes_back() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.body_text(), "hello");

    let forwarded = origin.only_request();
    assert_eq!(forwarded.method, "GET");
    assert_eq!(forwarded.version, "HTTP/1.1");

    proxy.stop().await;
}

#[tokio::test]
async fn the_absolute_url_reaches_the_origin_in_origin_form() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/api/users", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.only_request().target, "/api/users");

    proxy.stop().await;
}

#[tokio::test]
async fn a_post_body_is_forwarded_to_the_origin() {
    let origin =
        Origin::responding_with(b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\n\r\ncreated").await;
    let proxy = Proxy::start().await;

    let response = request_with_body(
        proxy.addr(),
        "POST",
        &format!("{}/users", origin.base_url()),
        &[("Content-Type", "application/json")],
        r#"{"name":"ada"}"#,
    )
    .await;

    assert_eq!(response.status, 201);
    assert_eq!(response.body_text(), "created");

    let forwarded = origin.only_request();
    assert_eq!(forwarded.method, "POST");
    assert_eq!(forwarded.body_text(), r#"{"name":"ada"}"#);
    assert_eq!(forwarded.header("content-type"), Some("application/json"));
    assert_eq!(forwarded.header("content-length"), Some("14"));

    proxy.stop().await;
}

#[tokio::test]
async fn query_parameters_survive_the_rewrite() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/search?q=rust&page=2&empty=", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(origin.only_request().target, "/search?q=rust&page=2&empty=");

    proxy.stop().await;
}

#[tokio::test]
async fn request_headers_reach_the_origin() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[
            ("X-Request-Id", "abc-123"),
            ("Accept", "application/json"),
            ("User-Agent", "proxylab-tests/1.0"),
        ],
    )
    .await;

    assert_eq!(response.status, 200);

    let forwarded = origin.only_request();
    assert_eq!(forwarded.header("x-request-id"), Some("abc-123"));
    assert_eq!(forwarded.header("accept"), Some("application/json"));
    assert_eq!(forwarded.header("user-agent"), Some("proxylab-tests/1.0"));

    proxy.stop().await;
}

#[tokio::test]
async fn the_origin_is_told_the_host_from_the_request_url() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;
    let authority = origin.addr().to_string();

    let response = send_raw(
        proxy.addr(),
        format!("GET http://{authority}/hello HTTP/1.1\r\nHost: stale.invalid\r\n\r\n").as_bytes(),
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        origin.only_request().header("host"),
        Some(authority.as_str())
    );

    proxy.stop().await;
}

#[tokio::test]
async fn proxy_only_headers_are_not_forwarded_to_the_origin() {
    let origin = Origin::responding_with(HELLO).await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[
            ("Proxy-Connection", "keep-alive"),
            ("Proxy-Authorization", "Basic c2VjcmV0"),
            ("Connection", "x-hop-header"),
            ("X-Hop-Header", "dropped"),
            ("X-Kept-Header", "kept"),
        ],
    )
    .await;

    assert_eq!(response.status, 200);

    let forwarded = origin.only_request();
    assert_eq!(forwarded.header("proxy-connection"), None);
    assert_eq!(forwarded.header("proxy-authorization"), None);
    assert_eq!(forwarded.header("x-hop-header"), None);
    assert_eq!(forwarded.header("x-kept-header"), Some("kept"));

    proxy.stop().await;
}

#[tokio::test]
async fn response_headers_reach_the_client() {
    let origin = Origin::responding_with(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: application/json\r\n\
          X-Origin-Marker: yes\r\n\
          Cache-Control: no-store\r\n\
          Content-Length: 2\r\n\
          \r\n\
          {}",
    )
    .await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/data", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.header("x-origin-marker"), Some("yes"));
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(response.body_text(), "{}");

    proxy.stop().await;
}

#[tokio::test]
async fn a_content_length_response_arrives_whole() {
    let body = "0123456789".repeat(200);
    let origin = Origin::start(move |_| {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    })
    .await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/big", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert_eq!(response.header("content-length"), Some("2000"));
    assert_eq!(response.body().len(), 2000);

    proxy.stop().await;
}

#[tokio::test]
async fn a_chunked_response_arrives_whole() {
    let origin = Origin::responding_with(
        b"HTTP/1.1 200 OK\r\n\
          Transfer-Encoding: chunked\r\n\
          \r\n\
          5\r\nhello\r\n\
          1\r\n \r\n\
          6\r\nworld!\r\n\
          0\r\n\r\n",
    )
    .await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/stream", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 200);
    assert!(
        response.is_chunked(),
        "expected the proxy to frame the body as chunked: {:?}",
        response.headers
    );
    assert_eq!(response.header("content-length"), None);
    assert_eq!(response.body_text(), "hello world!");

    proxy.stop().await;
}

#[tokio::test]
async fn a_204_is_forwarded_with_no_body() {
    let origin = Origin::responding_with(b"HTTP/1.1 204 No Content\r\n\r\n").await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "DELETE",
        &format!("{}/users/1", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 204);
    assert!(response.framed_body.is_empty());
    assert_eq!(response.header("content-length"), None);
    assert_eq!(response.header("transfer-encoding"), None);

    proxy.stop().await;
}

#[tokio::test]
async fn a_redirect_is_forwarded_with_its_location() {
    let origin = Origin::responding_with(
        b"HTTP/1.1 301 Moved Permanently\r\nLocation: /moved\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/old", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 301);
    assert_eq!(response.header("location"), Some("/moved"));
    assert!(response.body().is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn an_origin_error_status_is_forwarded_unchanged() {
    for (status, response_bytes, body) in [
        (
            400_u16,
            &b"HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\n\r\nbad request"[..],
            "bad request",
        ),
        (
            404,
            &b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found"[..],
            "not found",
        ),
        (
            500,
            &b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\n\r\nboom!"[..],
            "boom!",
        ),
    ] {
        let owned = response_bytes.to_vec();
        let origin = Origin::start(move |_| owned.clone()).await;
        let proxy = Proxy::start().await;

        let response = request(
            proxy.addr(),
            "GET",
            &format!("{}/thing", origin.base_url()),
            &[],
        )
        .await;

        assert_eq!(response.status, status);
        assert_eq!(response.body_text(), body);

        proxy.stop().await;
    }
}

#[tokio::test]
async fn sequential_requests_are_each_forwarded() {
    let origin = Origin::start(|request| {
        let body = request.target.clone();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    })
    .await;
    let proxy = Proxy::start().await;

    for path in ["/first", "/second", "/third"] {
        let response = request(
            proxy.addr(),
            "GET",
            &format!("{}{path}", origin.base_url()),
            &[],
        )
        .await;

        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), path);
    }

    let targets: Vec<String> = origin
        .requests()
        .into_iter()
        .map(|request| request.target)
        .collect();
    assert_eq!(targets, ["/first", "/second", "/third"]);

    proxy.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_client_connections_are_each_forwarded() {
    let origin = Origin::start(|request| {
        let body = request.target.clone();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    })
    .await;
    let proxy = Proxy::start().await;

    let exchanges: Vec<_> = (0..8)
        .map(|index| {
            let url = format!("{}/client-{index}", origin.base_url());
            let addr = proxy.addr();
            tokio::spawn(async move { (index, request(addr, "GET", &url, &[]).await) })
        })
        .collect();

    for exchange in exchanges {
        let (index, response) = exchange.await.expect("client task does not panic");

        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), format!("/client-{index}"));
    }

    assert_eq!(origin.requests().len(), 8);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_that_is_not_http_is_answered_with_400() {
    let proxy = Proxy::start().await;

    let response = send_raw(proxy.addr(), b"this is not an HTTP request\r\n\r\n").await;

    assert_eq!(response.status, 400);

    proxy.stop().await;
}

#[tokio::test]
async fn a_request_without_an_absolute_url_is_answered_with_400() {
    let proxy = Proxy::start().await;

    let response = send_raw(
        proxy.addr(),
        b"GET /not-a-proxy-request HTTP/1.1\r\nHost: example.test\r\n\r\n",
    )
    .await;

    assert_eq!(response.status, 400);
    assert!(
        response.body_text().contains("absolute URL"),
        "unhelpful body: {:?}",
        response.body_text()
    );

    proxy.stop().await;
}

#[tokio::test]
async fn an_unsupported_scheme_is_answered_with_501() {
    let proxy = Proxy::start().await;

    let response = send_raw(
        proxy.addr(),
        b"GET https://example.test/secure HTTP/1.1\r\nHost: example.test\r\n\r\n",
    )
    .await;

    assert_eq!(response.status, 501);

    proxy.stop().await;
}

#[tokio::test]
async fn a_destination_that_refuses_the_connection_is_answered_with_502() {
    let proxy = Proxy::start().await;
    let dead = unused_addr().await;

    let response = request(proxy.addr(), "GET", &format!("http://{dead}/hello"), &[]).await;

    assert_eq!(response.status, 502);
    assert!(
        response.body_text().contains("could not connect"),
        "unhelpful body: {:?}",
        response.body_text()
    );
    assert_no_internal_detail(&response);

    proxy.stop().await;
}

#[tokio::test]
async fn a_destination_that_answers_with_garbage_is_answered_with_502() {
    let origin = Origin::responding_with(b"not an HTTP response at all\r\n\r\n").await;
    let proxy = Proxy::start().await;

    let response = request(
        proxy.addr(),
        "GET",
        &format!("{}/hello", origin.base_url()),
        &[],
    )
    .await;

    assert_eq!(response.status, 502);
    assert_no_internal_detail(&response);

    proxy.stop().await;
}

/// An address nothing is listening on: a port is bound to learn a free one and
/// then released, so connecting to it is refused rather than ignored.
async fn unused_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    drop(listener);

    addr.to_string()
}

/// The client is told what went wrong, not how the proxy is built.
fn assert_no_internal_detail(response: &RawResponse) {
    let body = response.body_text();

    for leak in ["Os {", "io::", "hyper", "Error {", "src/"] {
        assert!(
            !body.contains(leak),
            "error body leaks internals ({leak}): {body:?}"
        );
    }
}
