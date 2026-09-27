//! End-to-end tests for what a `CONNECT` request is answered with.
//!
//! This file is about the exchange before the handover: the status line, the
//! headers, and every target that gets refused instead. What happens after
//! the handover — the two TLS sessions and the HTTP inside them — is in
//! `https_interception.rs`.
//!
//! Everything runs in this process on OS-assigned ports. Nothing here
//! touches the network, DNS, or any external service, and no test needs the
//! proxy's production port.

mod support;

use support::{connect_request, connect_tunnel, send_raw, Proxy, RawResponse, RawServer};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

/// Long enough to absorb a loaded machine, short enough that a proxy that
/// never answers fails the test instead of hanging it.
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::test]
async fn a_connect_to_a_local_server_is_answered_with_connection_established() {
    let upstream = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    let tunnel = connect_tunnel(proxy.addr(), &upstream.authority()).await;

    assert_eq!(tunnel.response.status, 200);
    assert_eq!(
        tunnel.response.status_line,
        "HTTP/1.1 200 Connection Established"
    );

    drop(tunnel);
    proxy.stop().await;
}

#[tokio::test]
async fn the_response_that_hands_the_connection_over_frames_no_body() {
    let upstream = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    let tunnel = connect_tunnel(proxy.addr(), &upstream.authority()).await;

    // RFC 9110 §9.3.6: a 2xx answer to CONNECT carries no body, so no header
    // may claim one — everything after the head is the client's TLS
    // handshake.
    assert_eq!(tunnel.response.header("content-length"), None);
    assert_eq!(tunnel.response.header("transfer-encoding"), None);
    // Nor is the connection finished: it is being handed over.
    assert_eq!(tunnel.response.header("connection"), None);

    drop(tunnel);
    proxy.stop().await;
}

#[tokio::test]
async fn a_certificate_is_minted_for_the_host_that_was_asked_for() {
    let upstream = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    assert_eq!(proxy.certified_hosts(), 0);

    let tunnel = connect_tunnel(proxy.addr(), &upstream.authority()).await;
    assert_eq!(tunnel.response.status, 200);

    assert_eq!(
        proxy.certified_hosts(),
        1,
        "the proxy answered without a certificate to present"
    );

    drop(tunnel);
    proxy.stop().await;
}

#[tokio::test]
async fn two_connects_to_one_host_share_its_certificate() {
    let upstream = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    for _ in 0..3 {
        let tunnel = connect_tunnel(proxy.addr(), &upstream.authority()).await;
        assert_eq!(tunnel.response.status, 200);
        drop(tunnel);
    }

    assert_eq!(
        proxy.certified_hosts(),
        1,
        "a certificate was minted more than once for one host"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn different_hosts_get_certificates_of_their_own() {
    let first = RawServer::echoing().await;
    let second = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    // Two loopback servers on different ports: same address, so the same
    // host, which is exactly why the second one must not add a certificate.
    let one = connect_tunnel(proxy.addr(), &first.authority()).await;
    let two = connect_tunnel(proxy.addr(), &second.authority()).await;
    assert_eq!(one.response.status, 200);
    assert_eq!(two.response.status, 200);
    assert_eq!(proxy.certified_hosts(), 1, "the host is the same host");

    // A different host name for the same place is a different host, because
    // the certificate has to name what the client asked for.
    let named = connect_tunnel(proxy.addr(), &format!("localhost:{}", first.addr().port())).await;
    assert_eq!(named.response.status, 200);
    assert_eq!(proxy.certified_hosts(), 2);

    drop((one, two, named));
    proxy.stop().await;
}

#[tokio::test]
async fn a_malformed_connect_target_is_answered_with_400() {
    let proxy = Proxy::start().await;

    for target in ["/index.html", "*", "http://example.test/", ":443"] {
        let response = send_raw(proxy.addr(), &connect_request(target)).await;

        assert_eq!(response.status, 400, "target {target:?} was not refused");
        assert_no_internal_detail(&response);
    }

    proxy.stop().await;
}

#[tokio::test]
async fn a_connect_target_with_no_port_is_answered_with_400() {
    let proxy = Proxy::start().await;

    for target in ["example.test", "example.test:", "[::1]"] {
        let response = send_raw(proxy.addr(), &connect_request(target)).await;

        assert_eq!(response.status, 400, "target {target:?} was not refused");
        assert!(
            response.body_text().contains("port"),
            "unhelpful body for {target:?}: {:?}",
            response.body_text()
        );
    }

    proxy.stop().await;
}

#[tokio::test]
async fn a_connect_target_with_a_port_that_is_not_a_number_is_answered_with_400() {
    let proxy = Proxy::start().await;

    for target in ["example.test:https", "example.test:44a", "example.test:0"] {
        let response = send_raw(proxy.addr(), &connect_request(target)).await;

        assert_eq!(response.status, 400, "target {target:?} was not refused");
        assert!(
            response.body_text().contains("valid port"),
            "unhelpful body for {target:?}: {:?}",
            response.body_text()
        );
    }

    proxy.stop().await;
}

#[tokio::test]
async fn a_connect_target_whose_port_is_out_of_range_is_answered_with_400() {
    let proxy = Proxy::start().await;

    let response = send_raw(proxy.addr(), &connect_request("example.test:65536")).await;

    assert_eq!(response.status, 400);

    proxy.stop().await;
}

#[tokio::test]
async fn a_connect_target_that_is_not_a_host_name_is_answered_with_400() {
    let proxy = Proxy::start().await;

    // Authority form with a host and a port, so it gets past the target
    // check, and not a name a certificate can be issued for — an empty
    // label, and a label starting with a hyphen, are not host names.
    for target in ["example..test:443", "-example.test:443", ".test:443"] {
        let response = send_raw(proxy.addr(), &connect_request(target)).await;

        assert_eq!(response.status, 400, "target {target:?} was not refused");
        assert!(
            response.body_text().contains("host"),
            "unhelpful body for {target:?}: {:?}",
            response.body_text()
        );
        assert_no_internal_detail(&response);
    }

    assert_eq!(
        proxy.certified_hosts(),
        0,
        "a certificate was minted for something that is not a host"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn an_unreachable_connect_target_is_answered_with_502_and_never_with_200() {
    let proxy = Proxy::start().await;
    let dead = unused_addr().await;

    let response = send_raw(proxy.addr(), &connect_request(&dead)).await;

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
async fn a_refused_connect_hands_no_connection_over() {
    let proxy = Proxy::start().await;
    let dead = unused_addr().await;

    let mut client = TcpStream::connect(proxy.addr())
        .await
        .expect("connect to the proxy");
    client
        .write_all(&connect_request(&dead))
        .await
        .expect("send the CONNECT");

    // Everything the proxy will ever say on this connection, up to the close.
    let mut answer = Vec::new();
    timeout(TEST_TIMEOUT, client.read_to_end(&mut answer))
        .await
        .expect("the proxy answers and closes the connection")
        .expect("read from the proxy");

    let answer = String::from_utf8(answer).expect("text answer");
    assert!(
        answer.starts_with("HTTP/1.1 502 "),
        "unexpected answer: {answer:?}"
    );

    proxy.stop().await;
}

#[tokio::test]
async fn the_upstream_connection_is_released_when_the_client_goes_away() {
    let (was_closed, upstream) = closing_watcher().await;
    let proxy = Proxy::start().await;

    let tunnel = connect_tunnel(proxy.addr(), &upstream).await;
    assert_eq!(tunnel.response.status, 200);
    // The client never starts a TLS handshake and goes away, so interception
    // gets nowhere — and the connection the proxy opened on its behalf must
    // not be left hanging.
    drop(tunnel);

    let received = timeout(TEST_TIMEOUT, was_closed)
        .await
        .expect("the upstream connection is closed too")
        .expect("the signal is not dropped")
        .expect("read from the upstream connection");

    assert!(received.is_empty());

    proxy.stop().await;
}

#[tokio::test]
async fn a_forwarded_request_is_unaffected_by_interception_being_available() {
    let upstream = RawServer::echoing().await;
    let proxy = Proxy::start().await;

    // A CONNECT and an ordinary proxy request, on the same proxy, in order.
    let tunnel = connect_tunnel(proxy.addr(), &upstream.authority()).await;
    assert_eq!(tunnel.response.status, 200);
    drop(tunnel);

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

/// A server that reports, once, that its connection reached end of stream.
///
/// Returns the report and the `host:port` to name in a `CONNECT`.
async fn closing_watcher() -> (
    tokio::sync::oneshot::Receiver<std::io::Result<Vec<u8>>>,
    String,
) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let authority = listener.local_addr().expect("local address").to_string();

    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };

        let mut received = Vec::new();
        let outcome = stream.read_to_end(&mut received).await.map(|_| received);
        let _ = sender.send(outcome);
    });

    (receiver, authority)
}

/// An address nothing is listening on: a port is bound to learn a free one
/// and then released, so connecting to it is refused rather than ignored.
async fn unused_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    drop(listener);

    addr.to_string()
}

/// Fails if an error body carries anything from inside the proxy.
///
/// A client is told a status and a reason. Rust type names, `io::Error`
/// wording, and source paths are for the log.
fn assert_no_internal_detail(response: &RawResponse) {
    let body = response.body_text();

    for leak in ["Os {", "io::", "hyper", "rustls", "Error {", "src/"] {
        assert!(
            !body.contains(leak),
            "error body leaks internals ({leak}): {body:?}"
        );
    }
}
