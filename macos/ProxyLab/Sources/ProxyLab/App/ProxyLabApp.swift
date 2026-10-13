//  ProxyLab's macOS application.
//
//      ProxyLab (SwiftUI)
//            │
//            │  Unix domain socket, one JSON object per line
//            ▼
//      proxy-ipc  ──▶  the Rust core
//
//  The whole of what this process does is draw what comes over that socket,
//  and own the lifecycle of the proxy-engine executable bundled in
//  ProxyLab.app. It holds no proxy logic: it does not listen on a port,
//  terminate TLS, parse HTTP, or keep a capture store. Launching the app
//  starts that bundled engine, waits until it is ready, trusts the CA that
//  engine is using, points this Mac's HTTP and HTTPS proxy at 127.0.0.1:9090,
//  and turns transparent capture on.
//  Stop and quit put that proxy back before stopping the process.
//
//  The socket is resolved once, here, by `IPCSocketLocation` — the same rules
//  the core uses to decide where to listen.
//
//  Two things in the window do not come over that socket: the iOS Simulator
//  section runs `xcrun simctl` and reads this Mac's proxy configuration, and
//  the Android Emulator section runs `adb`. The simulator has no proxy of its
//  own. This Mac's HTTP and HTTPS proxy is set and restored by the capture
//  lifecycle, not by connecting a simulator. Both device sections are told
//  whether the core is running rather than looking for it a second way.
//
//  Breakpoints come over the same socket again. Asking the core to send this
//  app the exchanges it holds still is also what makes it hold any: a core
//  nobody is watching pauses nothing, so this app crashing or being quit
//  cannot leave a request stranded.
//
//  The iPhone section does come over the socket. Putting the proxy on the
//  local network is something only the core can do, and this app asks it to —
//  it never opens a listening socket of its own. A network change moves that
//  listener from one LAN address to another. It does not move 127.0.0.1:9090
//  or 127.0.0.1:9091.
//
//  Transparent capture does not come over the socket, and cannot: it is a
//  Network Extension that macOS installs and runs, and this app's part is
//  asking macOS to. The extension hands what it captures to the core over the
//  core's own loopback bridge, so the traffic still arrives here the one way
//  all traffic does — as exchanges on the IPC socket. The app turns it on
//  only after that bundled core is ready, and turns it off before stopping
//  the core.

import AppKit
import ProxyLabCore
import SwiftUI

@main
struct ProxyLabApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    /// Resolved once at launch: the socket path does not change while the app
    /// is running, and neither does the client reading it.
    @State private var startup = Startup.resolve()

    var body: some Scene {
        Window("ProxyLab", id: "traffic") {
            switch startup {
            case let .ready(
                traffic, rules, breakpoints, saved, pinned, simulator, android, iPhone, transparent,
                diagnostics, lifecycle, commands
            ):
                ProxyLabWindowView(
                    traffic: traffic,
                    rules: rules,
                    breakpoints: breakpoints,
                    saved: saved,
                    pinned: pinned,
                    simulator: simulator,
                    android: android,
                    iPhone: iPhone,
                    transparent: transparent,
                    diagnostics: diagnostics,
                    lifecycle: lifecycle,
                    commands: commands
                )
                .task {
                    delegate.capture = lifecycle
                    let watch = NetworkPathWatch()
                    watch.onChange = {
                        lifecycle.activeNetworkChanged()
                        await iPhone.networkDidChange(
                            primaryInterface: SystemConfigurationProxy.primaryInterfaceName()
                        )
                        simulator.noteRouteMayHaveChanged()
                    }
                    watch.start()
                    delegate.networkWatch = watch
                    await lifecycle.startOnLaunch()
                    simulator.noteRouteMayHaveChanged()
                }
            case let .failed(message):
                StartupFailureView(message: message)
            }
        }
        .defaultSize(width: 1_180, height: 760)
        // The minimum is the content's, and the window can be dragged larger
        // or smaller than the size it opens at. A fixed content size would
        // leave the sidebar's foot below the window with no way to reach it.
        .windowResizability(.contentMinSize)
        .windowToolbarStyle(.unified)
        .commands {
            ProxyLabCommands(router: startup.commandRouter)
        }
    }
}

/// Whether there is anything to connect to in the first place.
private enum Startup {
    /// The window's two models, both reading the one connection the app opens,
    /// and the pinned domains, which read nothing at all.
    case ready(
        TrafficInspectorModel,
        RulesModel,
        BreakpointsModel,
        SavedRequestsModel,
        PinnedDomainsStore,
        SimulatorSessionModel,
        AndroidSessionModel,
        IPhoneSetupModel,
        TransparentProxyModel,
        DiagnosticsModel,
        CaptureLifecycle,
        AppCommandRouter
    )
    /// The socket path could not be worked out at all.
    case failed(String)

    /// The menu router, once the window's models exist.
    var commandRouter: AppCommandRouter? {
        if case let .ready(_, _, _, _, _, _, _, _, _, _, _, router) = self {
            return router
        }
        return nil
    }

    /// Works out where the core is and builds the models that read it.
    ///
    /// One `IPCClient` for both screens. The traffic model owns the
    /// connection — it is what connects, subscribes, and reconnects — and the
    /// rules model makes its requests on the same client, so the app holds one
    /// socket however many screens are looking at it.
    @MainActor
    static func resolve() -> Startup {
        do {
            let path = try IPCSocketLocation.resolve()
            let client = IPCClient.overSocket(at: path)
            let traffic = TrafficInspectorModel(session: client, socketPath: path)
            let breakpoints = BreakpointsModel(session: client)
            let simulator = SimulatorSessionModel()
            let android = AndroidSessionModel()
            let iPhone = IPhoneSetupModel(session: client)
            let transparent = TransparentProxyModel(control: SystemTransparentProxyControl())
            // The certificate is the core's, at the path the core uses. The
            // store is this user's trust settings. A prompt is refused once
            // quit has been accepted, and while Stop is already underway.
            let trust = MacOSCaptureTrust(
                certificateLocator: { CoreLocations.caCertificate() },
                store: SecurityInterceptionTrustStore()
            )
            let lifecycle = CaptureLifecycle(
                core: BundledCoreProcess(executable: BundledCoreExecutable.url()),
                ports: LoopbackPortCheck(),
                readiness: IPCCoreReadiness(socketPath: path),
                transparent: transparent,
                proxies: SystemConfigurationProxy(recordURL: SystemConfigurationProxy.defaultRecordURL()),
                trust: trust
            )
            trust.onActivity = { [weak lifecycle] text in
                lifecycle?.noteTrustActivity(text)
            }
            trust.authorizationPermitted = { [weak lifecycle] in
                AppAuthorizationWindow.mayPrompt && lifecycle?.acceptsTrustPrompt == true
            }
            lifecycle.onProxyChanged = { simulator.noteRouteMayHaveChanged() }
            // One stream, one consumer: the traffic model owns the
            // connection's notifications and hands on the two that are about
            // an exchange being held still.
            traffic.breakpoints = breakpoints

            let rules = RulesModel(session: client, chooser: OpenPanelFileChooser())
            // The same client again. A saved request is the core's, read
            // over the socket like the traffic and the rules; replaying
            // one goes through the traffic session, which is the one
            // replay path there is.
            let saved = SavedRequestsModel(session: client, traffic: client)
            // Reads those same models, and asks the core over the same
            // socket. It does not keep a second copy of their state. The
            // capture stages are the lifecycle's own history.
            let diagnostics = DiagnosticsModel(
                session: client,
                traffic: traffic,
                simulator: simulator,
                android: android,
                iPhone: iPhone,
                transparent: transparent,
                trust: trust
            )
            diagnostics.lifecycleTrace = { [weak lifecycle] in lifecycle?.trace }
            let commands = AppCommandRouter(
                traffic: traffic,
                rules: rules,
                saved: saved,
                breakpoints: breakpoints,
                performCapture: { action in
                    Task { await lifecycle.perform(action) }
                }
            )

            return .ready(
                traffic,
                rules,
                breakpoints,
                saved,
                // Read from this Mac's preferences, not from the core: a pin
                // is something the developer chose and the proxy never sees.
                PinnedDomainsStore(),
                // Reaches the core through neither the socket nor anything
                // else: it runs `simctl` and reads this Mac's proxy
                // configuration. Its defaults are the real ones.
                simulator,
                // The same again for Android, over `adb`, which is found
                // where the SDK is rather than on a PATH a Dock-launched app
                // does not have.
                android,
                // The same client again: whether the proxy is on the local
                // network is the core's to say and the core's to change, so
                // this asks over the socket like everything else.
                iPhone,
                // Reaches macOS, not the core: installing the extension and
                // switching its configuration are the system's to do. The
                // capture lifecycle is what asks, after the bundled core is
                // ready.
                transparent,
                diagnostics,
                lifecycle,
                commands
            )
        } catch let error as IPCError {
            return .failed(error.message)
        } catch {
            return .failed("The ProxyLab socket path could not be worked out.")
        }
    }
}

/// Shown when there is no socket path to connect to.
///
/// Not the same thing as a core that is not running, which the window handles
/// itself: this is the app having nowhere to look.
private struct StartupFailureView: View {
    let message: String

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "questionmark.folder")
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(.tertiary)
            Text("ProxyLab cannot find the core")
                .font(.system(size: 13, weight: .medium))
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
            Text("Set \(IPCSocketLocation.environmentVariable) to the socket the core is on.")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        }
        .padding(32)
        .frame(minWidth: 420, minHeight: 260)
    }
}

/// Whether a certificate-trust panel may be shown.
///
/// AppKit will not present one usefully once termination has been accepted.
/// This app returns `.terminateCancel` until cleanup finishes, and records
/// the moment it finally accepts termination so a trust prompt is not started
/// on the way out. The system-proxy restore during that cleanup is a
/// different panel, and is unchanged.
@MainActor
enum AppAuthorizationWindow {
    private(set) static var terminationAccepted = false

    static var mayPrompt: Bool { !terminationAccepted }

    static func noteTerminationAccepted() {
        terminationAccepted = true
    }
}

/// The one thing AppKit is needed for.
///
/// A Swift package's executable is not an application bundle, so nothing has
/// told AppKit that this process owns a window and a menu bar. `Scripts/make-app-bundle.sh`
/// builds a bundle that does; this makes `swift run ProxyLab` behave the same
/// way during development.
///
/// Quit is handled here, not in a view's `onDisappear`: closing the window
/// and Command-Q both ask the application to terminate. Termination is
/// cancelled until the system proxy has been restored and the live route no
/// longer points at ProxyLab. Returning `.terminateLater` would mark the app
/// as already quitting, and macOS then will not present the administrator
/// dialog that restoration needs.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// The capture lifecycle, set when the window is ready. Nil until then,
    /// which is also before the bundled core has been launched.
    var capture: CaptureLifecycle?

    /// Watches the network path for as long as the app is running.
    var networkWatch: NetworkPathWatch?

    /// Command-Q or the last window closing, while restoration is in progress.
    /// A second request does not start a second shutdown.
    private var quitAttempt: Task<Void, Never>?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        // Sudden termination kills the process with SIGKILL and skips every
        // quit path below. This app holds system proxy state, so that is not
        // a safe way to exit. SIGKILL from outside, a crash, and power loss
        // still skip it; nothing in this process can run after those.
        let process = ProcessInfo.processInfo
        process.disableSuddenTermination()
        process.disableAutomaticTermination(
            "ProxyLab restores the system HTTP and HTTPS proxy before it quits."
        )
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let capture else {
            AppAuthorizationWindow.noteTerminationAccepted()
            return .terminateNow
        }
        if capture.canExit {
            AppAuthorizationWindow.noteTerminationAccepted()
            return .terminateNow
        }
        if quitAttempt != nil { return .terminateCancel }

        quitAttempt = Task { @MainActor in
            await capture.quit()
            let done = capture.canExit
            self.quitAttempt = nil
            if done {
                NSApp.terminate(nil)
            } else {
                self.explainBlockedQuit(capture)
            }
        }
        return .terminateCancel
    }

    /// Command-Q was refused because the system proxy is still ProxyLab's, or
    /// could not be shown to be otherwise. Retry runs the same shutdown.
    /// Quitting anyway is not offered: the proxy engine would disappear while
    /// this Mac still sends HTTP and HTTPS to it.
    private func explainBlockedQuit(_ capture: CaptureLifecycle) {
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "Cannot quit safely"
        let detail = capture.statusDetail
            ?? "Cannot quit safely because this Mac is still using ProxyLab as its proxy. "
            + "The proxy engine was left running."
        alert.informativeText = detail.contains("still using ProxyLab")
            ? detail
            : "Cannot quit safely because this Mac is still using ProxyLab as its proxy.\n\n\(detail)"
        alert.addButton(withTitle: "Retry")
        alert.addButton(withTitle: "Keep Running")
        if alert.runModal() == .alertFirstButtonReturn {
            NSApp.terminate(nil)
        }
    }
}
