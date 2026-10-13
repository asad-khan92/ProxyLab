//! Carrying out what a request asked for.
//!
//! This is the whole of the IPC server's access to ProxyLab: a client can
//! reach the stored traffic, the rules, and the controls through the
//! operations below and through nothing else. There is no path from a request
//! to a file of a client's choosing, or to a query the server did not write.
//! `diagnostics` is the one question about the process itself, and it reads
//! the listeners, the LAN control, and an in-memory buffer — never a captured
//! exchange.
//!
//! No SQL lives in this crate. Every operation is a call into
//! `proxy-storage`, which already owns the schema, the queries, and the body
//! files.
//!
//! # Keeping the database off the runtime
//!
//! Storage is synchronous, because SQLite is. Calling it straight from a
//! connection task would block a runtime thread — the same thread the proxy's
//! connections are running on — so every call goes through
//! [`spawn_blocking`](tokio::task::spawn_blocking). A slow query then costs a
//! blocking thread and nothing else.
//!
//! # What a failure tells the client
//!
//! The code, and a phrase. A storage failure names a database file and an
//! operating-system error, which is worth having in the proxy's own log and
//! is not worth sending to a client that cannot act on it; the client is told
//! the history could not be read.

use std::sync::Arc;

use proxy_storage::{
    EventPublisher, EventSubscriber, NewSavedRequest, SavedRequest, SavedRequestError, Storage,
    StorageError, StreamPublisher, StreamSubscriber, TrafficPage, TrafficQuery,
};
use rules::{Rule, RuleEngine, RuleSet};

use crate::breakpoint::{BreakpointControl, PauseStream};
use crate::diagnostics::{self, DiagnosticsControl};
use crate::lan::LanControl;
use crate::protocol::{
    BreakpointDeleted, BreakpointDetail, BreakpointList, BreakpointOperation, Cleared,
    CurlGenerated, CurlParsed, CurlProblem, Deleted, DeveloperOperation, ErrorBody, ErrorCode,
    HarExport, HarExported, HarImported, HarOperation, HarSelection, LanOperation, Outcome,
    PauseResolved, RuleDeleted, RuleDetail, RuleList, RuleOperation, SavedRequestDeleted,
    SavedRequestDetail, SavedRequestList, SavedRequestOperation, StreamMessageDetail,
    ThrottleOperation, TrafficDetail, TrafficList, TrafficOperation, MAX_EXPORT_ENTRIES,
};
use crate::replay::RequestReplayer;
use crate::throttle::ThrottleControl;

/// What the IPC server is allowed to do on a client's behalf.
///
/// Cheap to clone: a clone shares the same database handle and the same event
/// stream, which is how one service serves every connection.
#[derive(Clone, Debug)]
pub struct TrafficService {
    storage: Storage,
    publisher: EventPublisher,
    stream_publisher: StreamPublisher,
    engine: Option<Arc<RuleEngine>>,
    replayer: Option<Arc<dyn RequestReplayer>>,
    lan: Option<Arc<dyn LanControl>>,
    breakpoints: Option<Arc<dyn BreakpointControl>>,
    throttle: Option<Arc<dyn ThrottleControl>>,
    diagnostics: Option<Arc<dyn DiagnosticsControl>>,
}

impl TrafficService {
    /// A service over `storage`, publishing from `publisher`.
    ///
    /// The publisher is the proxy's own: subscribing a client attaches
    /// another subscriber to the stream the proxy already publishes on,
    /// rather than starting a second stream beside it.
    ///
    /// Rule operations are refused until [`serving_rules`](Self::serving_rules)
    /// gives the service the engine to keep in step. A service with no engine
    /// could still write rules to the database, and the running proxy would
    /// go on using the ones it loaded at startup — a rule that appears to have
    /// been created and does nothing.
    pub fn new(storage: Storage, publisher: EventPublisher) -> Self {
        Self {
            storage,
            publisher,
            stream_publisher: StreamPublisher::new(),
            engine: None,
            replayer: None,
            lan: None,
            breakpoints: None,
            throttle: None,
            diagnostics: None,
        }
    }

    /// The same service, also publishing captured WebSocket messages and SSE
    /// events from `publisher`.
    ///
    /// The publisher is the proxy's own, the same way the traffic publisher
    /// is: a client that has subscribed is attached to the stream the proxy
    /// already publishes on.
    pub fn publishing_streams(mut self, publisher: StreamPublisher) -> Self {
        self.stream_publisher = publisher;
        self
    }

    /// The same service, also sending requests through `replayer`.
    ///
    /// Replay is refused until this is given, for the same reason rules are:
    /// a server with nothing to send a request through could accept a replay
    /// and do nothing with it, which is worse than saying so. Nothing else
    /// about the service changes — a replayed request is captured, stored,
    /// and published by the proxy itself, so this service never sees it
    /// again except as an exchange like any other.
    pub fn replaying_with(mut self, replayer: Arc<dyn RequestReplayer>) -> Self {
        self.replayer = Some(replayer);
        self
    }

    /// The same service, also putting the proxy on the local network through
    /// `lan`, and taking it off again.
    ///
    /// Refused until this is given, for the same reason rules and replay
    /// are: a server with no listener to bind could accept `enable_lan` and
    /// do nothing, which is worse than saying so — and worse here than
    /// anywhere else, because what it would be lying about is whether other
    /// machines can reach the proxy.
    pub fn exposing_lan(mut self, lan: Arc<dyn LanControl>) -> Self {
        self.lan = Some(lan);
        self
    }

    /// The same service, also holding exchanges still through `breakpoints`.
    ///
    /// Refused until this is given, for the same reason the rest are: a
    /// server that accepted a breakpoint and had nothing to pause with would
    /// be a breakpoint that never fires. Watching is refused too, which is
    /// what keeps a client from believing it will be told about pauses that
    /// cannot happen.
    pub fn breaking_on(mut self, breakpoints: Arc<dyn BreakpointControl>) -> Self {
        self.breakpoints = Some(breakpoints);
        self
    }

    /// The same service, also shaping traffic through `throttle`.
    ///
    /// Refused until this is given, and for the sharpest version of the same
    /// reason: a server that accepted throttling settings and shaped nothing
    /// would be lying about how fast the proxy is.
    pub fn throttling_with(mut self, throttle: Arc<dyn ThrottleControl>) -> Self {
        self.throttle = Some(throttle);
        self
    }

    /// The same service, also managing the rules `engine` is serving.
    ///
    /// Every rule change is written to the database and then handed to the
    /// engine, in that order: a rule the proxy is applying is one that has
    /// been stored, and a restart brings back exactly what is being applied.
    pub fn serving_rules(mut self, engine: Arc<RuleEngine>) -> Self {
        self.engine = Some(engine);
        self
    }

    /// The same service, also answering `diagnostics` from `diagnostics`.
    ///
    /// Without one, the answer still says the core is running and storage is
    /// open — both of which are true of a server that got this far — and the
    /// listeners are unknown, because this crate did not bind them.
    pub fn reporting(mut self, diagnostics: Arc<dyn DiagnosticsControl>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// Starts receiving the exchanges finished from now on.
    pub fn subscribe(&self) -> EventSubscriber {
        self.publisher.subscribe()
    }

    /// Starts receiving captured stream-message summaries from now on.
    pub fn subscribe_streams(&self) -> StreamSubscriber {
        self.stream_publisher.subscribe()
    }

    /// Starts receiving the exchanges breakpoints hold still.
    ///
    /// `None` when this server is not serving breakpoints. Holding the
    /// stream is what makes the proxy pause at all, and dropping it is what
    /// releases whatever it was holding — see
    /// [`PauseStream`](crate::breakpoint::PauseStream).
    pub fn watch_breakpoints(&self) -> Option<PauseStream> {
        self.breakpoints.clone().map(PauseStream::attach)
    }

    /// Carries out one breakpoint operation.
    ///
    /// Not on a blocking thread and not near the database: breakpoints are
    /// held in memory by the running proxy, and none of these waits for
    /// anything. Nothing about a paused exchange is logged — what is being
    /// held still is a request with its headers and its body in it.
    pub async fn execute_breakpoints(
        &self,
        operation: BreakpointOperation,
    ) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let Some(breakpoints) = self.breakpoints.clone() else {
            proxy_diagnostics::warn(
                "ipc",
                name,
                "configuration",
                "the operation was refused: breakpoints are not being served",
            );

            return Err(ErrorBody::new(
                ErrorCode::Internal,
                "this server is not managing breakpoints",
            ));
        };

        Ok(match operation {
            BreakpointOperation::List => Outcome::ListBreakpoints(BreakpointList {
                breakpoints: breakpoints.list(),
            }),

            BreakpointOperation::Create(new) => {
                let breakpoint = breakpoints.create(new);
                println!("Breakpoint created: {}", breakpoint.summary());

                Outcome::CreateBreakpoint(BreakpointDetail { breakpoint })
            }

            BreakpointOperation::Update(id, update) => {
                let breakpoint = breakpoints.update(id, update).ok_or_else(|| {
                    ErrorBody::new(ErrorCode::NotFound, "there is no breakpoint with that id")
                })?;
                println!("Breakpoint updated: {}", breakpoint.summary());

                Outcome::UpdateBreakpoint(BreakpointDetail { breakpoint })
            }

            BreakpointOperation::Delete(id) => {
                let deleted = breakpoints.delete(id);
                if deleted {
                    println!("Breakpoint deleted: {id}");
                }

                Outcome::DeleteBreakpoint(BreakpointDeleted {
                    breakpoint_id: id,
                    deleted,
                })
            }

            BreakpointOperation::Continue(pause, decision) => {
                Outcome::ContinueBreakpoint(PauseResolved {
                    pause_id: pause,
                    resolved: breakpoints.resolve(pause, decision),
                })
            }

            BreakpointOperation::Cancel(pause) => Outcome::CancelBreakpoint(PauseResolved {
                pause_id: pause,
                resolved: breakpoints.resolve(pause, crate::breakpoint::PauseDecision::Cancel),
            }),
        })
    }

    /// Carries out one throttling operation.
    ///
    /// Changing it is reported, because it changes how every exchange from
    /// now on behaves and a proxy that is mysteriously slow should say so in
    /// its own log. The line names the numbers a developer typed and nothing
    /// any exchange carried.
    pub async fn execute_throttle(
        &self,
        operation: ThrottleOperation,
    ) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let Some(throttle) = self.throttle.clone() else {
            proxy_diagnostics::warn(
                "ipc",
                name,
                "configuration",
                "the operation was refused: throttling is not being served",
            );

            return Err(ErrorBody::new(
                ErrorCode::Internal,
                "this server is not shaping traffic",
            ));
        };

        Ok(match operation {
            ThrottleOperation::Status => Outcome::ThrottleStatus(throttle.settings()),
            ThrottleOperation::Set(settings) => {
                let settings = throttle.apply(settings);
                println!("Throttling is now {}", settings.summary());

                Outcome::SetThrottle(settings)
            }
        })
    }

    /// Carries out one rule operation.
    ///
    /// The database is synchronous, so this runs on a blocking thread for
    /// the same reason a traffic operation does. The engine is refreshed on
    /// this thread too, while nothing is holding the database: a request
    /// being evaluated at that moment finishes against the rules it started
    /// with.
    pub async fn execute_rules(&self, operation: RuleOperation) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let Some(engine) = self.engine.clone() else {
            // Not a failure of the request's: this build of the server was
            // started without a rule engine to keep in step.
            proxy_diagnostics::warn(
                "ipc",
                name,
                "configuration",
                "the operation was refused: rules are not being served",
            );

            return Err(ErrorBody::new(
                ErrorCode::Internal,
                "this server is not managing rules",
            ));
        };
        let storage = self.storage.clone();

        let result = tokio::task::spawn_blocking(move || run_rules(&storage, &engine, operation))
            .await
            .map_err(|error| abandoned(name, error))?;

        result.map_err(|failure| failure.reported(name))
    }

    /// Carries out one cURL or replay operation.
    ///
    /// Parsing needs nothing but the text, and happens here. Writing a
    /// stored request out as cURL needs the database, so it goes to a
    /// blocking thread like every other database call. Replaying needs the
    /// proxy, and is awaited: the answer is what the destination said.
    pub async fn execute_developer(
        &self,
        operation: DeveloperOperation,
    ) -> Result<Outcome, ErrorBody> {
        let name = operation.name();

        match operation {
            DeveloperOperation::ParseCurl(command) => Ok(Outcome::ParseCurl(parse_curl(&command))),
            DeveloperOperation::CurlFromTraffic(id) => self.curl_from_traffic(id, name).await,
            DeveloperOperation::Replay(request) => self.replay(request, name).await,
        }
    }

    /// Writes one stored request out as the command that would send it again.
    async fn curl_from_traffic(
        &self,
        id: proxy_storage::ExchangeId,
        name: &'static str,
    ) -> Result<Outcome, ErrorBody> {
        let storage = self.storage.clone();

        let result = tokio::task::spawn_blocking(move || {
            let exchange = storage.get(id)?.ok_or(Failure::NotFound)?;
            let written = proxy_curl::from_captured(&exchange.request);

            Ok::<_, Failure>(Outcome::CurlFromTraffic(CurlGenerated {
                curl: written.command,
                truncated: written.truncated,
            }))
        })
        .await
        .map_err(|error| abandoned(name, error))?;

        result.map_err(|failure| failure.reported(name))
    }

    /// Sends one request through the proxy.
    ///
    /// Not on a blocking thread: this is a network request, and the whole of
    /// what it does is await one. Nothing is logged about it here — the
    /// proxy reports the exchange it carried, the way it reports every other
    /// one, and a request being replayed is not a second thing to write down.
    async fn replay(
        &self,
        request: proxy_http::RequestSpec,
        name: &'static str,
    ) -> Result<Outcome, ErrorBody> {
        let Some(replayer) = self.replayer.clone() else {
            proxy_diagnostics::warn(
                "ipc",
                name,
                "configuration",
                "the operation was refused: replay is not being served",
            );

            return Err(ErrorBody::new(
                ErrorCode::Internal,
                "this server is not sending requests",
            ));
        };

        Ok(Outcome::ReplayRequest(replayer.replay(request).await))
    }

    /// Carries out one LAN operation.
    ///
    /// Not on a blocking thread, and not near the database: binding and
    /// closing a listening socket is what these do, and both are awaited.
    ///
    /// Turning it on and off is reported, because it is the one operation
    /// that changes who can reach ProxyLab. The line names the port and
    /// whether the proxy is on the local network; it does not name the
    /// address, which is this developer's own network position and is
    /// already on screen for whoever asked.
    pub async fn execute_lan(&self, operation: LanOperation) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let Some(lan) = self.lan.clone() else {
            proxy_diagnostics::warn(
                "ipc",
                name,
                "configuration",
                "the operation was refused: LAN exposure is not being served",
            );

            return Err(ErrorBody::new(
                ErrorCode::Internal,
                "this server is not managing local network exposure",
            ));
        };

        match operation {
            LanOperation::Status => Ok(Outcome::LanStatus(lan.status().await)),
            LanOperation::Enable(address) => {
                let status = lan.enable(address).await.map_err(|refusal| {
                    let error = refusal.error();
                    proxy_diagnostics::warn("lan", "enable", "bind", &error.message);

                    error
                })?;
                proxy_diagnostics::info(
                    "lan",
                    "enable",
                    "lifecycle",
                    &format!(
                        "the proxy is now reachable from the local network on port {}",
                        status.port
                    ),
                );

                Ok(Outcome::EnableLan(status))
            }
            LanOperation::Disable => {
                let status = lan.disable().await;
                proxy_diagnostics::info(
                    "lan",
                    "disable",
                    "lifecycle",
                    "the proxy is no longer reachable from the local network",
                );

                Ok(Outcome::DisableLan(status))
            }
        }
    }

    /// Where the process stands, and the recent problems it has kept.
    ///
    /// Cheap on purpose. LAN exposure is read from the control the LAN
    /// operations already use, the listener facts are the ones recorded when
    /// they were bound, and the recent errors are an in-memory buffer. No
    /// database is opened and no traffic is read.
    pub async fn execute_diagnostics(&self) -> Result<Outcome, ErrorBody> {
        let facts = match &self.diagnostics {
            Some(diagnostics) => diagnostics.facts(),
            None => diagnostics::DiagnosticsFacts::serving(env!("CARGO_PKG_VERSION")),
        };
        let lan = match &self.lan {
            Some(lan) => Some(lan.status().await),
            None => None,
        };

        Ok(Outcome::Diagnostics(diagnostics::report(facts, lan)))
    }

    /// Carries out one saved-request operation.
    ///
    /// The database is synchronous, so this runs on a blocking thread for
    /// the same reason a traffic operation does. Nothing is logged about any
    /// of them: a saved request is exactly where a bearer token lives, and
    /// even its name is text a person typed. That is the discipline the cURL
    /// and replay operations already follow.
    pub async fn execute_saved(
        &self,
        operation: SavedRequestOperation,
    ) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let storage = self.storage.clone();

        let result = tokio::task::spawn_blocking(move || run_saved(&storage, operation))
            .await
            .map_err(|error| abandoned(name, error))?;

        result.map_err(|failure| failure.reported(name))
    }

    /// Carries out one HAR operation.
    ///
    /// On a blocking thread, and for two reasons rather than one: the
    /// database is synchronous, and so is writing or reading a file that may
    /// be tens of megabytes. Neither belongs on a runtime thread the proxy's
    /// connections are running on.
    ///
    /// What is reported is the count, not the traffic: an export names how
    /// many exchanges it wrote, and an import how many it read. No URL, no
    /// header, and no path — the path came from the client, and this server
    /// does not repeat back what a client sent.
    pub async fn execute_har(&self, operation: HarOperation) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let storage = self.storage.clone();

        let result = tokio::task::spawn_blocking(move || run_har(&storage, operation))
            .await
            .map_err(|error| abandoned(name, error))?;

        result.map_err(|failure| failure.reported(name))
    }

    /// Carries out one traffic operation.
    pub async fn execute(&self, operation: TrafficOperation) -> Result<Outcome, ErrorBody> {
        let name = operation.name();
        let storage = self.storage.clone();

        // The database is synchronous, so it runs on a blocking thread. A
        // panic in there is a bug in this process and not a reason to take
        // the server down with it, so a join failure is answered rather than
        // propagated.
        let result = tokio::task::spawn_blocking(move || run(&storage, operation))
            .await
            .map_err(|error| abandoned(name, error))?;

        result.map_err(|failure| failure.reported(name))
    }
}

/// Why an operation produced no outcome.
enum Failure {
    /// The database would not do it.
    Storage(StorageError),
    /// There is no such exchange.
    NotFound,
    /// There is no such stream message.
    StreamNotFound,
    /// There is no such rule.
    RuleNotFound,
    /// The rule is not one this server can store or act on.
    InvalidRule(&'static str),
    /// There is no such saved request.
    SavedRequestNotFound,
    /// The saved request is not one this server can keep or send.
    InvalidSavedRequest(&'static str),
    /// The HAR file could not be written or read.
    Har(proxy_har::HarError),
}

impl Failure {
    /// What the client is told, having reported what it is not told.
    fn reported(self, operation: &'static str) -> ErrorBody {
        match self {
            Self::Storage(error) => {
                if matches!(error, StorageError::InvalidSearch) {
                    return ErrorBody::new(
                        ErrorCode::InvalidParameters,
                        "the search is not a regular expression",
                    );
                }
                // The operating-system error is worth having in the diagnostic
                // log. The database path is not: a report is pasted into
                // issues, and the path is this user's home directory. The
                // client is told a fixed phrase either way.
                proxy_diagnostics::error(
                    "storage",
                    operation,
                    "database",
                    &format!("the {operation} operation failed: {error}"),
                );

                ErrorBody::new(
                    ErrorCode::StorageUnavailable,
                    "the stored traffic could not be read",
                )
            }
            // Not a failure of the server's, so nothing is logged: asking for
            // an exchange that has been deleted is a normal thing for a
            // client to do.
            Self::NotFound => ErrorBody::new(
                ErrorCode::NotFound,
                "there is no stored exchange with that id",
            ),
            Self::StreamNotFound => ErrorBody::new(
                ErrorCode::NotFound,
                "there is no stored stream message with that id",
            ),
            Self::RuleNotFound => {
                ErrorBody::new(ErrorCode::NotFound, "there is no rule with that id")
            }
            // The reason is one of the rule model's own fixed phrases, which
            // name nothing the client sent.
            Self::InvalidRule(reason) => ErrorBody::new(ErrorCode::InvalidRule, reason),
            Self::SavedRequestNotFound => ErrorBody::new(
                ErrorCode::NotFound,
                "there is no saved request with that id",
            ),
            // A fixed phrase again: either the saved request model's or
            // `proxy-http`'s, both of which name nothing the client sent.
            Self::InvalidSavedRequest(reason) => {
                ErrorBody::new(ErrorCode::InvalidSavedRequest, reason)
            }
            Self::Har(error) => {
                // What the file system said is worth having. The path the
                // client chose is not sent back and is not kept in the
                // diagnostic log: it is this user's own file.
                proxy_diagnostics::error(
                    "har",
                    operation,
                    "file",
                    &format!("the {operation} operation failed: {error}"),
                );

                ErrorBody::new(
                    ErrorCode::HarUnavailable,
                    "the HAR file could not be written or read",
                )
            }
        }
    }
}

impl From<proxy_har::HarError> for Failure {
    fn from(error: proxy_har::HarError) -> Self {
        Self::Har(error)
    }
}

impl From<SavedRequestError> for Failure {
    fn from(error: SavedRequestError) -> Self {
        Self::InvalidSavedRequest(error.reason())
    }
}

impl From<StorageError> for Failure {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

/// Reads one cURL command.
///
/// A command that does not parse is a result and not a failure: see
/// [`CurlParsed`]. Nothing about the command is logged — a cURL command is
/// exactly where a bearer token lives.
fn parse_curl(command: &str) -> CurlParsed {
    match proxy_curl::parse(command) {
        Ok(import) => CurlParsed {
            request: Some(import.request),
            notes: import.notes,
            problem: None,
        },
        Err(error) => CurlParsed {
            request: None,
            notes: Vec::new(),
            problem: Some(CurlProblem {
                code: error.code(),
                message: error.message(),
                option: error.option().map(str::to_owned),
            }),
        },
    }
}

/// One traffic operation, on the thread that may block.
fn run(storage: &Storage, operation: TrafficOperation) -> Result<Outcome, Failure> {
    match operation {
        TrafficOperation::List(request) => list(storage, request.query),
        TrafficOperation::Get(id) => {
            let exchange = storage.get(id)?.ok_or(Failure::NotFound)?;

            Ok(Outcome::GetTraffic(TrafficDetail { exchange }))
        }
        TrafficOperation::Delete(id) => Ok(Outcome::DeleteTraffic(Deleted {
            exchange_id: id,
            deleted: storage.delete(id)?,
        })),
        TrafficOperation::Clear => Ok(Outcome::ClearTraffic(Cleared {
            removed: storage.clear()?,
        })),
        TrafficOperation::ListStream(request) => {
            let page =
                storage.list_stream_messages(request.exchange_id, request.after, request.limit)?;

            Ok(Outcome::ListStreamMessages(page))
        }
        TrafficOperation::GetStream(id) => {
            let message = storage
                .get_stream_message(id)?
                .ok_or(Failure::StreamNotFound)?;

            Ok(Outcome::GetStreamMessage(StreamMessageDetail { message }))
        }
    }
}

/// One rule operation, on the thread that may block.
///
/// Every operation that changes anything ends the same way: the database is
/// written, and then the engine is given the whole rule set again. Rebuilding
/// the set rather than patching it is deliberate — the set the proxy applies
/// is then always exactly what the database holds, including the ordering,
/// and there is no second code path that could put them out of step.
fn run_rules(
    storage: &Storage,
    engine: &RuleEngine,
    operation: RuleOperation,
) -> Result<Outcome, Failure> {
    match operation {
        RuleOperation::List => Ok(Outcome::ListRules(RuleList {
            rules: storage.list_rules()?,
        })),

        RuleOperation::Get(id) => {
            let rule = storage.rule(id)?.ok_or(Failure::RuleNotFound)?;

            Ok(Outcome::GetRule(RuleDetail { rule }))
        }

        RuleOperation::Create(new) => {
            let rule = Rule::create(new).map_err(|error| Failure::InvalidRule(error.reason()))?;
            storage.insert_rule(&rule)?;
            refresh(storage, engine)?;

            println!("Rule created: {}", rule.summary());

            Ok(Outcome::CreateRule(RuleDetail { rule }))
        }

        RuleOperation::Update(id, update) => {
            let rule = storage
                .update_rule(id, update)?
                .ok_or(Failure::RuleNotFound)?;
            refresh(storage, engine)?;

            println!("Rule updated: {}", rule.summary());

            Ok(Outcome::UpdateRule(RuleDetail { rule }))
        }

        RuleOperation::Delete(id) => {
            let deleted = storage.delete_rule(id)?;
            if deleted {
                refresh(storage, engine)?;
                println!("Rule deleted: {id}");
            }

            Ok(Outcome::DeleteRule(RuleDeleted {
                rule_id: id,
                deleted,
            }))
        }
    }
}

/// Gives the engine the rules the database now holds.
///
/// Called after every change, and never while a request is being evaluated:
/// replacing the snapshot is one pointer write, and a request already
/// evaluating finishes against the set it took.
fn refresh(storage: &Storage, engine: &RuleEngine) -> Result<(), Failure> {
    engine.replace(RuleSet::new(storage.list_rules()?));

    Ok(())
}

/// Loads the rules a database holds into a set ready to serve.
///
/// What a proxy calls at startup, so the rules it applies from its first
/// request are the ones it was left with.
pub fn load_rules(storage: &Storage) -> Result<RuleSet, StorageError> {
    Ok(RuleSet::new(storage.list_rules()?))
}

/// One saved-request operation, on the thread that may block.
///
/// Every request that is stored has already been turned into a sendable one
/// and back: [`checked`] builds a `RequestSpec` from it and keeps the URL
/// that parse normalized. So a saved request in the database is one the
/// replay endpoint will accept, and a URL this proxy cannot speak to is
/// refused when it is saved rather than when it is sent.
fn run_saved(storage: &Storage, operation: SavedRequestOperation) -> Result<Outcome, Failure> {
    match operation {
        SavedRequestOperation::List => Ok(Outcome::ListSavedRequests(SavedRequestList {
            saved_requests: storage.list_saved_requests()?,
        })),

        SavedRequestOperation::Get(id) => {
            let saved_request = storage
                .saved_request(id)?
                .ok_or(Failure::SavedRequestNotFound)?;

            Ok(Outcome::GetSavedRequest(SavedRequestDetail {
                saved_request,
            }))
        }

        SavedRequestOperation::Create(new) => {
            let saved_request = checked(SavedRequest::create(new)?)?;
            storage.insert_saved_request(&saved_request)?;

            Ok(Outcome::CreateSavedRequest(SavedRequestDetail {
                saved_request,
            }))
        }

        SavedRequestOperation::CreateFromTraffic(exchange, name) => {
            // Read out of the store rather than sent by the client, for the
            // reason `curl_from_traffic` is: the core kept the request, and
            // is what knows what it was. The captured body comes with its
            // truncation, so a request saved from an upload the capture limit
            // cut short says it sends less than the original did.
            let exchange = storage.get(exchange)?.ok_or(Failure::NotFound)?;
            let mut saved_request = checked(SavedRequest::create(NewSavedRequest {
                name,
                method: exchange.request.method.clone(),
                url: exchange.request.url.clone(),
                headers: exchange.request.headers.clone(),
                body: Vec::new(),
            })?)?;
            saved_request.body = exchange.request.body.clone();
            storage.insert_saved_request(&saved_request)?;

            Ok(Outcome::SaveRequestFromTraffic(SavedRequestDetail {
                saved_request,
            }))
        }

        SavedRequestOperation::Update(id, mut update) => {
            // Checked before it is written, so the stored request stays one
            // the replay endpoint will accept whatever an edit did to it —
            // and the normalized method and URL are written back into the
            // update, so what is stored is what will be sent. An edit that
            // named neither leaves both as they were.
            let current = storage
                .saved_request(id)?
                .ok_or(Failure::SavedRequestNotFound)?;
            let checked = checked(current.updated(update.clone())?)?;
            if update.method.is_some() || update.url.is_some() {
                update.method = Some(checked.method);
                update.url = Some(checked.url);
            }

            let saved_request = storage
                .update_saved_request(id, update)?
                .ok_or(Failure::SavedRequestNotFound)?;

            Ok(Outcome::UpdateSavedRequest(SavedRequestDetail {
                saved_request,
            }))
        }

        SavedRequestOperation::Delete(id) => Ok(Outcome::DeleteSavedRequest(SavedRequestDeleted {
            saved_request_id: id,
            deleted: storage.delete_saved_request(id)?,
        })),
    }
}

/// The same saved request, with its URL as `proxy-http` normalizes one.
///
/// The check that a saved request is a request this proxy can send, made
/// where it is saved rather than where it is replayed. It is the same
/// `RequestSpec::new` a replay goes through, so the two cannot disagree about
/// what a URL may be — and the normalized URL is kept, so the URL that is
/// stored is the URL that will be sent.
fn checked(mut saved: SavedRequest) -> Result<SavedRequest, Failure> {
    let request = proxy_http::RequestSpec::from_saved(&saved)
        .map_err(|error| Failure::InvalidSavedRequest(error.reason()))?;

    saved.method = request.method().to_owned();
    saved.url = request.url().to_owned();

    Ok(saved)
}

/// One HAR operation, on the thread that may block.
fn run_har(storage: &Storage, operation: HarOperation) -> Result<Outcome, Failure> {
    match operation {
        HarOperation::Export(export) => export_har(storage, export),
        HarOperation::Import(path) => import_har(storage, &path),
    }
}

/// Writes the exchanges an export names to the file it names.
///
/// One exchange at a time, straight into the file: exporting a whole session
/// is thousands of exchanges with their bodies, and holding that in memory
/// would make the size of an export the size of this process. A filtered
/// export therefore pages through the store exactly as a traffic list does,
/// with the same cursor and the same order.
fn export_har(storage: &Storage, export: HarExport) -> Result<Outcome, Failure> {
    let mut writer = proxy_har::HarWriter::create(&export.path)?;
    let mut truncated = false;

    match export.selection {
        HarSelection::Exchanges(ids) => {
            for id in ids {
                // An exchange that has been deleted since it was selected is
                // left out rather than failing the export: the person asked
                // for what they can see, and losing the whole file over one
                // row that went away would be the wrong answer.
                if let Some(exchange) = storage.get(id)? {
                    writer.write(&exchange)?;
                }
            }
        }

        HarSelection::Filtered(filter) => {
            // Oldest first, so the entries are in the order the traffic
            // happened — which is the order every other HAR file is in, and
            // the order a waterfall is drawn from.
            let mut query = TrafficQuery::oldest()
                .with_limit(proxy_storage::MAX_PAGE_LIMIT)
                .with_filter(filter);

            loop {
                let page = storage.list(&query)?;
                let next = page.next;

                for summary in page.exchanges {
                    if writer.entries() as usize >= MAX_EXPORT_ENTRIES {
                        truncated = true;
                        break;
                    }
                    if let Some(exchange) = storage.get(summary.id)? {
                        writer.write(&exchange)?;
                    }
                }

                match next {
                    Some(cursor) if !truncated => query = query.after(cursor),
                    _ => break,
                }
            }
        }
    }

    let summary = writer.finish()?;
    println!(
        "Exported {} exchanges as HAR ({} bytes)",
        summary.entries, summary.bytes
    );

    Ok(Outcome::ExportHar(HarExported {
        entries: summary.entries,
        bytes: summary.bytes,
        truncated,
    }))
}

/// Reads a HAR file, and keeps what it describes as captured traffic.
///
/// Nothing is sent. What comes back from `proxy-har` is
/// [`TrafficExchange`]es, and what happens to them is what happens to an
/// exchange the proxy captured: they are stored, and the traffic list shows
/// them. They are not published on the event stream — that stream carries the
/// exchanges the proxy *finished*, and an imported one is not one — so a
/// client reloads its list after an import, the way it reloads its rules
/// after changing one.
fn import_har(storage: &Storage, path: &str) -> Result<Outcome, Failure> {
    let import = proxy_har::read(path, &proxy_har::ImportLimits::default())?;
    let mut imported = 0;
    let mut skipped = import.skipped;

    for exchange in &import.exchanges {
        match storage.insert(exchange) {
            Ok(()) => imported += 1,
            Err(error) => {
                // One row that would not go in is not the whole import: the
                // rest of the file is still worth having, and the count says
                // what did not arrive. The error names the database and no
                // traffic — see `proxy-storage`.
                proxy_diagnostics::warn(
                    "storage",
                    "import_har",
                    "database",
                    &format!("one imported exchange was not stored: {error}"),
                );
                skipped += 1;
            }
        }
    }

    proxy_diagnostics::info(
        "har",
        "import_har",
        "lifecycle",
        &format!("imported {imported} exchanges from a HAR file"),
    );

    Ok(Outcome::ImportHar(HarImported { imported, skipped }))
}

/// A blocking task that did not return an answer.
///
/// The panic payload is not logged. A panic can carry whatever a test or a
/// bug formatted, and that is not a diagnostic.
fn abandoned(operation: &'static str, error: tokio::task::JoinError) -> ErrorBody {
    let message = if error.is_panic() {
        "the operation panicked"
    } else {
        "the operation was cancelled"
    };
    proxy_diagnostics::error("ipc", operation, "internal", message);

    ErrorBody::new(
        ErrorCode::Internal,
        "the server could not finish this operation",
    )
}

/// A page of the traffic list, and how many there are in total.
///
/// Both come from the same call so a client is never shown a page from one
/// moment and a count from another.
fn list(storage: &Storage, query: TrafficQuery) -> Result<Outcome, Failure> {
    let total = storage.count_matching(&query.filter)?;
    let TrafficPage { exchanges, next } = storage.list(&query)?;

    Ok(Outcome::ListTraffic(TrafficList {
        exchanges,
        next_cursor: next,
        total,
    }))
}
