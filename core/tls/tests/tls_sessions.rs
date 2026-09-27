//! End-to-end tests for the two TLS sessions interception is made of.
//!
//! Both ends of every handshake here are in this process, on loopback ports
//! the operating system chose. Nothing reaches the network, no DNS is looked
//! up, and no public web site is involved: the "origin server" is a socket in
//! this test binary presenting a certificate this crate minted.
//!
//! The two directions are the same code seen from opposite ends —
//! `accept_client` is ProxyLab answering a client, `connect_upstream` is
//! ProxyLab connecting to an origin — so one pair of sockets exercises both.
//! Which end a test is *about* is what its name says.

use std::io;

use proxy_tls::{
    client_protocol, upstream_protocol, CertificateAuthority, Host, Interceptor, Protocol,
    TlsError, UpstreamTrust,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

/// Long enough to absorb a loaded machine, short enough that a handshake
/// which never completes fails the test instead of hanging it.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// An interceptor whose CA signs the certificates it presents.
///
/// Its own upstream trust is the public web PKI, which nothing here uses: a
/// test that connects somewhere builds a second interceptor that trusts this
/// one's CA.
fn serving() -> Interceptor {
    Interceptor::new(
        CertificateAuthority::generate().expect("generate a CA"),
        &UpstreamTrust::PublicRoots,
    )
    .expect("build an interceptor")
}

/// An interceptor that trusts `serving`'s CA and nothing else.
fn trusting(serving: &Interceptor) -> Interceptor {
    Interceptor::new(
        CertificateAuthority::generate().expect("generate a CA"),
        &UpstreamTrust::Certificates(vec![serving.authority().certificate_der().clone()]),
    )
    .expect("build an interceptor")
}

/// An interceptor whose sessions offer only `protocols` over ALPN.
///
/// For the tests that are about what one end can and cannot speak: a client
/// that has never heard of HTTP/2, or an origin that has not.
fn serving_only(protocols: &[Protocol]) -> Interceptor {
    Interceptor::offering(
        CertificateAuthority::generate().expect("generate a CA"),
        &UpstreamTrust::PublicRoots,
        protocols,
    )
    .expect("build an interceptor")
}

/// An interceptor that trusts `serving`'s CA and offers only `protocols`.
fn trusting_only(serving: &Interceptor, protocols: &[Protocol]) -> Interceptor {
    Interceptor::offering(
        CertificateAuthority::generate().expect("generate a CA"),
        &UpstreamTrust::Certificates(vec![serving.authority().certificate_der().clone()]),
        protocols,
    )
    .expect("build an interceptor")
}

/// Completes both halves of one handshake and reports what ALPN settled on.
///
/// The first protocol is the server's view of the session it answered, the
/// second the client's view of the session it opened. They are read from the
/// two ends of the same connection, so a test that asserts on both is
/// asserting that the ends agree.
async fn negotiated(server: &Interceptor, client: &Interceptor) -> (Protocol, Protocol) {
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    let accepted = accepted.expect("the server side of the handshake");
    let connected = connected.expect("the client side of the handshake");

    (client_protocol(&accepted), upstream_protocol(&connected))
}

/// An interceptor that trusts only the public web PKI.
///
/// No locally generated CA is ever in that store, so every origin in these
/// tests is refused by it — which is the point of the tests that use it.
fn trusting_only_the_public_roots() -> Interceptor {
    Interceptor::new(
        CertificateAuthority::generate().expect("generate a CA"),
        &UpstreamTrust::PublicRoots,
    )
    .expect("build an interceptor")
}

fn host(text: &str) -> Host {
    Host::parse(text).expect("usable host")
}

/// Two ends of one TCP connection on a loopback port the OS chose.
///
/// The first is the end a client speaks from, the second the end a server
/// answers on.
async fn connected() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");

    let connecting = tokio::spawn(async move { TcpStream::connect(addr).await });
    let (accepted, _) = listener.accept().await.expect("accept the pair");
    let client = connecting
        .await
        .expect("connect task does not panic")
        .expect("connect to the pair");

    (client, accepted)
}

#[tokio::test]
async fn a_client_that_trusts_the_ca_accepts_a_generated_certificate() {
    let server = serving();
    let client = trusting(&server);
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    accepted.expect("the server side of the handshake");
    let session = connected.expect("the client side of the handshake");

    let presented = session
        .get_ref()
        .1
        .peer_certificates()
        .expect("the server presented a certificate")
        .to_vec();
    let expected = server
        .identity(&host)
        .expect("the identity is the one that was served")
        .certificate_der()
        .clone();

    assert_eq!(presented, [expected]);
}

#[tokio::test]
async fn the_verified_session_is_modern_and_speaks_http_2() {
    let server = serving();
    let client = trusting(&server);
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    accepted.expect("the server side of the handshake");
    let session = connected.expect("the client side of the handshake");
    let connection = session.get_ref().1;

    assert_eq!(
        connection.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(connection.alpn_protocol(), Some(&b"h2"[..]));
}

#[tokio::test]
async fn two_ends_that_both_speak_http_2_settle_on_it() {
    let server = serving();
    let client = trusting(&server);

    let (served, connected) = negotiated(&server, &client).await;

    assert_eq!(served, Protocol::Http2);
    assert_eq!(connected, Protocol::Http2);
}

#[tokio::test]
async fn a_client_that_speaks_only_http_1_1_is_answered_in_http_1_1() {
    // The server offers both. What decides is that the client offered one,
    // which is exactly the case of a client built before HTTP/2 existed.
    let server = serving();
    let client = trusting_only(&server, &[Protocol::Http11]);

    let (served, connected) = negotiated(&server, &client).await;

    assert_eq!(served, Protocol::Http11);
    assert_eq!(connected, Protocol::Http11);
}

#[tokio::test]
async fn an_origin_that_speaks_only_http_1_1_is_spoken_to_in_http_1_1() {
    // The same handshake from the other side: ProxyLab is the client here,
    // offering both, and the origin can only take one of them. Offering
    // HTTP/2 is not requiring it.
    let origin = serving_only(&[Protocol::Http11]);
    let proxy = trusting(&origin);

    let (served, connected) = negotiated(&origin, &proxy).await;

    assert_eq!(served, Protocol::Http11);
    assert_eq!(connected, Protocol::Http11);
}

#[tokio::test]
async fn an_origin_that_offers_no_alpn_at_all_is_spoken_to_in_http_1_1() {
    let origin = serving_only(&[]);
    let proxy = trusting(&origin);

    let (served, connected) = negotiated(&origin, &proxy).await;

    assert_eq!(served, Protocol::Http11);
    assert_eq!(connected, Protocol::Http11);
}

#[tokio::test]
async fn an_origin_certificate_that_chains_to_no_trusted_root_is_refused() {
    let server = serving();
    let client = trusting_only_the_public_roots();
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (_, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake finishes, one way or the other");

    let error = connected.expect_err("an untrusted certificate must not be accepted");

    assert!(matches!(error, TlsError::UpstreamHandshake(_)), "{error:?}");
}

#[tokio::test]
async fn an_origin_certificate_from_another_ca_is_refused() {
    let server = serving();
    let elsewhere = serving();
    // Trusts a real CA — just not the one the server it is about to reach
    // signs with.
    let client = trusting(&elsewhere);
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (_, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake finishes, one way or the other");

    connected.expect_err("a certificate from another CA must not be accepted");
}

#[tokio::test]
async fn an_origin_certificate_for_another_host_is_refused() {
    let server = serving();
    let client = trusting(&server);
    let (client_end, server_end) = connected().await;

    // The certificate is signed by the CA the client trusts, and names a
    // different host. Trusting the issuer is not trusting the name.
    let (_, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host("other.example.com"), server_end),
            client.connect_upstream(&host("localhost"), client_end),
        ),
    )
    .await
    .expect("the handshake finishes, one way or the other");

    let error = connected.expect_err("a certificate for another host must not be accepted");

    assert!(matches!(error, TlsError::UpstreamHandshake(_)), "{error:?}");
}

#[tokio::test]
async fn a_session_can_be_established_for_an_ip_address() {
    let server = serving();
    let client = trusting(&server);
    let host = host("127.0.0.1");
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    accepted.expect("the server side of the handshake");
    connected.expect("an IP subject alternative name is matched against the address");
}

#[tokio::test]
async fn a_certificate_for_an_address_does_not_serve_a_different_address() {
    let server = serving();
    let client = trusting(&server);
    let (client_end, server_end) = connected().await;

    let (_, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host("127.0.0.2"), server_end),
            client.connect_upstream(&host("127.0.0.1"), client_end),
        ),
    )
    .await
    .expect("the handshake finishes, one way or the other");

    connected.expect_err("a certificate for another address must not be accepted");
}

#[tokio::test]
async fn a_tls_1_0_client_hello_is_refused() {
    assert_legacy_hello_is_refused(0x0301).await;
}

#[tokio::test]
async fn a_tls_1_1_client_hello_is_refused() {
    assert_legacy_hello_is_refused(0x0302).await;
}

/// Offers `version` and nothing else, and expects to be turned away.
///
/// A hand-written `ClientHello` rather than a library one, because no client
/// built on this crate's TLS library can offer TLS 1.0 or TLS 1.1 — the
/// library does not implement them. Writing the bytes is the only way to ask
/// the question, and the answer is what a client stuck on an obsolete
/// protocol would get: a fatal alert and no session.
async fn assert_legacy_hello_is_refused(version: u16) {
    let server = serving();
    let host = host("localhost");
    let (mut client_end, server_end) = connected().await;

    let (accepted, alert) = timeout(
        TEST_TIMEOUT,
        together(server.accept_client(&host, server_end), async move {
            client_end.write_all(&legacy_client_hello(version)).await?;
            client_end.flush().await?;

            let mut answer = Vec::new();
            client_end.read_to_end(&mut answer).await?;

            io::Result::Ok(answer)
        }),
    )
    .await
    .expect("the server answers");

    let error = accepted.expect_err("an obsolete protocol version must not be accepted");
    assert!(matches!(error, TlsError::ClientHandshake(_)), "{error:?}");

    // A fatal alert, so the client is told why rather than left waiting.
    let alert = alert.expect("read the server's answer");
    assert_eq!(alert.first(), Some(&0x15), "not an alert record: {alert:?}");
    assert_eq!(alert.get(5), Some(&0x02), "the alert is not fatal");
    assert_eq!(
        alert.get(6),
        Some(&70),
        "the alert should name the protocol version"
    );
}

#[tokio::test]
async fn the_same_hello_offering_tls_1_2_is_answered() {
    let server = serving();
    let host = host("localhost");
    let (mut client_end, server_end) = connected().await;

    // The control for the two tests above. The same hand-written hello, with
    // only the version changed to one the server does speak, is answered with
    // a handshake record rather than an alert — so what got the other two
    // refused was the protocol version and not the way they were written.
    let (_, answer) = timeout(
        TEST_TIMEOUT,
        together(server.accept_client(&host, server_end), async move {
            client_end.write_all(&legacy_client_hello(0x0303)).await?;
            client_end.flush().await?;

            let mut first = [0_u8; 1];
            client_end.read_exact(&mut first).await.map(|_| first[0])
        }),
    )
    .await
    .expect("the server answers");

    assert_eq!(
        answer.expect("read the server's answer"),
        0x16,
        "the server answered with an alert rather than a handshake"
    );
}

/// The bytes of a `ClientHello` that offers only `version`.
///
/// Pre-TLS 1.3 shape: the version is in the record header and in the body,
/// and there is no `supported_versions` extension naming a newer one — which
/// is what makes this an offer of `version` and nothing else.
///
/// It is otherwise a well-formed hello, deliberately: the cipher suites,
/// groups and signature algorithms are ones the server does support, so the
/// only thing left for it to object to is the protocol version. Leaving them
/// out gets the hello refused for the wrong reason, and a test that passes
/// for the wrong reason proves nothing.
fn legacy_client_hello(version: u16) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&version.to_be_bytes());
    body.extend_from_slice(&[0xab; 32]); // random
    body.push(0); // no session to resume
    body.extend_from_slice(&[0x00, 0x02]); // one cipher suite, two bytes of it
    body.extend_from_slice(&[0xc0, 0x2b]); // ECDHE-ECDSA-AES128-GCM-SHA256
    body.extend_from_slice(&[0x01, 0x00]); // no compression
    body.extend_from_slice(&extensions());

    let mut handshake = vec![0x01]; // client_hello
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    handshake.extend_from_slice(&body);

    let mut record = vec![0x16]; // handshake
    record.extend_from_slice(&version.to_be_bytes());
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);

    record
}

/// The extension block of [`legacy_client_hello`].
///
/// `supported_groups` and `signature_algorithms`, both naming things the
/// server supports. Nothing here says anything about protocol versions.
fn extensions() -> Vec<u8> {
    let mut extensions = Vec::new();

    // supported_groups: secp256r1.
    extensions.extend_from_slice(&[0x00, 0x0a, 0x00, 0x04, 0x00, 0x02, 0x00, 0x17]);
    // signature_algorithms: ecdsa_secp256r1_sha256, rsa_pss_rsae_sha256.
    extensions.extend_from_slice(&[0x00, 0x0d, 0x00, 0x06, 0x00, 0x04, 0x04, 0x03, 0x08, 0x04]);
    // ec_point_formats: uncompressed.
    extensions.extend_from_slice(&[0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);

    let mut block = (extensions.len() as u16).to_be_bytes().to_vec();
    block.extend_from_slice(&extensions);

    block
}

#[tokio::test]
async fn a_session_closes_cleanly() {
    let server = serving();
    let client = trusting(&server);
    let host = host("localhost");
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client(&host, server_end),
            client.connect_upstream(&host, client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    let mut server_session = accepted.expect("the server side of the handshake");
    let mut client_session = connected.expect("the client side of the handshake");

    let (shut, read) = timeout(TEST_TIMEOUT, async {
        tokio::join!(server_session.shutdown(), async {
            // Reaching the end of the stream without an error is what proves
            // the close was announced: an intercepted session that simply
            // dropped its socket would surface here as an unexpected
            // end-of-file, which is indistinguishable from a truncation
            // attack and is what clients complain about.
            let mut rest = Vec::new();
            client_session.read_to_end(&mut rest).await.map(|_| rest)
        })
    })
    .await
    .expect("the session closes");

    shut.expect("the server closes its side");
    assert!(read.expect("the close was announced").is_empty());
}

#[tokio::test]
async fn a_hello_that_names_a_host_is_answered_as_that_host() {
    // The connection was made to an address, as a flow captured below the
    // application is; the client still means a name, and says so in SNI.
    let server = serving();
    let client = trusting(&server);
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client_named(&host("127.0.0.1"), server_end),
            client.connect_upstream(&host("localhost"), client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    let (named, _) = accepted.expect("the server side of the handshake");
    connected.expect("the certificate names the host the client asked for");

    assert_eq!(named, host("localhost"));
    assert_eq!(server.cached_hosts(), 1, "one certificate, for the name");
}

#[tokio::test]
async fn a_hello_that_names_no_host_is_answered_as_the_address() {
    // rustls sends no SNI for an address, which is what a client connecting
    // to a bare IP does; the certificate is then for the address, because
    // that is what the client is checking.
    let server = serving();
    let client = trusting(&server);
    let (client_end, server_end) = connected().await;

    let (accepted, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client_named(&host("127.0.0.1"), server_end),
            client.connect_upstream(&host("127.0.0.1"), client_end),
        ),
    )
    .await
    .expect("the handshake completes");

    let (named, _) = accepted.expect("the server side of the handshake");
    connected.expect("an address certificate is accepted for the address");

    assert_eq!(named, host("127.0.0.1"));
}

#[tokio::test]
async fn a_named_hello_is_still_refused_by_a_client_that_does_not_trust_the_ca() {
    let server = serving();
    let client = trusting_only_the_public_roots();
    let (client_end, server_end) = connected().await;

    let (_, connected) = timeout(
        TEST_TIMEOUT,
        together(
            server.accept_client_named(&host("127.0.0.1"), server_end),
            client.connect_upstream(&host("localhost"), client_end),
        ),
    )
    .await
    .expect("the handshake finishes, one way or the other");

    connected.expect_err("reading the name first must not make the certificate trusted");
}

/// Drives two futures to completion together on this task.
///
/// A handshake needs both ends making progress, and neither end here is
/// `'static`, so they are polled together rather than spawned.
async fn together<A, B>(a: A, b: B) -> (A::Output, B::Output)
where
    A: std::future::Future,
    B: std::future::Future,
{
    tokio::join!(a, b)
}
