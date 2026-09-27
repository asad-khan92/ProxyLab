//  ProxyLab's macOS application.
//
//      ProxyLab (SwiftUI)
//            │
//            │  Unix domain socket, one JSON object per line
//            ▼
//      proxy-ipc  ──▶  the Rust core
//
//  The whole of what this process does is draw what comes over that socket.
//  It holds no proxy logic: it does not listen on a port, terminate TLS, parse
//  HTTP, or keep a capture store. If the core is not running there is nothing
//  to show, and the window says so rather than inventing traffic.
//
//  The socket is resolved once, here, by `IPCSocketLocation` — the same rules
//  the core uses to decide where to listen.
//
//  Two things in the window do not come over that socket: the iOS Simulator
//  section runs `xcrun simctl` and reads this Mac's proxy configuration, and
//  the Android Emulator section runs `adb`. Neither changes anything about
//  this Mac, and both are told whether the core is running rather than looking
//  for it a second way.
//
//  Breakpoints come over the same socket again. Asking the core to send this
//  app the exchanges it holds still is also what makes it hold any: a core
//  nobody is watching pauses nothing, so this app crashing or being quit
//  cannot leave a request stranded.
//
//  The iPhone section does come over the socket. Putting the proxy on the
//  local network is something only the core can do, and this app asks it to —
//  it never opens a listening socket of its own, and it changes nothing about
//  this Mac's own network, proxy, or firewall settings.
//
//  Transparent capture does not come over the socket, and cannot: it is a
//  Network Extension that macOS installs and runs, and this app's part is
//  asking macOS to. The extension hands what it captures to the core over the
//  core's own loopback bridge, so the traffic still arrives here the one way
//  all traffic does — as exchanges on the IPC socket. Nothing turns it on at
//  launch.

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
                traffic, rules, breakpoints, saved, pinned, simulator, android, iPhone, transparent
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
                    transparent: transparent
                )
            case let .failed(message):
                StartupFailureView(message: message)
            }
        }
        .defaultSize(width: 1_180, height: 760)
        .windowToolbarStyle(.unified)
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
        TransparentProxyModel
    )
    /// The socket path could not be worked out at all.
    case failed(String)

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
            // One stream, one consumer: the traffic model owns the
            // connection's notifications and hands on the two that are about
            // an exchange being held still.
            traffic.breakpoints = breakpoints

            return .ready(
                traffic,
                RulesModel(session: client, chooser: OpenPanelFileChooser()),
                breakpoints,
                // The same client again. A saved request is the core's, read
                // over the socket like the traffic and the rules; replaying
                // one goes through the traffic session, which is the one
                // replay path there is.
                SavedRequestsModel(session: client, traffic: client),
                // Read from this Mac's preferences, not from the core: a pin
                // is something the developer chose and the proxy never sees.
                PinnedDomainsStore(),
                // Reaches the core through neither the socket nor anything
                // else: it runs `simctl` and reads this Mac's proxy
                // configuration. Its defaults are the real ones.
                SimulatorSessionModel(),
                // The same again for Android, over `adb`, which is found
                // where the SDK is rather than on a PATH a Dock-launched app
                // does not have.
                AndroidSessionModel(),
                // The same client again: whether the proxy is on the local
                // network is the core's to say and the core's to change, so
                // this asks over the socket like everything else.
                IPhoneSetupModel(session: client),
                // Reaches macOS, not the core: installing the extension and
                // switching its configuration are the system's to do. It
                // reads where things stand and changes nothing until asked.
                TransparentProxyModel(control: SystemTransparentProxyControl())
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

/// The one thing AppKit is needed for.
///
/// A Swift package's executable is not an application bundle, so nothing has
/// told AppKit that this process owns a window and a menu bar. `Scripts/make-app-bundle.sh`
/// builds a bundle that does; this makes `swift run ProxyLab` behave the same
/// way during development.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
