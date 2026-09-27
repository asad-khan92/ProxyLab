//! The one host ProxyLab answers as itself: where a phone gets the CA.
//!
//! ```text
//! iPhone, already pointed at the proxy
//!     │  GET http://proxylab.test/
//!     ▼
//! this module ──▶ a page with the steps, and a link to the certificate
//!     │
//!     └────────▶ GET http://proxylab.test/ca.cer
//!                └──▶ the development CA's certificate
//! ```
//!
//! A phone whose Wi-Fi proxy is set to ProxyLab can reach ProxyLab and
//! nothing else about this Mac: there is no file sharing, no AirDrop to
//! automate, and no way to run anything on the phone. What there is, is a
//! browser that is already sending every request through the proxy. So the
//! proxy answers one made-up host by itself, and that is how the certificate
//! gets across.
//!
//! # Why a made-up host
//!
//! [`SETUP_HOST`] is under `.test`, which RFC 6761 reserves for exactly this
//! and which no registry will ever delegate. So answering it takes nothing
//! away: a request for it has never reached anything and never will. Every
//! other host is forwarded exactly as before.
//!
//! # It is the certificate, and only the certificate
//!
//! What is served is the CA's *certificate* — the public half, the same bytes
//! the proxy already presents to every client it intercepts for. The private
//! key is not read here, not named here, and has no route that would reach
//! it. Serving this to whoever asks costs nothing: a certificate is not a
//! secret, and a device that installs it is trusting ProxyLab rather than
//! giving it anything.
//!
//! # It does not install anything
//!
//! The page says what to tap. macOS's keychain is not touched, the phone is
//! not configured, and trusting the certificate is a thing a person does on
//! the device, twice — once to install the profile and once, in a different
//! screen, to turn full trust on. Both are spelled out because the second one
//! is the step everybody misses.

use hyper::header::{HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use hyper::{Response, StatusCode};
use proxy_tls::CertificateAuthority;

use super::headers;
use super::reply::{self, ProxyBody};

/// The host ProxyLab answers as itself.
///
/// Under `.test`, which is reserved and never resolvable, so nothing that
/// would otherwise have worked is being taken over.
pub const SETUP_HOST: &str = "proxylab.test";

/// Media type that makes iOS hand a certificate to Settings rather than
/// showing it as a download.
const CERTIFICATE_TYPE: &str = "application/x-x509-ca-cert";

/// Whether a request target names ProxyLab itself.
///
/// The host as the request spelled it, without a port: a client may write the
/// port and it makes no difference to which host is meant. Compared without
/// regard to case, because host names are not case-sensitive.
pub fn is_setup_host(host: &str) -> bool {
    host.eq_ignore_ascii_case(SETUP_HOST)
}

/// Answers a request addressed to [`SETUP_HOST`].
///
/// Every path is answered — an unknown one with `404` — because the host is
/// ProxyLab's own and forwarding anything addressed to it would be sending a
/// request to a name that does not resolve.
pub fn answer(path: &str, authority: &CertificateAuthority) -> Response<ProxyBody> {
    // The query is not part of which page this is, and there is no parameter
    // any of these take.
    let path = path.split(['?', '#']).next().unwrap_or("/");

    match path {
        "/" | "" | "/index.html" => page(),
        "/ca.cer" => certificate(authority.certificate_der().to_vec(), CERTIFICATE_TYPE),
        "/ca.pem" => certificate(
            authority.certificate_pem().as_bytes().to_vec(),
            "application/x-pem-file",
        ),
        _ => reply::error(StatusCode::NOT_FOUND, "no such ProxyLab setup page"),
    }
}

/// The certificate, as bytes a device will take.
///
/// Never cached: a ProxyLab that has been given a new CA would otherwise hand
/// a phone the old one out of its own browser cache, and the failure that
/// produces — every HTTPS request refused, with a certificate that is
/// installed and trusted — is a miserable one to work out.
fn certificate(bytes: Vec<u8>, media_type: &'static str) -> Response<ProxyBody> {
    let mut response = Response::new(reply::bytes(bytes));

    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(media_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers::set_close(response.headers_mut());

    response
}

/// The page the phone lands on.
///
/// One file, no images, no scripts, and nothing fetched from anywhere: it is
/// being read on a device whose only route to the network is a proxy whose
/// certificate is not trusted yet, so anything it referred to would not load.
fn page() -> Response<ProxyBody> {
    let mut response = Response::new(reply::text(PAGE.to_owned()));

    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers::set_close(response.headers_mut());

    response
}

/// The setup page.
const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ProxyLab setup</title>
<style>
body { font: 17px/1.5 -apple-system, sans-serif; margin: 0; padding: 24px 20px 48px; color: #1d1d1f; background: #fff; }
h1 { font-size: 24px; margin: 0 0 4px; }
p.lead { color: #6e6e73; margin: 0 0 24px; }
a.button { display: block; text-align: center; background: #0071e3; color: #fff; text-decoration: none; padding: 14px; border-radius: 10px; font-weight: 600; margin: 0 0 28px; }
ol { padding-left: 22px; margin: 0 0 28px; }
li { margin-bottom: 10px; }
code { background: #f2f2f7; border-radius: 4px; padding: 1px 5px; font-size: 15px; }
.warn { background: #fff8e6; border-left: 3px solid #ff9f0a; padding: 12px 14px; border-radius: 6px; font-size: 15px; }
@media (prefers-color-scheme: dark) {
  body { background: #000; color: #f5f5f7; }
  code { background: #1c1c1e; }
  .warn { background: #2a2110; }
}
</style>
</head>
<body>
<h1>ProxyLab</h1>
<p class="lead">This device is going through ProxyLab. To see HTTPS traffic, install and trust its certificate.</p>

<a class="button" href="/ca.cer">Download the ProxyLab certificate</a>

<ol>
<li>Tap the button above, then tap <strong>Allow</strong>.</li>
<li>Open <strong>Settings &rsaquo; Profile Downloaded</strong> and tap <strong>Install</strong>. Enter your passcode, then tap <strong>Install</strong> again.</li>
<li>Open <strong>Settings &rsaquo; General &rsaquo; About &rsaquo; Certificate Trust Settings</strong>.</li>
<li>Turn <strong>ProxyLab</strong> on under &ldquo;Enable full trust for root certificates&rdquo;.</li>
</ol>

<p class="warn">Step 4 is separate, and HTTPS will not be readable without it. Turn this certificate back off when you are done debugging: while it is trusted, whoever runs ProxyLab can read this device&rsquo;s encrypted traffic.</p>

<p style="color:#6e6e73;font-size:15px">Plain certificate file: <a href="/ca.pem">ca.pem</a></p>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::BodyExt;
    use hyper::header::CONNECTION;

    fn authority() -> CertificateAuthority {
        CertificateAuthority::generate().expect("generate a CA")
    }

    async fn body_of(response: Response<ProxyBody>) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .expect("the body is complete")
            .to_bytes()
            .to_vec()
    }

    #[test]
    fn the_setup_host_is_under_a_reserved_name() {
        assert!(SETUP_HOST.ends_with(".test"));
    }

    #[test]
    fn the_setup_host_is_recognised_whatever_its_case() {
        assert!(is_setup_host("proxylab.test"));
        assert!(is_setup_host("ProxyLab.Test"));
        assert!(is_setup_host("PROXYLAB.TEST"));
    }

    #[test]
    fn no_other_host_is() {
        for host in [
            "example.com",
            "proxylab.test.example.com",
            "proxylab.local",
            "proxylab",
            "",
        ] {
            assert!(!is_setup_host(host), "{host}");
        }
    }

    #[tokio::test]
    async fn the_root_is_the_setup_page() {
        let response = answer("/", &authority());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");

        let body = String::from_utf8(body_of(response).await).expect("the page is text");
        assert!(body.contains("Certificate Trust Settings"), "{body}");
        assert!(body.contains("/ca.cer"), "{body}");
    }

    #[tokio::test]
    async fn the_page_says_the_trust_step_is_a_separate_one() {
        let body =
            String::from_utf8(body_of(answer("/", &authority())).await).expect("the page is text");

        assert!(body.contains("Enable full trust"), "{body}");
        assert!(body.contains("separate"), "{body}");
    }

    #[tokio::test]
    async fn the_page_says_what_trusting_it_costs() {
        let body =
            String::from_utf8(body_of(answer("/", &authority())).await).expect("the page is text");

        assert!(body.contains("read this device"), "{body}");
    }

    #[tokio::test]
    async fn the_certificate_is_served_as_one_a_phone_will_install() {
        let authority = authority();
        let response = answer("/ca.cer", &authority);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], CERTIFICATE_TYPE);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert_eq!(
            body_of(response).await,
            authority.certificate_der().to_vec()
        );
    }

    #[tokio::test]
    async fn the_certificate_is_also_served_as_text() {
        let authority = authority();
        let response = answer("/ca.pem", &authority);

        assert_eq!(response.headers()[CONTENT_TYPE], "application/x-pem-file");

        let body = String::from_utf8(body_of(response).await).expect("PEM is text");
        assert_eq!(body, authority.certificate_pem());
        assert!(body.starts_with("-----BEGIN CERTIFICATE-----"), "{body}");
    }

    #[tokio::test]
    async fn nothing_served_here_is_a_private_key() {
        let authority = authority();

        for path in ["/", "/ca.cer", "/ca.pem", "/ca-key.pem", "/key"] {
            let body =
                String::from_utf8_lossy(&body_of(answer(path, &authority)).await).into_owned();

            assert!(!body.contains("PRIVATE KEY"), "{path}: {body}");
        }
    }

    #[tokio::test]
    async fn a_query_does_not_change_which_page_is_served() {
        let response = answer("/ca.cer?from=iphone", &authority());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], CERTIFICATE_TYPE);
    }

    #[tokio::test]
    async fn an_unknown_path_is_answered_rather_than_forwarded() {
        let response = answer("/something-else", &authority());

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn every_answer_closes_the_connection() {
        let authority = authority();

        for path in ["/", "/ca.cer", "/ca.pem", "/nothing"] {
            assert_eq!(
                answer(path, &authority).headers()[CONNECTION],
                "close",
                "{path}"
            );
        }
    }
}
