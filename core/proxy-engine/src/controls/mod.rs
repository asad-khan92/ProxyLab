//! What a developer is doing to the traffic as it goes past.
//!
//! ```text
//!                       ┌─────────────┐
//!  request ────────────▶│  Controls   │────────────▶ upstream
//!                       │             │
//!                       │ breakpoints │  hold it still and edit it
//!                       │ throttle    │  make the link slower than it is
//!                       └─────────────┘
//!  response ◀───────────────────────────────────────
//! ```
//!
//! Two things, one handle. They are together because they are threaded
//! through the same places and are the same kind of thing — live, deliberate
//! interference with traffic the proxy is carrying, turned on from the UI and
//! off again — and because a pipeline function that took one more argument
//! for each of them separately would take two.
//!
//! Neither is a rule. A rule is a standing decision the proxy makes by itself
//! (see `rules`); these are a person doing something to one exchange, or to
//! every exchange while they watch. Neither is persisted, for the reason LAN
//! exposure is not: a ProxyLab that has just started carries traffic exactly
//! as the network does.
//!
//! # Where they are applied
//!
//! ```text
//! request arrives
//!    │
//!    ├─ breakpoint: held still before anything else looks at it
//!    │              so what the rules, the capture, and the origin
//!    │              see is the edited request
//!    ├─ latency:    waited once, before the rules decide
//!    ├─ upload:     the request body, paced on its way upstream
//!    ▼
//! response arrives
//!    │
//!    ├─ breakpoint: held still before the client is given any of it
//!    ├─ download:   the response body, paced on its way back
//!    ▼
//! client
//! ```
//!
//! A request breakpoint stops the request *before* the rules are consulted,
//! and that is deliberate: the request a developer edited is the request
//! ProxyLab is carrying, so it is the one the rules match against, the one
//! the capture records, and the one the origin receives. An edit that
//! happened after the rules had decided would be an edit the traffic list
//! disagreed with.
//!
//! # What is shaped, and what is not
//!
//! Bandwidth is applied to the bodies that cross the proxy — an HTTP/1.1
//! body, an HTTP/2 stream's body, a server-sent event stream — and to a
//! WebSocket's two directions once it has stopped being HTTP. It is not
//! applied to a response ProxyLab generated itself: a Block rule's refusal
//! and a Map Local file are answers that never crossed a network, and pacing
//! them would be simulating a link they did not travel over. Added latency is
//! applied to those, because the request did travel to the proxy.

pub mod breakpoint;
pub mod pause;
pub mod stage;
pub mod throttle;

use std::sync::Arc;

use rules::RequestContext;

pub use breakpoint::{Breakpoints, PAUSE_DEADLINE};
pub use pause::{RequestOutcome, ResponseOutcome, WebSocketOutcome};
pub use stage::StagedBody;
pub use throttle::{Throttle, Throttled};

/// The breakpoints and the throttling one proxy is running with.
///
/// Cheap to clone: a clone shares the same breakpoints and the same
/// throttling, which is how one set of controls serves every connection. A
/// clone is what each connection is handed, the same way it is handed the
/// capture and the rule engine.
#[derive(Clone, Debug, Default)]
pub struct Controls {
    breakpoints: Arc<Breakpoints>,
    throttle: Arc<Throttle>,
}

impl Controls {
    /// Controls that are stopping nothing and shaping nothing.
    ///
    /// Which is also what they are until the UI says otherwise: a breakpoint
    /// has to be configured *and* watched before anything pauses, and
    /// throttling has to be switched on before anything is paced.
    pub fn new() -> Self {
        Self::default()
    }

    /// The breakpoints, for the IPC server to configure and to watch.
    pub fn breakpoints(&self) -> &Arc<Breakpoints> {
        &self.breakpoints
    }

    /// The throttling, for the IPC server to read and to change.
    pub fn throttle(&self) -> &Arc<Throttle> {
        &self.throttle
    }
}

/// Which request an exchange is, for the purpose of stopping it.
///
/// Three fields a breakpoint matches on and two a paused exchange is labelled
/// with, built once where a request's destination becomes known and carried
/// through to where its response comes back — because a response has no host,
/// path, or method of its own to match on.
#[derive(Clone, Debug)]
pub struct Exchange {
    /// The destination host, without a port and without IPv6 brackets.
    pub host: String,
    /// The request path, without the query string.
    pub path: String,
    /// The request method, as it was sent.
    pub method: String,
    /// The URL the exchange is for, as the capture records it.
    pub url: String,
    /// How this request's target is spelled, which is what an edited URL
    /// replaces.
    pub form: TargetForm,
}

impl Exchange {
    /// A plaintext request, whose target is the absolute URL it named.
    pub fn forwarded(
        host: impl Into<String>,
        path: impl Into<String>,
        method: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            path: path.into(),
            method: method.into(),
            url: url.into(),
            form: TargetForm::Absolute,
        }
    }

    /// A request read out of an intercepted TLS session, whose target is the
    /// path its session's host was asked for.
    pub fn intercepted(
        host: impl Into<String>,
        path: impl Into<String>,
        method: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            path: path.into(),
            method: method.into(),
            url: url.into(),
            form: TargetForm::Origin,
        }
    }

    /// The parts a breakpoint's matcher is asked about.
    ///
    /// The same [`RequestContext`] the rule engine is given, so "this host,
    /// this path, this method" means exactly one thing in ProxyLab.
    pub fn context(&self) -> RequestContext<'_> {
        RequestContext::new(&self.host, &self.path, &self.method)
    }
}

/// How a request's target is written on the wire.
///
/// The one thing an edited URL has to know: see [`pause`] for what each of
/// them means an edit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetForm {
    /// `http://example.com/a?b` — a plaintext request through the proxy.
    Absolute,
    /// `/a?b` — a request inside an intercepted TLS session.
    Origin,
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_ipc::ThrottleSettings;

    #[test]
    fn new_controls_stop_nothing_and_shape_nothing() {
        let controls = Controls::new();

        assert!(!controls.breakpoints().is_watched());
        assert_eq!(controls.breakpoints().pending(), 0);
        assert_eq!(controls.throttle().settings(), ThrottleSettings::none());
    }

    #[test]
    fn a_clone_shares_the_same_controls() {
        let controls = Controls::new();
        let clone = controls.clone();

        clone.throttle().set(ThrottleSettings {
            enabled: true,
            latency_ms: 25,
            ..ThrottleSettings::none()
        });

        assert_eq!(controls.throttle().settings().latency(), Some(25));
    }

    #[test]
    fn an_exchange_is_matched_on_its_host_path_and_method() {
        let exchange = Exchange::forwarded(
            "api.example.com",
            "/orders",
            "POST",
            "http://api.example.com/orders?page=2",
        );
        let context = exchange.context();

        assert_eq!(context.host, "api.example.com");
        assert_eq!(context.path, "/orders");
        assert_eq!(context.method, "POST");
        assert_eq!(exchange.form, TargetForm::Absolute);
    }

    #[test]
    fn an_intercepted_exchange_names_its_target_in_origin_form() {
        let exchange =
            Exchange::intercepted("api.example.com", "/x", "GET", "https://api.example.com/x");

        assert_eq!(exchange.form, TargetForm::Origin);
    }
}
