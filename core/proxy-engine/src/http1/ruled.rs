//! Carrying out what the rules decided.
//!
//! ```text
//! request ──▶ evaluate ──▶ Allow      ──▶ the proxy's usual path
//!                          Block      ──▶ answered here, nothing sent upstream
//!                          MapLocal   ──▶ answered from a file, nothing sent upstream
//!                          MapRemote  ──▶ sent somewhere else
//!                          Redirect   ──▶ answered with a redirect, nothing sent upstream
//! ```
//!
//! The Modify rules collected on the way to the decision are carried out by
//! [`crate::modify`], on the two routes that send the request anywhere.
//!
//! Deciding happens in `rules`, which touches nothing. This module is the
//! other half: the file is opened here, the generated response is built here,
//! and the connection to a mapped destination is opened here. Nothing in this
//! module decides *whether* a rule applies.
//!
//! # Where rules are applied, and where they are not
//!
//! Every HTTP request the proxy is about to carry is evaluated — a plaintext
//! one, and a request read out of an intercepted TLS session. A `CONNECT` is
//! not: it names a host and a port and carries no path, so matching it
//! against a path rule would be matching something that is not there, and
//! blocking it would block every request in the session rather than the one a
//! rule named. The requests inside the session are each evaluated on their
//! own, once TLS interception has made them readable.
//!
//! # What a client is told
//!
//! A blocked request gets the rule's status and one fixed sentence. A mapping
//! that could not be carried out gets `500` and another. Neither ever names
//! the rule, the file, or the path: a response body is something the client
//! keeps, and why a rule failed is for the developer's log.

use std::io;
use std::path::Path;

use http_body_util::BodyExt;
use hyper::header::{HeaderValue, CONTENT_TYPE, HOST, LOCATION};
use hyper::http::uri::PathAndQuery;
use hyper::{Request, Response, StatusCode, Uri, Version};
use proxy_storage::Destination;
use proxy_tls::Host;
use rules::{
    BlockAction, MapLocal, Redirect, RemoteScheme, RemoteTarget, RequestContext, RuleEngine,
    RuleOutcome, MAX_MAP_LOCAL_BYTES,
};

use super::headers;
use super::reply::{self, ProxyBody};
use super::upstream::UpstreamError;
use crate::capture::Recorder;
use crate::upstream::{PooledBody, UpstreamBody, UpstreamDestination, UpstreamManager};

/// What the rules say about a request: the decision, and the Modify rules
/// collected on the way to it.
///
/// A proxy running without a rule engine — which is what a build with no
/// database is — allows everything, rather than refusing to carry traffic
/// because it has nowhere to keep rules.
pub fn decide(engine: Option<&RuleEngine>, host: &str, path: &str, method: &str) -> RuleOutcome {
    match engine {
        Some(engine) => engine.outcome(&RequestContext::new(host, path, method)),
        None => RuleOutcome::allowed(),
    }
}

/// The response a `Block` produces.
///
/// Nothing is sent upstream, and nothing about the rule reaches the client:
/// the status is the rule's, and the sentence is this one.
pub fn blocked(block: &BlockAction, recorder: &Recorder) -> Response<ProxyBody> {
    // The status was validated when the rule was written, so this cannot
    // fail; one that somehow is not a status becomes the default rather than
    // a panic on the request path.
    let status = StatusCode::from_u16(block.status).unwrap_or(StatusCode::FORBIDDEN);

    generated(reply::error(status, "blocked by a ProxyLab rule"), recorder)
}

/// The response a `Redirect` produces.
///
/// The rule's status, a `Location` that is the rule's target exactly, and one
/// fixed sentence for a client that shows bodies. Nothing is sent upstream,
/// and nothing about the request is carried into the `Location`.
pub fn redirected(redirect: &Redirect, recorder: &Recorder) -> Response<ProxyBody> {
    // Validated when the rule was written: one of the four redirect statuses.
    let status = StatusCode::from_u16(redirect.status).unwrap_or(StatusCode::FOUND);
    let mut response = reply::error(status, "redirected by a ProxyLab rule");

    if let Ok(location) = HeaderValue::from_str(&redirect.location()) {
        response.headers_mut().insert(LOCATION, location);
    }

    generated(response, recorder)
}

/// The response a `MapLocal` produces, having read the file.
///
/// The only filesystem access rules cause, and it reaches exactly one path:
/// the one the rule names. Nothing from the request is joined to it, appended
/// to it, or used to choose a file beside it — see [`rules::MapLocal`] — so
/// there is no traversal here to defend against.
pub async fn mapped_local(map: &MapLocal, recorder: &Recorder) -> Response<ProxyBody> {
    let response = match read_mapped(map).await {
        Ok(body) => {
            let mut response = Response::new(reply::bytes(body));

            *response.status_mut() = StatusCode::from_u16(map.status).unwrap_or(StatusCode::OK);
            if let Some(content_type) = content_type(map) {
                response.headers_mut().insert(CONTENT_TYPE, content_type);
            }
            headers::set_close(response.headers_mut());

            response
        }
        Err(failure) => {
            // The path goes in the log and never in the response: a client
            // being served a fixture has no business learning where on this
            // machine it lives.
            eprintln!(
                "Map local failed for {}: {}",
                map.path().display(),
                failure.log_reason()
            );

            reply::error(StatusCode::INTERNAL_SERVER_ERROR, MAP_LOCAL_REASON)
        }
    };

    generated(response, recorder)
}

/// Rewrites a request for the destination a `MapRemote` names.
///
/// The path and query are worked out from the request's own URI and the
/// rule's, in [`RemoteTarget::rewrite`]. Nothing is assembled by pasting two
/// URLs together, which is how a request to
/// `https://staging.example.comhttps://api.example.com/foo` happens.
///
/// The method, the body, and every header but `Host` and the hop-by-hop ones
/// are the request's own: a mapping changes where a request goes, not what
/// it is.
pub fn into_mapped_request<B>(request: Request<B>, target: &RemoteTarget) -> Request<B> {
    let (mut parts, body) = request.into_parts();

    let rewritten = target.rewrite(parts.uri.path(), parts.uri.query());
    parts.uri = rewritten
        .parse::<PathAndQuery>()
        .map(Uri::from)
        .unwrap_or_else(|_| Uri::from_static("/"));
    parts.version = Version::HTTP_11;
    headers::strip_hop_by_hop(&mut parts.headers);

    // The origin is told the host it actually is. A `Host` left naming the
    // original destination would reach a server that has no such virtual
    // host, which is a mapping that appears to work and returns the wrong
    // thing.
    if let Ok(host) = HeaderValue::from_str(&target.host_header()) {
        parts.headers.insert(HOST, host);
    }

    Request::from_parts(parts, body)
}

/// Works out the upstream destination a mapped request is going to.
///
/// The scheme the rule names decides the transport: `http` is a plain socket,
/// `https` is a TLS session this proxy opens and whose certificate it
/// verifies — the same session an intercepted request would have used, to the
/// mapped host rather than the original one. So a mapping may move a request
/// between the two, and the hop ProxyLab makes is no less checked than the one
/// the client would have made itself.
///
/// Which HTTP that connection speaks is not the rule's business and is not
/// stated in one: a destination that agrees to HTTP/2 over ALPN is spoken to
/// in HTTP/2, and one that does not is spoken to in HTTP/1.1. A plaintext
/// destination is HTTP/1.1, because there is no ALPN to ask with.
///
/// This is also what the connection pool keys on, which is why it is built
/// from the rule's destination and never from the URL the client asked for: a
/// request mapped to another host must not be given a connection opened for
/// the host it named. See [`crate::upstream::key`].
pub fn mapped_upstream(target: &RemoteTarget) -> Result<UpstreamDestination, UpstreamError> {
    let port = target.effective_port();

    match target.scheme {
        RemoteScheme::Http => UpstreamDestination::plaintext(&target.host, port),
        RemoteScheme::Https => {
            let host = Host::parse(&target.host).map_err(|_| UpstreamError::UnusableHost)?;

            UpstreamDestination::tls(&host, port)
        }
    }
}

/// Sends a mapped request to the destination the rule named.
///
/// The connection comes from the shared pool, so a rule that maps many
/// requests to one destination reuses connections to it exactly as an
/// unmapped request to that destination would.
pub async fn send_mapped(
    target: &RemoteTarget,
    upstream: &UpstreamManager,
    request: Request<UpstreamBody>,
) -> Result<Response<PooledBody>, UpstreamError> {
    let destination = mapped_upstream(target)?;

    upstream.send(&destination, request).await
}

/// Where a mapped request is actually going, for the capture to show.
pub fn mapped_destination(target: &RemoteTarget) -> Destination {
    Destination::new(target.host.clone(), target.effective_port())
}

/// Records a response the proxy produced itself, and wraps its body.
///
/// A ruled request is traffic like any other: it appears in the list with the
/// response the client actually received, so a developer sees what happened
/// rather than a request that vanished. Wrapping the body is also what ends
/// the exchange — when the last of it reaches the client, the recorder
/// publishes.
///
/// Used by [`super::service`] too, for the one other response the proxy
/// writes without going anywhere: the setup page a device fetches the CA
/// from ([`super::setup`]). It is traffic for the same reason a blocked
/// request is — it happened, and a developer looking for it should find it.
pub(super) fn generated(response: Response<ProxyBody>, recorder: &Recorder) -> Response<ProxyBody> {
    recorder.upstream_responded(&response);

    let (parts, body) = response.into_parts();

    Response::from_parts(parts, recorder.capture_response(body).boxed())
}

/// Reads the file a mapping names, refusing anything that is not one.
///
/// Checked in this order so that each answer is the specific one: a
/// directory is a directory rather than "could not read", and a file larger
/// than a response may hold is refused before any of it is read.
async fn read_mapped(map: &MapLocal) -> Result<Vec<u8>, MapLocalFailure> {
    let path = map.path();

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(MapLocalFailure::from_io)?;
    if metadata.is_dir() {
        return Err(MapLocalFailure::Directory);
    }
    if !metadata.is_file() {
        return Err(MapLocalFailure::NotAFile);
    }
    if metadata.len() > MAX_MAP_LOCAL_BYTES {
        return Err(MapLocalFailure::TooLarge);
    }

    tokio::fs::read(path)
        .await
        .map_err(MapLocalFailure::from_io)
}

/// The content type a mapped file is answered with.
///
/// The rule's own if it named one, otherwise what the extension implies, and
/// nothing at all when neither says. A guess from a handful of known
/// extensions is honest; sniffing the bytes would not be.
fn content_type(map: &MapLocal) -> Option<HeaderValue> {
    if let Some(configured) = &map.content_type {
        return HeaderValue::from_str(configured).ok();
    }

    guess_content_type(map.path()).map(HeaderValue::from_static)
}

/// The content type a file extension implies.
fn guess_content_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();

    Some(match extension.as_str() {
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "application/xml",
        "csv" => "text/csv; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/vnd.microsoft.icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

/// What a client is told when a mapped file could not be served.
///
/// One sentence for every reason. Which reason it was is in the log, because
/// the difference between "the file is missing" and "the file is a
/// directory" tells a client about this machine's filesystem.
const MAP_LOCAL_REASON: &str = "a ProxyLab rule could not serve its local file";

/// Why a mapped file could not be served.
#[derive(Debug)]
enum MapLocalFailure {
    /// There is no such file.
    Missing,
    /// The path names a directory.
    Directory,
    /// The path names something that is neither a file nor a directory.
    NotAFile,
    /// The file cannot be read by this process.
    Denied,
    /// The file is larger than one response may hold.
    TooLarge,
    /// The filesystem refused for some other reason.
    Other,
}

impl MapLocalFailure {
    /// What an I/O error means for a mapping.
    fn from_io(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::Missing,
            io::ErrorKind::PermissionDenied => Self::Denied,
            _ => Self::Other,
        }
    }

    /// What the log is told, which may be specific.
    fn log_reason(&self) -> &'static str {
        match self {
            Self::Missing => "the file does not exist",
            Self::Directory => "the path is a directory",
            Self::NotAFile => "the path is not a regular file",
            Self::Denied => "permission denied",
            Self::TooLarge => "the file is larger than a mapped response may be",
            Self::Other => "the file could not be read",
        }
    }
}
