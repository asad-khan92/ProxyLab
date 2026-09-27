//! Origins that speak the WebSocket handshake, and the clients that ask for it.
//!
//! The handshake is written by hand, on both ends, for the same reason the
//! rest of this harness writes HTTP by hand: what is under test is the bytes
//! ProxyLab put on the wire. Whether `Sec-WebSocket-Key` arrived unchanged,
//! whether `Connection: Upgrade` survived a proxy that strips hop-by-hop
//! headers, and whether the `101` reached the client with the origin's own
//! `Sec-WebSocket-Accept` on it are exactly the details a WebSocket library on
//! either end would hide.
//!
//! What happens *after* the handshake is opaque bytes, which is what ProxyLab
//! treats it as. These origins therefore send and expect plain byte strings
//! rather than WebSocket frames: a proxy that forwards bytes unchanged carries
//! frames unchanged, and a test that framed them would be testing the frame
//! writer.

#![allow(dead_code)]

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use proxy_tls::{Interceptor, UpstreamTrust};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::{RecordedRequest, TEST_TIMEOUT};

/// The accept value these origins answer with.
///
/// A real server computes it from the client's key; nothing in ProxyLab reads
/// it, and nothing here needs to, so it is a constant the client can assert on
/// — which proves the origin's own answer reached the client unchanged.
pub const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

/// The key the test clients send.
pub const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// The handshake response a WebSocket origin agrees with.
pub const AGREED: &str = concat!(
    "HTTP/1.1 101 Switching Protocols\r\n",
    "Upgrade: websocket\r\n",
    "Connection: Upgrade\r\n",
    "Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n",
    "\r\n"
);

/// The answer of an origin that will not upgrade.
pub const REFUSED: &str = concat!(
    "HTTP/1.1 404 Not Found\r\n",
    "Content-Type: text/plain\r\n",
    "Content-Length: 9\r\n",
    "Connection: close\r\n",
    "\r\n",
    "no socket"
);

/// Either half of an upgraded connection, whatever it is carried over.
pub type Stream = Box<dyn Upgraded>;

/// A stream an upgraded connection can be carried on.
pub trait Upgraded: AsyncRead + AsyncWrite + Send + Unpin {}

impl<S> Upgraded for S where S: AsyncRead + AsyncWrite + Send + Unpin {}

/// An in-process origin that answers a WebSocket handshake.
///
/// After the handshake it hands the socket to the behaviour the test supplied,
/// which reads and writes bytes as it likes — exactly as a WebSocket
/// implementation would, and exactly as opaquely as ProxyLab sees it.
pub struct WsOrigin {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    handshakes: Arc<Mutex<usize>>,
    task: JoinHandle<()>,
}

impl WsOrigin {
    /// An origin that agrees to upgrade and then echoes every byte back.
    pub async fn echoing() -> Self {
        Self::start(AGREED, |stream| async move {
            let (mut from_client, mut to_client) = tokio::io::split(stream);
            let _ = tokio::io::copy(&mut from_client, &mut to_client).await;
            let _ = to_client.shutdown().await;
        })
        .await
    }

    /// An origin that agrees to upgrade and then runs `behaviour`.
    pub async fn start<H, F>(handshake: &'static str, behaviour: H) -> Self
    where
        H: Fn(Stream) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handshakes = Arc::new(Mutex::new(0));

        let behaviour = Arc::new(behaviour);
        let recorded = Arc::clone(&requests);
        let counted = Arc::clone(&handshakes);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let behaviour = Arc::clone(&behaviour);
                let recorded = Arc::clone(&recorded);
                let counted = Arc::clone(&counted);

                tokio::spawn(async move {
                    let _ = serve(Box::new(stream), handshake, behaviour, recorded, counted).await;
                });
            }
        });

        Self {
            addr,
            requests,
            handshakes,
            task,
        }
    }

    /// The address the origin is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The `ws://host:port` prefix for building absolute request targets.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every handshake request the origin has received.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("requests are readable").clone()
    }

    /// The single handshake request the origin received.
    pub fn only_request(&self) -> RecordedRequest {
        let mut requests = self.requests();

        assert_eq!(requests.len(), 1, "expected exactly one handshake");
        requests.remove(0)
    }

    /// How many connections reached the handshake.
    ///
    /// What a blocked request is asserted against: a rule that stops a
    /// WebSocket is one the origin never hears from.
    pub fn handshakes(&self) -> usize {
        *self.handshakes.lock().expect("count is readable")
    }
}

impl Drop for WsOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The same origin behind TLS, for `wss://`.
pub struct TlsWsOrigin {
    addr: SocketAddr,
    host: String,
    interceptor: Arc<Interceptor>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    handshakes: Arc<Mutex<usize>>,
    task: JoinHandle<()>,
}

impl TlsWsOrigin {
    /// A TLS origin, listening as `host`, that upgrades and then echoes.
    pub async fn echoing(host: &str) -> Self {
        Self::start(host, AGREED, |stream| async move {
            let (mut from_client, mut to_client) = tokio::io::split(stream);
            let _ = tokio::io::copy(&mut from_client, &mut to_client).await;
            let _ = to_client.shutdown().await;
        })
        .await
    }

    /// A TLS origin, listening as `host`, that answers `handshake` and then
    /// runs `behaviour`.
    ///
    /// It offers `http/1.1` alone over ALPN, which is what a WebSocket origin
    /// is: there is no form of this handshake carried over HTTP/2.
    pub async fn start<H, F>(host: &str, handshake: &'static str, behaviour: H) -> Self
    where
        H: Fn(Stream) -> F + Send + Sync + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let interceptor = Arc::new(
            Interceptor::offering(
                proxy_tls::CertificateAuthority::generate().expect("generate a CA"),
                &UpstreamTrust::PublicRoots,
                &[proxy_tls::Protocol::Http11],
            )
            .expect("build an interceptor"),
        );
        let listener = TcpListener::bind((host, 0))
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handshakes = Arc::new(Mutex::new(0));

        let behaviour = Arc::new(behaviour);
        let recorded = Arc::clone(&requests);
        let counted = Arc::clone(&handshakes);
        let serving = Arc::clone(&interceptor);
        let name = proxy_tls::Host::parse(host).expect("usable host");
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let behaviour = Arc::clone(&behaviour);
                let recorded = Arc::clone(&recorded);
                let counted = Arc::clone(&counted);
                let serving = Arc::clone(&serving);
                let name = name.clone();

                tokio::spawn(async move {
                    let Ok(session) = serving.accept_client(&name, stream).await else {
                        return;
                    };

                    let _ = serve(Box::new(session), handshake, behaviour, recorded, counted).await;
                });
            }
        });

        Self {
            addr,
            host: host.to_owned(),
            interceptor,
            requests,
            handshakes,
            task,
        }
    }

    /// The `host:port` a `CONNECT` request should name to reach it.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.addr.port())
    }

    /// The host name its certificate is issued for.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// What a proxy has to trust to accept this origin's certificate.
    pub fn trust(&self) -> UpstreamTrust {
        super::trusting(self.interceptor.authority())
    }

    /// The single handshake request the origin received.
    pub fn only_request(&self) -> RecordedRequest {
        let mut requests = self.requests.lock().expect("requests are readable").clone();

        assert_eq!(requests.len(), 1, "expected exactly one handshake");
        requests.remove(0)
    }

    /// How many connections reached the handshake.
    pub fn handshakes(&self) -> usize {
        *self.handshakes.lock().expect("count is readable")
    }
}

impl Drop for TlsWsOrigin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads the handshake, answers it, and hands what is left to `behaviour`.
async fn serve<H, F>(
    stream: Stream,
    handshake: &'static str,
    behaviour: Arc<H>,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
    counted: Arc<Mutex<usize>>,
) -> io::Result<()>
where
    H: Fn(Stream) -> F + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    let mut reader = BufReader::new(stream);
    let request = read_request_head(&mut reader).await?;

    recorded
        .lock()
        .expect("requests are writable")
        .push(request);
    *counted.lock().expect("count is writable") += 1;

    let mut stream = reader.into_inner();
    stream.write_all(handshake.as_bytes()).await?;
    stream.flush().await?;

    if handshake.starts_with("HTTP/1.1 101") {
        behaviour(stream).await;
    }

    Ok(())
}

/// Reads one request head — everything up to the blank line — and parses it.
///
/// A WebSocket handshake carries no body, so the head is the whole request,
/// and stopping at the blank line leaves anything that follows for whoever
/// takes the socket over.
async fn read_request_head<S>(reader: &mut BufReader<S>) -> io::Result<RecordedRequest>
where
    S: AsyncRead + Unpin,
{
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];

    while !head.ends_with(b"\r\n\r\n") {
        if reader.read_exact(&mut byte).await.is_err() {
            break;
        }
        head.push(byte[0]);
    }

    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();

    let version = parts.next().unwrap_or_default().to_owned();

    Ok(RecordedRequest {
        method,
        target,
        version,
        headers,
        body: Vec::new(),
    })
}

/// The bytes of a WebSocket upgrade request for `url`.
pub fn upgrade_request(url: &str, host: &str) -> Vec<u8> {
    format!(
        "GET {url} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: Upgrade\r\n\
         Upgrade: websocket\r\n\
         Sec-WebSocket-Key: {KEY}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Origin: http://test.invalid\r\n\
         \r\n"
    )
    .into_bytes()
}

/// A response head read off a socket that is about to stop being HTTP.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Handshake {
    /// First value of `name`, or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Reads a response head one byte at a time, leaving the rest on the socket.
///
/// One byte at a time because what follows the blank line is not HTTP and
/// belongs to whoever reads next; a buffered reader would swallow it.
pub async fn read_handshake<S>(stream: &mut S) -> Handshake
where
    S: AsyncRead + Unpin,
{
    let head = timeout(TEST_TIMEOUT, async {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];

        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            head.push(byte[0]);
        }

        io::Result::Ok(head)
    })
    .await
    .expect("the proxy answers the handshake")
    .expect("read the handshake");

    let text = String::from_utf8(head).expect("text head");
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status line has a status code");
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();

    Handshake { status, headers }
}

/// Writes `message` and reads exactly as many bytes back.
pub async fn round_trip<S>(stream: &mut S, message: &[u8]) -> Vec<u8>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(TEST_TIMEOUT, async {
        stream.write_all(message).await?;
        stream.flush().await?;

        let mut back = vec![0_u8; message.len()];
        stream.read_exact(&mut back).await?;

        io::Result::Ok(back)
    })
    .await
    .expect("the tunnel carries the message")
    .expect("read the answer")
}

/// Reads exactly `count` bytes.
pub async fn read_exactly<S>(stream: &mut S, count: usize) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    timeout(TEST_TIMEOUT, async {
        let mut buffer = vec![0_u8; count];
        stream.read_exact(&mut buffer).await?;

        io::Result::Ok(buffer)
    })
    .await
    .expect("the tunnel delivers")
    .expect("read")
}

/// The two halves of an upgraded client connection.
pub fn split<S>(stream: S) -> (ReadHalf<S>, WriteHalf<S>)
where
    S: AsyncRead + AsyncWrite,
{
    tokio::io::split(stream)
}
