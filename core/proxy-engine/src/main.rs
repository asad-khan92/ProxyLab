//! ProxyLab proxy engine binary.
//!
//! Loads (or, the first time, creates) the local development certificate
//! authority, binds the proxy listener, reports where it is listening and
//! where the CA certificate can be found, and runs the accept loop until
//! Ctrl+C.
//!
//! Traffic capture runs with it, publishing a finished exchange per request
//! and storing it. Nothing here subscribes to the event stream — that is for
//! the UI that comes later — but persistence is wired up, so traffic outlives
//! the run that captured it.
//!
//! The IPC server runs alongside it, on a Unix socket, and is how the macOS
//! UI reaches any of this. Nothing subscribes to the traffic event stream
//! here; the UI does, across that socket.
//!
//! The UI can also ask for a request to be sent. That goes through the same
//! capture, the same interceptor, and the same rule engine this listener is
//! using — see `proxy_engine::replay` — so a replayed request is traffic like
//! any other, and there is no second HTTP client in the process.
//!
//! Rules are loaded from the database once, into an engine both halves
//! share: the listener consults it before carrying a request, and the IPC
//! server replaces what it holds whenever a rule is created, changed, or
//! removed. So a rule takes effect on the next request without the proxy
//! asking the database anything, and the rules a restart comes back with are
//! the ones that were being applied.
//!
//! Storage and IPC fail differently on purpose.
//!
//! A database that cannot be opened is reported and the proxy runs without
//! one: carrying requests is what a proxy is for, and keeping a record of
//! them is what it does afterwards. A socket that cannot be bound stops the
//! process, because it is the only way anything reaches ProxyLab from
//! outside — a core running with no socket is a core the developer cannot
//! see, control, or stop except by finding the process. The usual reason to
//! fail is also the most telling one: another ProxyLab already has it, and
//! starting a second is not what was wanted.
//!
//! The CA is only ever generated once. Nothing here installs it or changes
//! any system trust setting: making a client trust it is a deliberate act by
//! the developer, which is why the path to it is printed.
//!
//! Breakpoints and throttling run with it, and nothing is configured here.
//! Both are held in memory by the running proxy and neither is read from or
//! written to disk: a ProxyLab that has just started pauses nothing and
//! carries traffic at the speed of the network it is on. They are shared with
//! the LAN listener, so a breakpoint or a bandwidth ceiling applies to a
//! phone's traffic exactly as it applies to a browser's — see
//! `proxy_engine::controls`.
//!
//! Nothing pauses unless the UI has asked to be told about pauses. That is
//! deliberate and is the whole of the disconnect story: a request held still
//! cannot outlive the connections that could have freed it.
//!
//! The proxy listens on loopback and on nothing else. A second listener, on
//! one address of this Mac's local network, can be turned on from the UI so
//! that a physical iPhone on the same Wi-Fi can be pointed at it — see
//! `proxy_engine::lan`. It is built here holding everything such a listener
//! would need and listening on nothing, and a run that was never asked is a
//! run only this Mac could reach. Nothing here reads a setting that would
//! turn it on, and nothing writes one.
//!
//! A third listener, also on loopback, is where macOS's transparent proxy
//! provider hands over the flows it captures — see `proxy_engine::transparent`.
//! It is started with the proxy, because it opens nothing to anything that
//! the explicit listener beside it does not already: until the macOS app
//! turns the Network Extension on, nothing connects to it. A port that cannot
//! be bound is reported and the proxy runs without it, since the explicit
//! proxy is the whole of what most runs use. It shares everything the
//! explicit listener has — interception, upstream pool, capture, rules and
//! controls — so a captured flow is traffic like any other.

use std::process::ExitCode;
use std::sync::Arc;

use proxy_engine::capture::Capture;
use proxy_engine::controls::Controls;
use proxy_engine::lan::{self, discovery, LanExposure};
use proxy_engine::listener::{Listener, DEFAULT_LISTEN_ADDR};
use proxy_engine::replay::Replayer;
use proxy_engine::status;
use proxy_engine::transparent::{Own, DEFAULT_TRANSPARENT_ADDR};
use proxy_engine::upstream::UpstreamManager;
use proxy_ipc::{load_rules, IpcConfig, IpcServer, TrafficService};
use proxy_storage::{Storage, StorageConfig, StorageWorker};
use proxy_tls::{CertificateAuthority, Interceptor};
use rules::RuleEngine;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> ExitCode {
    let ca_directory = match CertificateAuthority::default_directory() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("Failed to work out where to keep the development CA: {error}");
            return ExitCode::FAILURE;
        }
    };

    let interceptor = match Interceptor::with_ca_in(&ca_directory) {
        Ok(interceptor) => Arc::new(interceptor),
        Err(error) => {
            eprintln!("Failed to prepare the development CA: {error}");
            return ExitCode::FAILURE;
        }
    };

    // The connections the proxy holds open to origin servers, shared by the
    // listener and by replay: one pool for the whole process, so a connection
    // one request opened answers the next request to the same origin.
    let upstream = Arc::new(UpstreamManager::new(Arc::clone(&interceptor)));

    // The breakpoints and the throttling, shared by both listeners and by the
    // IPC server. Nothing is configured here and nothing is read from disk: a
    // ProxyLab that has just started stops nothing and shapes nothing, and it
    // stays that way until the UI asks otherwise.
    let controls = Controls::new();

    let listener = match Listener::bind(DEFAULT_LISTEN_ADDR).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Failed to listen on {DEFAULT_LISTEN_ADDR}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let listen_addr = listener.local_addr();
    println!("{}", status::startup_report(listen_addr, &ca_directory));

    // Where the transparent proxy provider hands over the flows it captures.
    // Not fatal: the explicit proxy runs without it, and says so.
    let transparent = match Listener::bind(DEFAULT_TRANSPARENT_ADDR).await {
        Ok(bridge) => Some(bridge),
        Err(error) => {
            eprintln!(
                "Failed to listen for transparent capture on {DEFAULT_TRANSPARENT_ADDR}: {error}"
            );
            None
        }
    };
    println!(
        "{}",
        status::transparent_report(match &transparent {
            Some(bridge) => Ok(bridge.local_addr()),
            None => Err("see the message above"),
        })
    );

    // Capture runs whether or not anything is listening to it: exchanges are
    // published to whoever has subscribed, and dropped when nobody has. The
    // UI that subscribes is a later phase.
    let capture = Capture::with_defaults();
    let publisher = capture.publisher().clone();
    let stream_publisher = capture.stream_publisher().clone();
    let storage = open_storage();
    println!(
        "{}",
        status::traffic_report(match &storage {
            Some(storage) => Ok(storage.config().database.as_path()),
            None => Err("see the message above"),
        })
    );

    // The worker owns the database from here: it drains a bounded queue on a
    // blocking thread, so nothing on a connection ever waits for SQLite.
    let worker = storage.as_ref().cloned().map(StorageWorker::spawn);
    let capture = match &worker {
        Some(worker) => {
            let handle = worker.handle();
            capture
                .storing_into(Arc::new(handle.clone()))
                .storing_streams(Arc::new(handle))
        }
        None => capture,
    };

    // The rules the proxy applies, loaded once and then held in memory: a
    // request is evaluated against a snapshot of them and never waits for the
    // database. With no database there are no rules to load and no way to
    // keep any, so the engine is absent and every request is allowed.
    let engine = storage.as_ref().map(|storage| {
        let rules = match load_rules(storage) {
            Ok(rules) => rules,
            Err(error) => {
                // A rule that cannot be read is not a reason to refuse to
                // proxy. It is a reason to say so loudly and carry traffic
                // under no rules at all, which is the safe direction: a
                // half-loaded rule set could block or reroute traffic in a
                // way nobody configured.
                eprintln!("Failed to load rules; none will be applied: {error}");
                rules::RuleSet::empty()
            }
        };

        Arc::new(RuleEngine::with(rules))
    });
    println!("{}", status::rules_report(engine.as_deref()));

    // The UI reads the stored traffic, watches the capture's own event
    // stream, and manages the rules, so it is given a handle to each rather
    // than a second copy of any. With no database there is nothing to serve,
    // so there is no socket to connect to either.
    // The same capture, the same interceptor, and the same rule engine the
    // listener is using: a replayed request is evaluated, sent, captured,
    // stored, and published by the pipeline the proxy is already running,
    // and not by a second one built beside it.
    let replayer = Arc::new(Replayer::new(
        capture.clone(),
        Arc::clone(&upstream),
        engine.clone(),
    ));

    // One signal stops everything: the proxy listener, the IPC server, and
    // the LAN listener if there is one each wait on it, so Ctrl+C ends the
    // process rather than part of it.
    let (stopping, _) = watch::channel(false);
    let proxy_signal = stopping.subscribe();
    let ipc_signal = stopping.subscribe();

    // Built with everything a second listener would need, and listening on
    // nothing. It stays that way until the UI asks for an address — see
    // `proxy_engine::lan`. The port is the one the proxy is already on, so a
    // phone and a browser on this Mac are told the same number.
    let lan = Arc::new(LanExposure::new(
        listen_addr.port(),
        Arc::clone(&interceptor),
        Arc::clone(&upstream),
        capture.clone(),
        engine.clone(),
        controls.clone(),
        stopping.subscribe(),
    ));
    println!("{}", lan::exposure::startup_report(&discovery::usable()));

    let ipc = match (&storage, &engine) {
        (Some(storage), Some(engine)) => match bind_ipc() {
            Ok(server) => Some((
                server,
                TrafficService::new(storage.clone(), publisher)
                    .publishing_streams(stream_publisher)
                    .serving_rules(Arc::clone(engine))
                    .replaying_with(replayer)
                    .exposing_lan(Arc::clone(&lan) as Arc<dyn proxy_ipc::LanControl>)
                    .breaking_on(
                        Arc::clone(controls.breakpoints()) as Arc<dyn proxy_ipc::BreakpointControl>
                    )
                    .throttling_with(
                        Arc::clone(controls.throttle()) as Arc<dyn proxy_ipc::ThrottleControl>
                    ),
            )),
            Err(error) => {
                eprintln!("Failed to start the IPC server: {error}");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    println!(
        "{}",
        status::ipc_report(match &ipc {
            Some((server, _)) => Ok(server.socket_path()),
            None => Err("there is no traffic database to serve"),
        })
    );

    // Started before the stop signal is wired to Ctrl+C, so it hears the same
    // signal as everything else. It shares the explicit listener's upstream
    // pool rather than owning it: the explicit listener is the one that
    // closes it, once, when the proxy stops.
    let bridging = transparent.map(|bridge| {
        let own = Own::this_process(&[listen_addr, bridge.local_addr()]);
        let bridge = bridge.sharing_upstream().transparent(own);
        let signal = stopping.subscribe();
        let running = bridge.run(
            Arc::clone(&interceptor),
            Arc::clone(&upstream),
            capture.clone(),
            engine.clone(),
            controls.clone(),
            stopped(signal),
        );

        tokio::spawn(running)
    });

    tokio::spawn(async move {
        ctrl_c().await;
        let _ = stopping.send(true);
    });

    let proxying = listener.run(
        interceptor,
        upstream,
        capture,
        engine.clone(),
        controls,
        stopped(proxy_signal),
    );
    let outcome = match ipc {
        Some((server, service)) => {
            let (proxied, served) =
                tokio::join!(proxying, server.run(service, stopped(ipc_signal)));
            if let Err(error) = served {
                eprintln!("IPC server stopped: {error}");
            }

            proxied
        }
        None => proxying.await,
    };

    // The bridge waits on the same signal and is winding down too. An error
    // from it is its own: the explicit proxy has already stopped cleanly or
    // not on its own account.
    if let Some(bridging) = bridging {
        match bridging.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("Transparent capture stopped: {error}"),
            Err(error) => eprintln!("Transparent capture stopped: {error}"),
        }
    }

    // The LAN listener is already winding down — it waits on the same signal
    // — and this is where the process waits for it. Nothing is said about
    // it: a listener going away with the proxy is not news.
    lan.shutdown().await;

    // After both listeners, so that everything the last connections published
    // is stored before the process ends.
    if let Some(worker) = worker {
        worker.shutdown().await;
    }

    if let Err(error) = outcome {
        eprintln!("Proxy engine stopped: {error}");
        return ExitCode::FAILURE;
    }

    println!("Shutting down.");
    ExitCode::SUCCESS
}

/// Binds the IPC socket, or says why it could not be.
fn bind_ipc() -> Result<IpcServer, proxy_ipc::IpcError> {
    IpcServer::bind(&IpcConfig::default_location()?)
}

/// Resolves once the stop signal has been given.
async fn stopped(mut signal: watch::Receiver<bool>) {
    while !*signal.borrow() {
        // A sender that is gone is a process that is ending, which is a
        // reason to stop rather than to keep waiting.
        if signal.changed().await.is_err() {
            return;
        }
    }
}

/// Opens the traffic database, or explains why there will not be one.
///
/// A failure here is reported and returns `None`: the proxy then runs without
/// persistence rather than not running. Storage is where a debugging proxy
/// keeps what it saw, and that is not a reason to refuse to carry traffic.
fn open_storage() -> Option<Storage> {
    let config = match StorageConfig::default_location() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Failed to work out where to keep captured traffic: {error}");
            return None;
        }
    };

    match Storage::open(config) {
        Ok(storage) => Some(storage),
        Err(error) => {
            eprintln!("Failed to open the traffic database: {error}");
            None
        }
    }
}

/// Resolves when Ctrl+C is received.
///
/// If the signal handler cannot be installed there is no way to observe
/// Ctrl+C, so the engine reports that and shuts down rather than running on
/// without a way to stop it.
async fn ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("Failed to wait for Ctrl+C: {error}");
    }
}
