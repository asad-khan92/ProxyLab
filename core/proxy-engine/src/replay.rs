//! Sending a request ProxyLab was asked to send, rather than one it was given.
//!
//! ```text
//! a client                          the UI
//!    │                                 │  replay_request
//!    ▼                                 ▼
//! proxy listener                  Replayer::replay
//!    │                                 │
//!    └──────────────┬──────────────────┘
//!                   ▼
//!            rule evaluation
//!                   ▼
//!            upstream (TLS if https)
//!                   ▼
//!            capture ──▶ storage ──▶ traffic events
//! ```
//!
//! The two paths join immediately. Everything below the fork is the same
//! code — [`ruled`](crate::http1::ruled) decides and carries out,
//! [`Origin`] speaks whichever protocol the destination agreed to, `proxy-tls`
//! opens the verified session, and [`Capture`] records — so a replayed
//! request is evaluated by the same rules, sent over the same kind of
//! connection, captured under the same limits, stored in the same table, and
//! published on the same event stream as a request a browser made.
//!
//! That includes HTTP/2. A replay is written as the canonical HTTP/1.1-shaped
//! request it always was; if the destination's TLS session settles on `h2`,
//! it goes out as HTTP/2, because that is what every request through this
//! proxy does. There is nothing to ask for and nothing to configure — replay
//! gained HTTP/2 by not having a second way of sending things.
//!
//! # Why there is an entry point at all
//!
//! A replay does not go through the listening socket. It could: the proxy is
//! listening, and a request could be written to it. But then ProxyLab would
//! be a client of itself — a second TCP connection, a `CONNECT` for an HTTPS
//! replay, a second TLS handshake against its own certificate, and a
//! dependency on the listener's address and on the proxy being reachable from
//! inside its own process. None of that is *request* behaviour; all of it is
//! transport ceremony that exists to carry a request from somewhere else, and
//! here there is no somewhere else. So the request is handed to the pipeline
//! at the point where a request arriving over a socket would have been handed
//! to it: decoded, and about to be evaluated.
//!
//! # What a replay is not
//!
//! It is not an edit of the exchange it came from. Every replay is a new
//! [`ExchangeId`](proxy_storage::ExchangeId) and a new row; the exchange a
//! developer copied the request out of is read, if it is read at all, and
//! never written.
//!
//! It is not authenticated. There is no login, no token refresh, and no
//! credential store: an `Authorization` header is sent because it is in the
//! request, and an expired one comes back as whatever the server says about
//! it.
//!
//! It is not a second HTTP client. There is no `curl` process, no `Command`,
//! and no second TLS implementation — the connection is opened by the same
//! two functions the proxy uses for everything else.
//!
//! # The client address
//!
//! A captured exchange names the address its client connected from, and a
//! replay has no client. It is recorded as `127.0.0.1:0` — this machine, on
//! no port — which is what actually happened: the request came from inside
//! ProxyLab. No column was added to say "this was a replay", because saying
//! so would be a schema change, an IPC model change, and a UI change for
//! something the traffic list already shows accurately.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, Uri, Version};
use proxy_http::RequestSpec;
use proxy_ipc::{ReplayFuture, ReplayOutcome, RequestReplayer};
use proxy_storage::{
    BodyCapture, CapturedRequest, ConnectionInfo, HttpVersion, Scheme, TrafficError,
    TrafficErrorKind,
};
use proxy_tls::Host;
use rules::{ModifyResponse, RuleDecision, RuleEngine, Transformation};

use crate::capture::{message, Capture, Recorder};
use crate::controls::{Controls, Exchange, StagedBody};
use crate::http1::reply::ProxyBody;
use crate::http1::upstream::UpstreamError;
use crate::http1::{headers, ruled, service};
use crate::modify;
use crate::shutdown::Shutdown;
use crate::upstream::{self, PooledBody, UpstreamDestination, UpstreamManager};

/// The address a replayed request is recorded as coming from.
///
/// This machine, on no port: the request came from inside ProxyLab.
fn replay_client() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

/// Sends requests through the proxy's own pipeline.
///
/// Holds exactly what the pipeline needs and nothing of its own: the capture
/// the proxy is already publishing and storing through, the interceptor that
/// owns TLS, and the rule engine the listener consults. Cheap to clone, and
/// one is built when the proxy starts.
#[derive(Clone)]
pub struct Replayer {
    capture: Capture,
    upstream: Arc<UpstreamManager>,
    rules: Option<Arc<RuleEngine>>,
    /// Controls that stop nothing and shape nothing. See
    /// [`answered`](Self::answered).
    controls: Controls,
}

impl Replayer {
    /// A replayer that captures through `capture` and reaches destinations
    /// through `upstream` — the proxy's own connection manager, which owns
    /// the interceptor every TLS session here is opened and verified by.
    ///
    /// `rules` is the engine itself rather than a snapshot of it, so a replay
    /// is evaluated against the rules as they are when it is sent — the same
    /// promise a request arriving on the listener gets. A proxy running
    /// without a rule engine allows everything, here as everywhere else.
    pub fn new(
        capture: Capture,
        upstream: Arc<UpstreamManager>,
        rules: Option<Arc<RuleEngine>>,
    ) -> Self {
        Self {
            capture,
            upstream,
            rules,
            controls: Controls::new(),
        }
    }

    /// Sends `request`, and reports what came back.
    ///
    /// Never fails: a request that could not be sent is captured as an
    /// exchange with no response and an error saying why, which is what the
    /// proxy does with a request it could not carry for a browser.
    pub async fn replay(&self, request: RequestSpec) -> ReplayOutcome {
        let recorder = self.begin(&request);
        let id = recorder.id();

        match self.carry(request, &recorder).await {
            Ok(status) => ReplayOutcome::answered(id, status),
            Err(failure) => {
                let error = failure.traffic_error();
                // The cause may be more specific than what is captured, and
                // names a socket or a TLS detail rather than anything the
                // request carried.
                eprintln!("Replay failed: {failure}");
                recorder.fail(error.clone());

                ReplayOutcome::failed(id, error)
            }
        }
    }

    /// Starts recording the exchange this replay will be.
    ///
    /// The head is the request as it was asked for, before the proxy's own
    /// hygiene is applied — the same thing the capture records for a request
    /// that arrived on the listener, which is what the client sent rather
    /// than what went upstream.
    fn begin(&self, request: &RequestSpec) -> Recorder {
        let destination = Some(request.destination());
        let connection = match request.scheme() {
            Scheme::Http => ConnectionInfo::http(replay_client(), destination),
            Scheme::Https => ConnectionInfo::https(replay_client(), destination),
        };

        self.capture.begin(
            connection,
            CapturedRequest {
                method: request.method().to_owned(),
                url: request.url().to_owned(),
                version: HttpVersion::Http11,
                headers: request.headers().clone(),
                body: BodyCapture::empty(),
            },
        )
    }

    /// Evaluates the rules, sends what they allow, and records the answer.
    ///
    /// The same outcome a request on the listener gets: a decision, and the
    /// Modify rules above it — applied to the request if it goes anywhere,
    /// and to the origin's answer if one comes back. See [`crate::modify`].
    async fn carry(&self, request: RequestSpec, recorder: &Recorder) -> Result<u16, ReplayFailure> {
        let outcome = ruled::decide(
            self.rules.as_deref(),
            request.host(),
            request.path(),
            request.method(),
        );
        // Safe to write down as it is: `RequestSpec` refuses a URL with
        // credentials in its authority, so there is no userinfo here for
        // `redact` to take out — see `proxy-http`.
        let target = request.url();

        match &outcome.decision {
            RuleDecision::Allow { .. } | RuleDecision::MapRemote { .. } => {}
            RuleDecision::Block { rule, block } => {
                println!("Rule {rule} blocked replayed {target}");

                return Ok(finish(ruled::blocked(block, recorder)).await);
            }
            RuleDecision::MapLocal { rule, map } => {
                println!("Rule {rule} answered replayed {target} locally");

                return Ok(finish(ruled::mapped_local(map, recorder).await).await);
            }
            RuleDecision::Redirect { rule, redirect } => {
                println!(
                    "Rule {rule} redirected replayed {target} to {}",
                    redirect.location()
                );

                return Ok(finish(ruled::redirected(redirect, recorder)).await);
            }
        }

        let (outgoing, modified) = modify::request(
            outgoing(&request)?.map(StagedBody::unread),
            &outcome.request,
        )
        .await;
        let exchange = if modified {
            let url = format!(
                "{}://{}{}",
                scheme_name(request.scheme()),
                request.host_header(),
                message::path_and_query(outgoing.uri())
            );
            recorder.request_edited(&outgoing, url.clone());

            Exchange::forwarded(
                request.host(),
                outgoing.uri().path(),
                outgoing.method().as_str(),
                url,
            )
        } else {
            Exchange::forwarded(
                request.host(),
                request.path(),
                request.method(),
                request.url(),
            )
        };

        if let RuleDecision::MapRemote { rule, map } = &outcome.decision {
            println!("Rule {rule} mapped replayed {target} to {}", map.target);
            recorder.destination(ruled::mapped_destination(&map.target));

            let outgoing = ruled::into_mapped_request(outgoing, &map.target)
                .map(|body| upstream::boxed(recorder.capture_request(body)));

            recorder.upstream_connect_started();
            let response = ruled::send_mapped(&map.target, &self.upstream, outgoing)
                .await
                .map_err(ReplayFailure::Upstream)?;

            return Ok(self
                .answered(response, recorder, &exchange, &outcome.response)
                .await);
        }

        println!("Replaying {} {target}", request.method());

        let outgoing = outgoing.map(|body| upstream::boxed(recorder.capture_request(body)));

        recorder.upstream_connect_started();
        let response = self.send(&request, outgoing).await?;

        Ok(self
            .answered(response, recorder, &exchange, &outcome.response)
            .await)
    }

    /// Records the response head and drains the body, so the exchange ends.
    ///
    /// The controls are the inert ones this replayer holds rather than the
    /// running proxy's: there is no client at the other end of a replay, so
    /// there is nothing to hold still for a developer to look at and nothing
    /// to deliver slowly. A replay is the UI asking a question, not traffic
    /// from a device being debugged. The Modify Response rules are not
    /// controls but rules, so they apply here as they do everywhere — and
    /// the status returned is the one the exchange is captured with, which
    /// is the origin's unless a rule changed it.
    async fn answered(
        &self,
        response: Response<PooledBody>,
        recorder: &Recorder,
        exchange: &Exchange,
        modify: &[Transformation<ModifyResponse>],
    ) -> u16 {
        recorder.upstream_responded(&response);

        finish(
            service::into_client_response(
                response,
                recorder,
                exchange,
                &self.controls,
                &Shutdown::never(),
                modify,
            )
            .await,
        )
        .await
    }

    /// Sends `outgoing` to the destination the request names.
    ///
    /// `http` is a plain socket and `https` is a TLS session this proxy opens
    /// and whose certificate it verifies — the same session an intercepted
    /// request's origin hop uses, through the same interceptor. There is no
    /// way to ask for an unverified one, here or anywhere else in ProxyLab.
    ///
    /// Which HTTP goes inside it is settled by that session's ALPN, as it is
    /// everywhere else: a destination that agrees to `h2` is replayed to over
    /// HTTP/2, and one that does not is replayed to over HTTP/1.1.
    ///
    /// The connection comes from the proxy's own pool, so a replay to a
    /// destination the proxy is already talking to reuses a connection to it
    /// rather than opening another — and the connection a replay opens is
    /// there for the traffic that follows.
    async fn send(
        &self,
        request: &RequestSpec,
        outgoing: Request<upstream::UpstreamBody>,
    ) -> Result<Response<PooledBody>, ReplayFailure> {
        let destination = match request.scheme() {
            Scheme::Http => UpstreamDestination::plaintext(request.host(), request.port()),
            Scheme::Https => {
                let host = Host::parse(request.host()).map_err(|_| ReplayFailure::UnusableHost)?;

                UpstreamDestination::tls(&host, request.port())
            }
        }
        .map_err(|_| ReplayFailure::UnusableHost)?;

        self.upstream
            .send(&destination, outgoing)
            .await
            .map_err(ReplayFailure::Upstream)
    }
}

/// The exchange's id, and nothing it is carrying.
impl fmt::Debug for Replayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replayer")
            .field("capturing", &true)
            .field("ruled", &self.rules.is_some())
            .finish()
    }
}

impl RequestReplayer for Replayer {
    fn replay(&self, request: RequestSpec) -> ReplayFuture<'_> {
        Box::pin(Replayer::replay(self, request))
    }
}

/// A scheme as a URL writes it.
fn scheme_name(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    }
}

/// Reads a response body to its end and throws it away.
///
/// Reading it is what ends the exchange: the body is wrapped by the recorder,
/// so the last frame is what publishes. Frames are dropped as they arrive
/// rather than collected, so replaying a request that returns a large
/// response costs the capture limit and not the whole response.
async fn finish(response: Response<ProxyBody>) -> u16 {
    let status = response.status().as_u16();
    let mut body = response.into_body();

    while let Some(frame) = body.frame().await {
        // A body that broke part-way is recorded as one by the wrapper; there
        // is nothing to do here but stop reading it.
        if frame.is_err() {
            break;
        }
    }

    status
}

/// Builds the message that goes on the wire.
///
/// Origin form, HTTP/1.1, the proxy's own hygiene applied, and `Host` from
/// the URL — exactly what a forwarded request is rewritten into. The headers
/// have already been checked by `RequestSpec`, so anything hyper refuses here
/// is a disagreement between the two and is reported as the proxy's own
/// failure rather than the request's.
fn outgoing(request: &RequestSpec) -> Result<Request<Full<Bytes>>, ReplayFailure> {
    let uri: Uri = request
        .path_and_query()
        .parse()
        .map_err(|_| ReplayFailure::Unsendable)?;

    let mut outgoing = Request::builder()
        .method(request.method())
        .uri(uri)
        .version(Version::HTTP_11)
        .body(Full::new(Bytes::from(request.body().to_vec())))
        .map_err(|_| ReplayFailure::Unsendable)?;

    for header in request.headers() {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| ReplayFailure::Unsendable)?;
        let value = HeaderValue::from_bytes(header.value.as_bytes())
            .map_err(|_| ReplayFailure::Unsendable)?;

        outgoing.headers_mut().append(name, value);
    }

    headers::strip_hop_by_hop(outgoing.headers_mut());
    let host =
        HeaderValue::from_str(&request.host_header()).map_err(|_| ReplayFailure::Unsendable)?;
    headers::set_host(outgoing.headers_mut(), &host);

    Ok(outgoing)
}

/// Why a replay produced no response.
#[derive(Debug)]
enum ReplayFailure {
    /// The host is one this proxy cannot open a connection to.
    UnusableHost,
    /// The request could not be put on the wire.
    ///
    /// Checked by `RequestSpec` before it got here, so reaching this means
    /// this build's two checks disagree — the proxy's own failure, not the
    /// request's.
    Unsendable,
    /// The destination did not give a usable answer.
    Upstream(UpstreamError),
}

impl ReplayFailure {
    /// The failure as a captured exchange records it.
    ///
    /// A curated phrase per stage, the same ones a forwarded request's
    /// failures are recorded with. Nothing from the request and nothing from
    /// the underlying error.
    fn traffic_error(&self) -> TrafficError {
        match self {
            Self::UnusableHost => TrafficError::new(
                TrafficErrorKind::RequestRejected,
                "that destination is not a host this proxy can reach",
            ),
            Self::Unsendable => TrafficError::new(
                TrafficErrorKind::ProxyFailure,
                "the proxy could not carry out this request",
            ),
            Self::Upstream(error) => TrafficError::new(error.traffic_kind(), error.reason()),
        }
    }
}

/// The log form. A socket error, a TLS stage, or hyper's own message: never
/// a URL, a header, or a body.
impl fmt::Display for ReplayFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnusableHost => f.write_str("the destination is not a host this proxy can name"),
            Self::Unsendable => f.write_str("the request could not be put on the wire"),
            Self::Upstream(error) => error.fmt(f),
        }
    }
}
