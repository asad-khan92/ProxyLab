//! The HTTP/2 ends of the proxying tests.
//!
//! Everything here runs in this process on ephemeral ports, like the rest of
//! the harness: an HTTPS origin that speaks HTTP/2 ([`Http2Origin`]), and a
//! client that reaches one through the proxy the way a browser does —
//! `CONNECT`, TLS with `h2` in its ALPN, then streams ([`Http2Client`]).
//! Nothing reaches the network and no external service is involved.
//!
//! # Why these two use an HTTP library
//!
//! The rest of the harness writes HTTP by hand, because what those tests are
//! about is the bytes the proxy put on the wire. These do not, and could not:
//! an HTTP/2 request is HPACK-compressed header blocks inside flow-controlled
//! frames, and hand-rolling that would be writing a second HTTP/2
//! implementation to check the first one. What these tests are about is what
//! the proxy did with a *stream* — which exchange it became, which rule it
//! met, which response came back — so the framing is left to the library on
//! both ends, as TLS is left to `proxy-tls` on both ends.
//!
//! # What they offer over ALPN
//!
//! `h2` and nothing else. An origin here is an origin that speaks HTTP/2, and
//! a client here is a client that asks for it; a test that wants the other
//! answer uses the HTTP/1.1 ends instead. Only the proxy offers both, which
//! is what puts ALPN in a position to decide anything.

#![allow(dead_code)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use proxy_tls::{CertificateAuthority, Host, Interceptor, Protocol, UpstreamTrust};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{connect_tunnel, TEST_TIMEOUT};

/// Port `0` asks the operating system for a free port.
const EPHEMERAL: &str = "127.0.0.1:0";

/// The only protocol the HTTP/2 ends of these tests speak.
const HTTP_2_ONLY: &[Protocol] = &[Protocol::Http2];

/// A CA generated for one test and thrown away with it.
fn generated_ca() -> CertificateAuthority {
    CertificateAuthority::generate().expect("generate a CA")
}

fn tls_host(host: &str) -> Host {
    Host::parse(host).expect("usable host")
}

/// A request as an HTTP/2 origin received it.
///
/// The pseudo-headers are kept apart from the headers, because that is how
/// they arrive and because keeping them apart is what several of these tests
/// check the proxy did: `:authority` is not a `Host` header, and an HTTP/2
/// request that carried a `Host` header would be a translation gone wrong.
#[derive(Debug, Clone)]
pub struct Http2Request {
    /// Which of the origin's connections this stream was opened on.
    ///
    /// Numbered from zero in the order the origin accepted them, so a test can
    /// assert that several streams shared one connection rather than only that
    /// the origin was opened to once.
    pub connection: usize,
    pub method: String,
    pub scheme: Option<String>,
    pub authority: Option<String>,
    pub path: String,
    pub version: Version,
    /// Header names lowercased, values as sent, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Http2Request {
    /// First value of `name`, or `None` if the header was not sent.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as text.
    pub fn body_text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("text body")
    }
}

/// What an origin answers one request with.
#[derive(Debug, Clone)]
pub struct Http2Reply {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Http2Reply {
    /// A `200 OK` whose body is `body`.
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::status(StatusCode::OK, body)
    }

    /// A reply with `status` and `body`.
    pub fn status(status: StatusCode, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// The same reply, also carrying `name: value`.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// An in-process HTTPS origin server that speaks HTTP/2.
///
/// TLS first — a certificate from a CA generated for this server, so a proxy
/// reaching it has to be told to trust that CA — and then HTTP/2 for as long
/// as the connection lasts, with as many concurrent streams as the other end
/// opens.
///
/// It listens on `localhost` and its certificate names `localhost`, so a
/// proxy connecting to it verifies a real host name against a real subject
/// alternative name.
pub struct Http2Origin {
    addr: SocketAddr,
    host: String,
    interceptor: Arc<Interceptor>,
    requests: Arc<Mutex<Vec<Http2Request>>>,
    connections: Arc<Mutex<usize>>,
    task: JoinHandle<()>,
}

impl Http2Origin {
    /// Starts an origin, listening as `host`, whose reply to every request is
    /// `reply`.
    pub async fn responding_with(host: &str, reply: Http2Reply) -> Self {
        Self::start(host, move |_| reply.clone()).await
    }

    /// Starts an origin, listening as `host`, that builds its reply from the
    /// request it received.
    ///
    /// `responder` is called once per stream and may be called several times
    /// at once, which is the point: a test asserting on multiplexing needs an
    /// origin that can have several streams in flight.
    pub async fn start<R>(host: &str, responder: R) -> Self
    where
        R: Fn(&Http2Request) -> Http2Reply + Send + Sync + 'static,
    {
        let interceptor = Arc::new(
            Interceptor::offering(generated_ca(), &UpstreamTrust::PublicRoots, HTTP_2_ONLY)
                .expect("build an interceptor"),
        );
        let listener = TcpListener::bind((host, 0))
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(Mutex::new(0));

        let responder = Arc::new(responder);
        let recorded = Arc::clone(&requests);
        let counted = Arc::clone(&connections);
        let serving = Arc::clone(&interceptor);
        let name = tls_host(host);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let responder = Arc::clone(&responder);
                let recorded = Arc::clone(&recorded);
                let counted = Arc::clone(&counted);
                let serving = Arc::clone(&serving);
                let name = name.clone();

                tokio::spawn(async move {
                    let Ok(session) = serving.accept_client(&name, stream).await else {
                        return;
                    };
                    let id = {
                        let mut connections = counted.lock().expect("connections are writable");
                        *connections += 1;

                        *connections - 1
                    };

                    let service = service_fn(move |request| {
                        let responder = Arc::clone(&responder);
                        let recorded = Arc::clone(&recorded);

                        async move {
                            Ok::<_, Infallible>(answer(id, request, responder, recorded).await)
                        }
                    });

                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(session), service)
                        .await;
                });
            }
        });

        Self {
            addr,
            host: host.to_owned(),
            interceptor,
            requests,
            connections,
            task,
        }
    }

    /// The `host:port` a `CONNECT` request should name to reach it.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.addr.port())
    }

    /// The address it is listening on, as a transparently captured flow to
    /// it would name it.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The host name its certificate is issued for.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// What a proxy has to trust to accept this origin's certificate.
    pub fn trust(&self) -> UpstreamTrust {
        UpstreamTrust::Certificates(vec![self.interceptor.authority().certificate_der().clone()])
    }

    /// Every request the origin has received so far.
    pub fn requests(&self) -> Vec<Http2Request> {
        self.requests.lock().expect("requests are readable").clone()
    }

    /// The single request the origin received.
    pub fn only_request(&self) -> Http2Request {
        let mut requests = self.requests();

        assert_eq!(requests.len(), 1, "expected exactly one origin request");
        requests.remove(0)
    }

    /// How many streams the origin served on connection `id`.
    pub fn requests_on(&self, id: usize) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.connection == id)
            .count()
    }

    /// How many TLS connections the origin has accepted.
    ///
    /// What this is for is the assertion that several streams shared one: an
    /// origin that was opened to four times for four requests is not being
    /// multiplexed to.
    pub fn connections(&self) -> usize {
        *self.connections.lock().expect("connections are readable")
    }
}

impl Drop for Http2Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Records one request and answers it.
async fn answer<R>(
    connection: usize,
    request: Request<Incoming>,
    responder: Arc<R>,
    recorded: Arc<Mutex<Vec<Http2Request>>>,
) -> Response<Full<Bytes>>
where
    R: Fn(&Http2Request) -> Http2Reply + Send + Sync + 'static,
{
    let (parts, body) = request.into_parts();
    let body = body
        .collect()
        .await
        .map(|collected| collected.to_bytes().to_vec())
        .unwrap_or_default();

    let received = Http2Request {
        connection,
        method: parts.method.as_str().to_owned(),
        scheme: parts.uri.scheme_str().map(str::to_owned),
        authority: parts.uri.authority().map(|a| a.to_string()),
        path: parts
            .uri
            .path_and_query()
            .map(|path| path.to_string())
            .unwrap_or_else(|| "/".to_owned()),
        version: parts.version,
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body,
    };

    let reply = responder(&received);
    recorded
        .lock()
        .expect("requests are writable")
        .push(received);

    let mut response = Response::new(Full::new(Bytes::from(reply.body)));
    *response.status_mut() = reply.status;
    for (name, value) in reply.headers {
        response.headers_mut().append(
            hyper::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
            hyper::header::HeaderValue::from_str(&value).expect("valid header value"),
        );
    }

    response
}

/// A response as an HTTP/2 client received it.
#[derive(Debug, Clone)]
pub struct Http2Response {
    pub status: u16,
    pub version: Version,
    /// Header names lowercased, values as received, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Http2Response {
    /// First value of `name`, or `None` if the header was not received.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as text.
    pub fn body_text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("text body")
    }
}

/// An HTTP/2 client that reaches origins through the proxy.
///
/// It does what a browser does: `CONNECT host:port`, a TLS handshake asking
/// for `h2`, and then streams inside the session. What it trusts is given to
/// it, so it is a client that has installed the proxy's development CA.
pub struct Http2Client {
    interceptor: Interceptor,
}

/// One HTTP/2 connection to the proxy, with streams still to send on it.
///
/// Held rather than used and dropped, because what several of these tests are
/// about is that one connection carried several exchanges. Cloning gives
/// another handle onto the same connection — which is what a test spawning a
/// task per stream needs, and is the client-side half of the multiplexing
/// being tested.
#[derive(Clone)]
pub struct Http2Connection {
    sender: hyper::client::conn::http2::SendRequest<Full<Bytes>>,
    authority: String,
}

impl Http2Client {
    /// A client that accepts certificates chaining to `trust` and asks for
    /// HTTP/2.
    pub fn trusting(trust: UpstreamTrust) -> Self {
        Self {
            interceptor: Interceptor::offering(generated_ca(), &trust, HTTP_2_ONLY)
                .expect("build an interceptor"),
        }
    }

    /// Opens a `CONNECT` to `authority`, handshakes, and starts HTTP/2 on it.
    ///
    /// Fails the test rather than returning an error: every test that calls
    /// this is about what happens on the connection, so one that never opened
    /// has nothing left to assert.
    pub async fn connect(&self, proxy: SocketAddr, authority: &str, host: &str) -> Http2Connection {
        let tunnel = connect_tunnel(proxy, authority).await;
        assert_eq!(
            tunnel.response.status, 200,
            "the proxy refused the CONNECT: {}",
            tunnel.response.status_line
        );

        let session = timeout(
            TEST_TIMEOUT,
            self.interceptor
                .connect_upstream(&tls_host(host), tunnel.stream),
        )
        .await
        .expect("the handshake finishes")
        .expect("the client accepts the proxy's certificate");

        assert_eq!(
            proxy_tls::upstream_protocol(&session),
            Protocol::Http2,
            "the proxy did not agree to speak HTTP/2"
        );

        let (sender, connection) = timeout(
            TEST_TIMEOUT,
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(session)),
        )
        .await
        .expect("the HTTP/2 preface is written")
        .expect("the proxy speaks HTTP/2");

        tokio::spawn(async move {
            let _ = connection.await;
        });

        Http2Connection {
            sender,
            authority: authority.to_owned(),
        }
    }

    /// The whole exchange on a connection of its own: `CONNECT`, handshake,
    /// one stream, read the answer.
    pub async fn exchange(
        &self,
        proxy: SocketAddr,
        authority: &str,
        host: &str,
        method: &str,
        path: &str,
    ) -> Http2Response {
        let connection = self.connect(proxy, authority, host).await;

        connection.send(method, path, &[], Vec::new()).await
    }
}

impl Http2Connection {
    /// Sends one request on a stream of this connection and reads the answer.
    pub async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Http2Response {
        self.request(method, path, headers, body)
            .await
            .expect("the proxy answers the stream")
    }

    /// The same, without insisting on an answer.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<Http2Response, hyper::Error> {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("https://{}{path}", self.authority))
            .version(Version::HTTP_2)
            .body(Full::new(Bytes::from(body)))
            .expect("valid request");

        for (name, value) in headers {
            request.headers_mut().append(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                hyper::header::HeaderValue::from_str(value).expect("valid header value"),
            );
        }

        let mut sender = self.sender.clone();
        let response = timeout(TEST_TIMEOUT, sender.send_request(request))
            .await
            .expect("the proxy answers within the timeout")?;

        let (parts, body) = response.into_parts();
        let body = timeout(TEST_TIMEOUT, body.collect())
            .await
            .expect("the response body arrives")?
            .to_bytes()
            .to_vec();

        Ok(Http2Response {
            status: parts.status.as_u16(),
            version: parts.version,
            headers: parts
                .headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_ascii_lowercase(),
                        String::from_utf8_lossy(value.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            body,
        })
    }

    /// Sends several requests at once and returns the answers by path.
    ///
    /// The requests are all started before any of them is awaited, which is
    /// what makes them concurrent streams on one connection rather than a
    /// sequence of them.
    pub async fn send_concurrently(&self, paths: &[&str]) -> HashMap<String, Http2Response> {
        let streams: Vec<_> = paths
            .iter()
            .map(|path| {
                let sender = self.sender.clone();
                let authority = self.authority.clone();
                let path = (*path).to_owned();

                tokio::spawn(async move {
                    let connection = Http2Connection { sender, authority };
                    let response = connection.send("GET", &path, &[], Vec::new()).await;

                    (path, response)
                })
            })
            .collect();

        let mut answers = HashMap::new();
        for stream in streams {
            let (path, response) = stream.await.expect("the stream task does not panic");
            answers.insert(path, response);
        }

        answers
    }
}

/// A plain TCP address nothing is listening on.
///
/// A port is bound to learn a free one and then released, so connecting to it
/// is refused rather than ignored.
pub async fn unused_addr() -> SocketAddr {
    let listener = TcpListener::bind(EPHEMERAL)
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local address");
    drop(listener);

    addr
}
