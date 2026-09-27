//! The breakpoints the proxy is holding traffic on, and what is held.
//!
//! ```text
//!   configured                    pending
//!   ┌──────────────────┐          ┌───────────────────────────┐
//!   │ request  api.…   │  match   │ pause 1 ─▶ oneshot sender │
//!   │ response api.…   │ ───────▶ │ pause 2 ─▶ oneshot sender │
//!   └──────────────────┘          └───────────────────────────┘
//!            ▲                                  │
//!            │ create/update/delete             │ resolve
//!            └──────────── IPC ─────────────────┘
//! ```
//!
//! Two tables and a count. The configured breakpoints say what to stop; the
//! pending pauses are the exchanges stopped right now, each holding the
//! sender of a channel the connection carrying it is waiting on; and the
//! count is how many clients have asked to be told about pauses.
//!
//! Everything here is in memory and nothing waits: matching is a walk down a
//! list a person wrote, and resolving a pause is a send on a channel. The
//! waiting is all on the other end, in [`Pending::wait`].
//!
//! # The count is the safety
//!
//! A pause is only created while at least one client is watching, and every
//! pending pause is released the moment the last one stops. There is no
//! reconciliation to get wrong: a UI that crashes drops its stream, the
//! stream's `Drop` calls [`detach`](BreakpointControl::detach), and the
//! exchanges that were being held carry on unchanged. A watcher that stays
//! connected and stops answering is covered by [`PAUSE_DEADLINE`].
//!
//! # Nothing about a pause is logged
//!
//! A held request is a request, with its headers and its body in it. The
//! proxy's own per-request reporting says an exchange was paused and by which
//! breakpoint, and nothing else about it reaches a log.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use proxy_ipc::{
    Breakpoint, BreakpointControl, BreakpointDirection, BreakpointId, BreakpointUpdate,
    NewBreakpoint, PauseDecision, PauseId, PauseNotice, PausedExchange, ReleaseReason,
};
use rules::RequestContext;
use tokio::sync::{broadcast, oneshot};
use tokio::time::{sleep, Duration};

use crate::shutdown::Shutdown;

/// How long an exchange may be held before it carries on by itself.
///
/// The second half of the disconnect story, and the weaker half: the first is
/// that a pause does not outlive the clients that could answer it, which
/// covers a UI that goes away. This covers the one it does not — a UI that is
/// still connected and is not going to answer, because it is wedged, or
/// because its window was closed without the pause being resolved.
///
/// Two minutes is long enough to read a body and change it, and short enough
/// that an exchange nobody is coming back to is not held past the point where
/// the client at the other end has given up on it.
pub const PAUSE_DEADLINE: Duration = Duration::from_secs(120);

/// The breakpoints this proxy is holding traffic on.
///
/// One of these exists per process. The IPC server configures it, the
/// pipeline consults it, and neither of them owns it.
#[derive(Debug)]
pub struct Breakpoints {
    state: Mutex<State>,
    notices: broadcast::Sender<PauseNotice>,
}

/// Everything the breakpoints know.
#[derive(Default)]
struct State {
    /// What to stop, in the order it was configured.
    configured: Vec<Breakpoint>,
    /// What is stopped right now, and how to tell each one what to do.
    pending: HashMap<PauseId, oneshot::Sender<PauseDecision>>,
    /// How many clients have asked to be told about pauses.
    watchers: usize,
}

impl Breakpoints {
    /// A proxy that is stopping nothing.
    pub fn new() -> Self {
        let (notices, _) = broadcast::channel(proxy_ipc::breakpoint::PAUSE_STREAM_CAPACITY);

        Self {
            state: Mutex::new(State::default()),
            notices,
        }
    }

    /// The breakpoints' state.
    ///
    /// A poisoned lock is recovered from rather than panicked on, for the
    /// reason the capture recorder's is: something else has already panicked,
    /// and a second panic on a connection task would take the proxy down over
    /// a debugging aid.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Which breakpoint would stop this exchange, if any.
    ///
    /// `None` when nothing is watching, which is what keeps a proxy nobody is
    /// looking at from ever pausing — and what makes this cheap enough to ask
    /// on every request.
    pub fn matching(
        &self,
        direction: BreakpointDirection,
        request: &RequestContext<'_>,
    ) -> Option<BreakpointId> {
        let state = self.lock();
        if state.watchers == 0 {
            return None;
        }

        state
            .configured
            .iter()
            .find(|breakpoint| {
                breakpoint.enabled
                    && breakpoint.direction == direction
                    && breakpoint.matcher.matches(request)
            })
            .map(|breakpoint| breakpoint.id)
    }

    /// Starts holding `paused` still, and tells whoever is watching.
    ///
    /// `None` when the last watcher went away between the match and here, in
    /// which case the exchange carries on as if nothing had matched.
    pub fn begin(&self, paused: PausedExchange) -> Option<Pending> {
        let pause_id = paused.pause_id;
        let (sender, decision) = oneshot::channel();

        {
            let mut state = self.lock();
            if state.watchers == 0 {
                return None;
            }
            state.pending.insert(pause_id, sender);
        }

        // Outside the lock: a send on a broadcast channel does not wait, but
        // nothing that reaches a client belongs inside a lock the request
        // path takes.
        let _ = self.notices.send(PauseNotice::Paused(paused));

        Some(Pending { pause_id, decision })
    }

    /// Stops holding one exchange, whatever the reason.
    ///
    /// The decision is sent to whoever is waiting, and everyone watching is
    /// told the pause is over — including the client that asked for it, which
    /// has the pause on screen and has to stop showing it.
    fn end(&self, pause_id: PauseId, decision: PauseDecision, reason: ReleaseReason) -> bool {
        let Some(sender) = self.lock().pending.remove(&pause_id) else {
            return false;
        };

        // A receiver that is gone is an exchange whose connection ended while
        // it was held. Nothing to tell, and nothing wrong.
        let _ = sender.send(decision);
        let _ = self
            .notices
            .send(PauseNotice::Released { pause_id, reason });

        true
    }

    /// Stops holding one exchange that nobody answered in time.
    fn expire(&self, pause_id: PauseId, reason: ReleaseReason) {
        self.end(pause_id, PauseDecision::unchanged(), reason);
    }

    /// Stops holding everything, because there is nobody left to ask.
    fn release_all(&self) {
        let pending: Vec<PauseId> = self.lock().pending.keys().copied().collect();

        for pause_id in pending {
            self.end(
                pause_id,
                PauseDecision::unchanged(),
                ReleaseReason::Abandoned,
            );
        }
    }

    /// How many exchanges are being held still.
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }

    /// Whether anything would be paused at all.
    pub fn is_watched(&self) -> bool {
        self.lock().watchers > 0
    }
}

impl Default for Breakpoints {
    fn default() -> Self {
        Self::new()
    }
}

impl BreakpointControl for Breakpoints {
    fn list(&self) -> Vec<Breakpoint> {
        self.lock().configured.clone()
    }

    fn create(&self, new: NewBreakpoint) -> Breakpoint {
        let breakpoint = Breakpoint {
            id: BreakpointId::next(),
            enabled: new.enabled,
            direction: new.direction,
            matcher: new.matcher,
        };
        self.lock().configured.push(breakpoint.clone());

        breakpoint
    }

    fn update(&self, id: BreakpointId, update: BreakpointUpdate) -> Option<Breakpoint> {
        let mut state = self.lock();
        let breakpoint = state
            .configured
            .iter_mut()
            .find(|breakpoint| breakpoint.id == id)?;

        if let Some(enabled) = update.enabled {
            breakpoint.enabled = enabled;
        }
        if let Some(direction) = update.direction {
            breakpoint.direction = direction;
        }
        if let Some(matcher) = update.matcher {
            breakpoint.matcher = matcher;
        }

        Some(breakpoint.clone())
    }

    /// Removes a breakpoint, leaving whatever it is already holding alone.
    ///
    /// A pause is not the breakpoint that caused it: the exchange is still
    /// held, and still has to be told what to do. Releasing it here would be
    /// deciding on the developer's behalf at the moment they were tidying up
    /// a list.
    fn delete(&self, id: BreakpointId) -> bool {
        let mut state = self.lock();
        let before = state.configured.len();
        state.configured.retain(|breakpoint| breakpoint.id != id);

        state.configured.len() != before
    }

    fn resolve(&self, id: PauseId, decision: PauseDecision) -> bool {
        let reason = match decision {
            PauseDecision::Continue { .. } => ReleaseReason::Continued,
            PauseDecision::Cancel => ReleaseReason::Cancelled,
        };

        self.end(id, decision, reason)
    }

    fn attach(&self) -> broadcast::Receiver<PauseNotice> {
        // Subscribed before the count is raised, so a pause that starts
        // between the two is one this client receives.
        let receiver = self.notices.subscribe();
        self.lock().watchers += 1;

        receiver
    }

    fn detach(&self) {
        let last = {
            let mut state = self.lock();
            state.watchers = state.watchers.saturating_sub(1);

            state.watchers == 0
        };

        if last {
            self.release_all();
        }
    }
}

/// One exchange being held still, from the pipeline's side.
///
/// Holding one is what stops the request going upstream: the task carrying it
/// is sitting in [`wait`](Self::wait) and will not go on until this is
/// resolved, expires, or the proxy stops.
#[derive(Debug)]
pub struct Pending {
    pause_id: PauseId,
    decision: oneshot::Receiver<PauseDecision>,
}

impl Pending {
    /// Which pause this is.
    pub fn id(&self) -> PauseId {
        self.pause_id
    }

    /// Waits to be told what to do with the exchange.
    ///
    /// Three ways out, and every one of them produces a decision:
    ///
    /// | What happened | What the exchange does |
    /// | --- | --- |
    /// | a client decided | what it said |
    /// | [`PAUSE_DEADLINE`] passed | carries on unchanged |
    /// | the proxy is stopping | carries on unchanged |
    ///
    /// There is no fourth way, and none of them is "stays here": an exchange
    /// that reaches this function always leaves it.
    pub async fn wait(self, breakpoints: &Breakpoints, shutdown: &Shutdown) -> PauseDecision {
        let pause_id = self.pause_id;

        tokio::select! {
            decided = self.decision => decided.unwrap_or_else(|_| PauseDecision::unchanged()),
            () = sleep(PAUSE_DEADLINE) => {
                breakpoints.expire(pause_id, ReleaseReason::TimedOut);

                PauseDecision::unchanged()
            }
            () = shutdown.signalled() => {
                breakpoints.expire(pause_id, ReleaseReason::Abandoned);

                PauseDecision::unchanged()
            }
        }
    }
}

/// How many breakpoints there are and how many exchanges are held, and
/// nothing about either. A matcher names hosts and paths a developer is
/// working on; a pause is a request with its body in it.
impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("configured", &self.configured.len())
            .field("pending", &self.pending.len())
            .field("watchers", &self.watchers)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use proxy_ipc::{PauseStream, PausedRequest};
    use proxy_storage::{BodyCapture, Headers};
    use rules::RuleMatcher;
    use std::sync::Arc;

    fn breakpoints() -> Arc<Breakpoints> {
        Arc::new(Breakpoints::new())
    }

    /// A client watching, which is what makes anything pause at all.
    fn watching(breakpoints: &Arc<Breakpoints>) -> PauseStream {
        PauseStream::attach(Arc::clone(breakpoints) as Arc<dyn BreakpointControl>)
    }

    fn on_requests_to(host: &str) -> NewBreakpoint {
        NewBreakpoint::new(
            BreakpointDirection::Request,
            RuleMatcher::any().with_host(host),
        )
    }

    fn paused(breakpoint_id: BreakpointId) -> PausedExchange {
        PausedExchange {
            pause_id: PauseId::next(),
            breakpoint_id,
            direction: BreakpointDirection::Request,
            method: "GET".to_owned(),
            url: "https://api.example.com/x".to_owned(),
            request: Some(PausedRequest {
                method: "GET".to_owned(),
                url: "https://api.example.com/x".to_owned(),
                headers: Headers::new(),
                body: BodyCapture::empty(),
            }),
            response: None,
            websocket: None,
        }
    }

    fn request<'a>() -> RequestContext<'a> {
        RequestContext::new("api.example.com", "/x", "GET")
    }

    #[test]
    fn a_proxy_nobody_is_watching_pauses_nothing() {
        let breakpoints = breakpoints();
        breakpoints.create(on_requests_to("api.example.com"));

        assert!(!breakpoints.is_watched());
        assert_eq!(
            breakpoints.matching(BreakpointDirection::Request, &request()),
            None
        );
    }

    #[test]
    fn a_watched_proxy_matches_the_first_enabled_breakpoint() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let disabled = breakpoints.create(NewBreakpoint {
            enabled: false,
            ..on_requests_to("api.example.com")
        });
        let enabled = breakpoints.create(on_requests_to("api.example.com"));

        let matched = breakpoints.matching(BreakpointDirection::Request, &request());

        assert_eq!(matched, Some(enabled.id));
        assert_ne!(matched, Some(disabled.id));
    }

    #[test]
    fn a_breakpoint_only_stops_the_half_it_is_for() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        breakpoints.create(on_requests_to("api.example.com"));

        assert!(breakpoints
            .matching(BreakpointDirection::Request, &request())
            .is_some());
        assert!(breakpoints
            .matching(BreakpointDirection::Response, &request())
            .is_none());
    }

    #[test]
    fn a_breakpoint_for_another_host_does_not_match() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        breakpoints.create(on_requests_to("other.example.com"));

        assert!(breakpoints
            .matching(BreakpointDirection::Request, &request())
            .is_none());
    }

    #[test]
    fn a_breakpoint_can_be_turned_off_and_on_again() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));

        let updated = breakpoints
            .update(
                breakpoint.id,
                BreakpointUpdate {
                    enabled: Some(false),
                    ..BreakpointUpdate::default()
                },
            )
            .expect("the breakpoint exists");

        assert!(!updated.enabled);
        assert!(breakpoints
            .matching(BreakpointDirection::Request, &request())
            .is_none());
    }

    #[test]
    fn updating_a_breakpoint_that_is_not_there_says_so() {
        let breakpoints = breakpoints();

        assert!(breakpoints
            .update(BreakpointId::next(), BreakpointUpdate::default())
            .is_none());
        assert!(!breakpoints.delete(BreakpointId::next()));
    }

    #[test]
    fn a_deleted_breakpoint_stops_matching() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));

        assert!(breakpoints.delete(breakpoint.id));
        assert!(breakpoints.list().is_empty());
        assert!(breakpoints
            .matching(BreakpointDirection::Request, &request())
            .is_none());
    }

    #[tokio::test]
    async fn a_pause_carries_the_decision_it_is_given() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));

        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");
        assert_eq!(breakpoints.pending(), 1);

        assert!(breakpoints.resolve(pending.id(), PauseDecision::Cancel));
        assert_eq!(
            pending.wait(&breakpoints, &Shutdown::never()).await,
            PauseDecision::Cancel
        );
        assert_eq!(breakpoints.pending(), 0);
    }

    #[tokio::test]
    async fn a_pause_that_is_already_over_cannot_be_resolved_again() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");

        assert!(breakpoints.resolve(pending.id(), PauseDecision::Cancel));
        assert!(!breakpoints.resolve(pending.id(), PauseDecision::Cancel));
    }

    #[tokio::test]
    async fn the_last_watcher_going_away_releases_everything_held() {
        let breakpoints = breakpoints();
        let watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");

        drop(watcher);

        assert_eq!(breakpoints.pending(), 0);
        assert_eq!(
            pending.wait(&breakpoints, &Shutdown::never()).await,
            PauseDecision::unchanged()
        );
    }

    #[tokio::test]
    async fn one_watcher_going_away_does_not_release_another_watchers_pauses() {
        let breakpoints = breakpoints();
        let first = watching(&breakpoints);
        let _second = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");

        drop(first);

        assert_eq!(breakpoints.pending(), 1);
        assert!(breakpoints.resolve(pending.id(), PauseDecision::Cancel));
    }

    #[tokio::test]
    async fn a_pause_that_starts_with_nobody_watching_never_starts() {
        let breakpoints = breakpoints();
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));

        assert!(breakpoints.begin(paused(breakpoint.id)).is_none());
        assert_eq!(breakpoints.pending(), 0);
    }

    #[tokio::test]
    async fn a_stopping_proxy_does_not_wait_for_a_decision() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");

        let (signal, shutdown) = Shutdown::channel();
        signal.stop();

        assert_eq!(
            pending.wait(&breakpoints, &shutdown).await,
            PauseDecision::unchanged()
        );
        assert_eq!(breakpoints.pending(), 0);
    }

    #[tokio::test]
    async fn a_watcher_is_told_what_was_paused_and_what_released_it() {
        let breakpoints = breakpoints();
        let mut watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");
        let pause_id = pending.id();

        match watcher.recv().await.expect("the stream is healthy") {
            PauseNotice::Paused(paused) => assert_eq!(paused.pause_id, pause_id),
            notice => panic!("expected a pause: {notice:?}"),
        }

        breakpoints.resolve(pause_id, PauseDecision::Cancel);

        match watcher.recv().await.expect("the stream is healthy") {
            PauseNotice::Released {
                pause_id: id,
                reason,
            } => {
                assert_eq!(id, pause_id);
                assert_eq!(reason, ReleaseReason::Cancelled);
            }
            notice => panic!("expected a release: {notice:?}"),
        }
    }

    #[test]
    fn deleting_a_breakpoint_leaves_what_it_is_holding_alone() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        let _pending = breakpoints
            .begin(paused(breakpoint.id))
            .expect("somebody is watching");

        assert!(breakpoints.delete(breakpoint.id));
        assert_eq!(breakpoints.pending(), 1);
    }

    #[test]
    fn the_breakpoints_never_render_what_they_are_holding() {
        let breakpoints = breakpoints();
        let _watcher = watching(&breakpoints);
        let breakpoint = breakpoints.create(on_requests_to("api.example.com"));
        breakpoints.begin(paused(breakpoint.id));

        let rendered = format!("{breakpoints:?}");

        assert!(!rendered.contains("api.example.com"), "{rendered}");
    }
}
