//  The window, and which of its two screens is in it.
//
//  Both screens read the same connection: one `IPCClient`, one socket, one
//  subscription. The traffic model owns the connection's lifecycle — it is the
//  thing that subscribes and reconnects — and the rules model borrows the same
//  client to make rule requests on it. There is no second socket, and rules are
//  asked for only when the rules screen is on screen.
//
//  The breakpoints model reads the same client again, and is given the two
//  notifications that are about a held exchange by the traffic model rather
//  than by reading the stream a second time — there is one stream and one
//  consumer of it. Watching starts when this window appears and not when the
//  Breakpoints screen does: asking to watch is what makes the core pause at
//  all, and a breakpoint is set on one screen to catch traffic seen on
//  another.
//
//  The pinned domains are the window's, not the traffic model's: they are a
//  preference this application keeps, they outlive any one connection, and the
//  core is never told about them.
//
//  The saved requests are the window's too, and unlike the pins they are the
//  core's: they are read over the same socket, they appear at the foot of the
//  traffic sidebar, and they are given the same `IPCClient` everything else
//  has. The traffic screen is what shows them because that is where a request
//  is saved from and where its replay lands.
//
//  The simulator and emulator sessions are the window's for the same reason,
//  and reach the core through neither: one runs `simctl` and reads this Mac's
//  proxy configuration, the other runs `adb`. What they do need from the core
//  is whether it is running, which is the traffic model's connection and is
//  passed to them rather than asked for a second way.
//
//  The iPhone section is the window's too, and it does read the same socket:
//  whether the proxy is on the local network is something only the core can
//  say, and only the core can change. It is given the same `IPCClient` the
//  other two models have, so there is still one connection.
//
//  The transparent capture section is the window's as well, and reaches the
//  core through nothing at all: it asks macOS to install and switch a
//  Network Extension, and the extension reaches the core over the core's own
//  loopback bridge. What it needs from the window is whether the core is
//  running, for the same reason the device panels do.

import SwiftUI

/// ProxyLab's window: traffic, rules, or breakpoints.
public struct ProxyLabWindowView: View {
    private let traffic: TrafficInspectorModel
    private let rules: RulesModel
    private let breakpoints: BreakpointsModel
    private let saved: SavedRequestsModel
    private let pinned: PinnedDomainsStore
    private let simulator: SimulatorSessionModel
    private let android: AndroidSessionModel
    private let iPhone: IPhoneSetupModel
    private let transparent: TransparentProxyModel

    @State private var section: AppSection = .traffic

    public init(
        traffic: TrafficInspectorModel,
        rules: RulesModel,
        breakpoints: BreakpointsModel,
        saved: SavedRequestsModel,
        pinned: PinnedDomainsStore,
        simulator: SimulatorSessionModel,
        android: AndroidSessionModel,
        iPhone: IPhoneSetupModel,
        transparent: TransparentProxyModel
    ) {
        self.traffic = traffic
        self.rules = rules
        self.breakpoints = breakpoints
        self.saved = saved
        self.pinned = pinned
        self.simulator = simulator
        self.android = android
        self.iPhone = iPhone
        self.transparent = transparent
    }

    public var body: some View {
        content
            // Loaded here rather than when the Breakpoints screen appears:
            // asking to watch is what makes the core pause at all, and a
            // breakpoint set on one screen has to work while another is
            // showing.
            .task { await breakpoints.load() }
            // An exchange that is being held is an exchange that is waiting
            // for a person, so the screen that can answer it is brought up.
            // Traffic that silently stops is worse than a screen that
            // changes.
            .onChange(of: breakpoints.paused.count) { before, now in
                if now > before { section = .breakpoints }
            }
    }

    @ViewBuilder
    private var content: some View {
        switch section {
        case .traffic:
            TrafficInspectorView(
                model: traffic,
                saved: saved,
                pinned: pinned,
                simulator: simulator,
                android: android,
                iPhone: iPhone,
                transparent: transparent,
                section: $section
            )
        case .rules:
            RulesView(model: rules, section: $section)
        case .breakpoints:
            BreakpointsView(model: breakpoints, section: $section)
        }
    }

}
