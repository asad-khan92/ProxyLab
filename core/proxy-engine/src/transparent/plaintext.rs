//! A flow that is not TLS: plain HTTP/1.1, as an application sends it to
//! port 80.
//!
//! An application that thinks it is talking to the origin writes an
//! origin-form request — `GET /path HTTP/1.1` and a `Host` header — which a
//! proxy has no way to route. So each request is given the absolute-form
//! target a client of the explicit proxy would have written, and then handed
//! to [`service::proxy`], unchanged: the same breakpoints, rules, capture,
//! upstream pool and WebSocket handling a forwarded request gets.
//!
//! The host is the one the request names in `Host`, because that is the host
//! the application means and the one a rule is written against. A request
//! that names none is sent to the address the flow was made to.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::header::HOST;
use hyper::http::uri::{Authority, Scheme};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Uri};
use hyper_util::rt::TokioIo;
use proxy_tls::Interceptor;
use rules::RuleEngine;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::capture::Capture;
use crate::controls::Controls;
use crate::http1::connect_target::ConnectTarget;
use crate::http1::service;
use crate::shutdown::Shutdown;
use crate::upstream::UpstreamManager;

/// The port an `http` URL does not need to name.
const DEFAULT_HTTP_PORT: u16 = 80;

/// Serves the plain HTTP an application sent into a flow to `target`.
#[allow(clippy::too_many_arguments)]
pub async fn serve<S>(
    stream: S,
    client: SocketAddr,
    target: ConnectTarget,
    interceptor: Arc<Interceptor>,
    upstream: Arc<UpstreamManager>,
    capture: Capture,
    rules: Option<Arc<RuleEngine>>,
    controls: Controls,
    shutdown: Shutdown,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let service = service_fn(move |request| {
        let request = absolute(request, &target);
        let interceptor = Arc::clone(&interceptor);
        let upstream = Arc::clone(&upstream);
        let capture = capture.clone();
        let rules = rules.clone();
        let controls = controls.clone();
        let shutdown = shutdown.clone();

        async move {
            Ok::<_, Infallible>(
                service::proxy(
                    client,
                    request,
                    interceptor,
                    upstream,
                    &capture,
                    rules,
                    controls,
                    shutdown,
                )
                .await,
            )
        }
    });

    // Upgrades, because a `ws://` handshake on port 80 hands its connection
    // over once the `101` is written, exactly as it does through the explicit
    // proxy.
    let served = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades()
        .await;

    if let Err(error) = served {
        eprintln!("{client} transparent HTTP connection error: {error}");
    }
}

/// `request`, with the absolute-form target a proxy routes by.
///
/// A request that already carries one — an application that believes it is
/// talking to a proxy — and a `CONNECT` are left as they are: both are
/// already in the form the explicit proxy's service reads.
fn absolute<B>(mut request: Request<B>, target: &ConnectTarget) -> Request<B> {
    if request.method() == Method::CONNECT || request.uri().scheme().is_some() {
        return request;
    }

    let authority = named_authority(&request).unwrap_or_else(|| flow_authority(target));
    let mut parts = request.uri().clone().into_parts();
    parts.scheme = Some(Scheme::HTTP);
    parts.authority = Some(authority);
    if parts.path_and_query.is_none() {
        parts.path_and_query = Some("/".parse().expect("a path is a path"));
    }

    if let Ok(uri) = Uri::from_parts(parts) {
        *request.uri_mut() = uri;
    }

    request
}

/// The authority the request's `Host` header names, if it names one.
///
/// Userinfo is not part of a `Host` header, and a value carrying it is not
/// taken: it would put credentials into the URL that is captured and shown.
fn named_authority<B>(request: &Request<B>) -> Option<Authority> {
    let value = request.headers().get(HOST)?.to_str().ok()?;
    if value.is_empty() || value.contains('@') {
        return None;
    }

    value.parse().ok()
}

/// The authority of the address the flow was made to.
fn flow_authority(target: &ConnectTarget) -> Authority {
    let host = if target.host().contains(':') {
        format!("[{}]", target.host())
    } else {
        target.host().to_owned()
    };
    let text = if target.port() == DEFAULT_HTTP_PORT {
        host
    } else {
        format!("{host}:{}", target.port())
    };

    text.parse()
        .expect("a parsed CONNECT target is an authority")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> ConnectTarget {
        ConnectTarget::parse(&text.parse().expect("uri")).expect("target")
    }

    fn request(method: &str, uri: &str, host: Option<&str>) -> Request<()> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(host) = host {
            builder = builder.header(HOST, host);
        }

        builder.body(()).expect("request")
    }

    fn rewritten(request: Request<()>, flow: &str) -> String {
        absolute(request, &target(flow)).uri().to_string()
    }

    #[test]
    fn an_origin_form_request_is_sent_to_the_host_it_names() {
        assert_eq!(
            rewritten(
                request("GET", "/path?q=1", Some("example.com")),
                "93.184.215.14:80"
            ),
            "http://example.com/path?q=1"
        );
    }

    #[test]
    fn a_port_in_the_host_header_is_kept() {
        assert_eq!(
            rewritten(
                request("GET", "/", Some("example.com:8080")),
                "93.184.215.14:8080"
            ),
            "http://example.com:8080/"
        );
    }

    #[test]
    fn a_request_naming_no_host_goes_to_the_address_the_flow_was_made_to() {
        assert_eq!(
            rewritten(request("GET", "/", None), "93.184.215.14:80"),
            "http://93.184.215.14/"
        );
        assert_eq!(
            rewritten(request("GET", "/", None), "93.184.215.14:8080"),
            "http://93.184.215.14:8080/"
        );
        assert_eq!(
            rewritten(request("GET", "/", None), "[2001:db8::1]:80"),
            "http://[2001:db8::1]/"
        );
    }

    #[test]
    fn a_host_header_carrying_credentials_is_not_put_in_the_url() {
        let url = rewritten(
            request("GET", "/", Some("user:secret@example.com")),
            "93.184.215.14:80",
        );

        assert_eq!(url, "http://93.184.215.14/");
    }

    #[test]
    fn a_host_header_that_is_not_an_authority_is_not_used() {
        assert_eq!(
            rewritten(request("GET", "/", Some("exa mple")), "93.184.215.14:80"),
            "http://93.184.215.14/"
        );
    }

    #[test]
    fn a_request_already_in_absolute_form_is_left_alone() {
        assert_eq!(
            rewritten(
                request("GET", "http://other.test/x", Some("example.com")),
                "93.184.215.14:80"
            ),
            "http://other.test/x"
        );
    }

    #[test]
    fn a_connect_is_left_for_the_explicit_proxy_to_answer() {
        assert_eq!(
            rewritten(
                request("CONNECT", "example.com:443", None),
                "93.184.215.14:80"
            ),
            "example.com:443"
        );
    }
}
