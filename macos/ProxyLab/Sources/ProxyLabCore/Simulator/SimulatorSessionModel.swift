//  Connecting a booted iOS Simulator to ProxyLab, and what that can and cannot
//  mean.
//
//      ┌ iOS Simulator ─────────────────┐
//      │ [ iPhone 17 Pro — iOS 26.1  ▾ ]│
//      │ ● Connected                    │
//      │ [ Connect ] [ Disconnect ]  ↻  │
//      └────────────────────────────────┘
//
//  # What Connect does
//
//  Two things, in this order, and it says which of them it got through:
//
//      1. the core is running          the window's own IPC connection
//      2. trust ProxyLab's CA          simctl keychain add-root-cert
//      3. how its traffic is captured  this Mac's HTTP and HTTPS proxy, when
//                                      it points at 127.0.0.1:9090; otherwise
//                                      transparent capture, when that is on
//
//  The third is a check and not a change. A simulator has no proxy settings of
//  its own — see `ProxyRoute` — so an explicit proxy is this Mac's HTTP and
//  HTTPS proxy. Capture's Start and Stop own that setting. This model only
//  reads it. When the proxy points at the explicit listener, that is the
//  route, including while transparent capture is also on: the simulator
//  connects to 127.0.0.1, and the Network Extension does not see that
//  connection. Transparent capture is the route only when the system proxy
//  does not. Trusting the CA is neither route. `127.0.0.1:9091` is not a
//  route and is not shown.
//
//  # What Disconnect does
//
//  It ends the session this model is keeping, and that is all it honestly can
//  do. This model changed no proxy setting — capture's Stop puts back the one
//  it saved — and the CA stays trusted, because `simctl keychain` can add a
//  certificate or erase the whole keychain and nothing in between, and erasing
//  a developer's simulator keychain is not the opposite of adding one
//  certificate to it. Both of those are said out loud rather than left for
//  someone to discover.
//
//  # Why the core's state is passed in
//
//  "Is ProxyLab running?" already has one answer in this application: the IPC
//  connection the window holds. This model is told it rather than opening a
//  socket, probing a port, or looking for a process — a second answer to that
//  question is a second thing that can be wrong.

import Foundation
import Observation
import SimulatorTools

/// Where a simulator stands with ProxyLab.
public enum SimulatorConnection: Equatable, Sendable {
    /// No simulator is connected.
    case notConnected
    /// Connect is part-way through.
    case connecting
    /// The CA is trusted and the simulator's traffic reaches the proxy.
    case connected(udid: String)
    /// The CA is trusted, but nothing is routing that simulator to the proxy.
    case certificateTrusted(udid: String)

    /// Which simulator this is about, when it is about one.
    public var udid: String? {
        switch self {
        case .notConnected, .connecting: nil
        case let .connected(udid), let .certificateTrusted(udid): udid
        }
    }
}

/// How a trusted simulator's traffic reaches ProxyLab, when it does.
public enum SimulatorTrafficPath: Equatable, Sendable {
    /// This Mac's HTTP and HTTPS proxy points at the explicit listener.
    ///
    /// This is the simulator's route whenever it is set, including while
    /// transparent capture is also on.
    case explicitProxy
    /// Transparent capture is listening, and the system proxy does not point
    /// at the explicit listener. The simulator is not shown a bridge port.
    case transparentCapture
}

/// What the simulator panel shows for one moment.
public struct SimulatorPresentation: Equatable, Sendable {
    public var connection: SimulatorConnection
    public var statusText: String
    public var note: SimulatorNote?
    public var trafficPath: SimulatorTrafficPath?

    public init(
        connection: SimulatorConnection,
        statusText: String,
        note: SimulatorNote?,
        trafficPath: SimulatorTrafficPath?
    ) {
        self.connection = connection
        self.statusText = statusText
        self.note = note
        self.trafficPath = trafficPath
    }
}

/// Something worth saying about the last thing that happened.
public struct SimulatorNote: Equatable, Sendable {
    /// The sentence to show.
    public let text: String
    /// Whether it is something that went wrong, which is how it is coloured.
    public let isFailure: Bool

    public init(text: String, isFailure: Bool) {
        self.text = text
        self.isFailure = isFailure
    }
}

/// The booted simulators, which one is selected, and what connecting it did.
@MainActor
@Observable
public final class SimulatorSessionModel {
    /// The simulators that were booted when this was last refreshed.
    public private(set) var simulators: [Simulator] = []

    /// Which one the picker has. Settable, because a picker binds to it.
    public var selected: String?

    /// Where the selected simulator stands.
    public private(set) var state: SimulatorConnection = .notConnected

    /// What the last action had to say, if anything.
    public private(set) var note: SimulatorNote?

    /// Whether a command is running, which is what disables the buttons.
    public private(set) var isWorking = false

    /// Whether discovery has been asked at least once.
    ///
    /// An empty list before that is "not checked", not "nothing is booted".
    public private(set) var hasRefreshed = false

    private let catalog: SimulatorCatalog
    private let trust: SimulatorTrustStore
    private let routes: any ProxyRouteReading
    private let endpoint: ProxyEndpoint
    private let certificatePath: String?

    /// Set once the CA install has succeeded, and cleared when the session ends.
    private var trustedUDID: String?
    /// The Mac proxy route read at the last successful trust, when it could be read.
    private var checkedRoute: ProxyRoute?
    /// The last trust succeeded and the proxy route could not be read.
    private var routeWasUnreadable = false
    /// The capture mode Connect was given. The panel asks again with the live one.
    private var reportedTransparentCapture = false
    /// Bumped when the system proxy may have changed, so a view that read the
    /// route reads it again.
    public private(set) var routeStamp = 0

    /// - Parameters:
    ///   - catalog: where the booted simulators are asked for.
    ///   - trust: what makes a simulator trust the CA.
    ///   - routes: what the simulator will connect through, read-only.
    ///   - endpoint: where the core's proxy listens.
    ///   - certificatePath: the CA certificate the core generated, or `nil`
    ///     when there is nowhere it could be.
    public init(
        catalog: SimulatorCatalog = SimulatorCatalog(),
        trust: SimulatorTrustStore = SimulatorTrustStore(),
        routes: any ProxyRouteReading = SystemProxyRoute(),
        endpoint: ProxyEndpoint = CoreLocations.proxyEndpoint,
        certificatePath: String? = CoreLocations.caCertificate()
    ) {
        self.catalog = catalog
        self.trust = trust
        self.routes = routes
        self.endpoint = endpoint
        self.certificatePath = certificatePath
    }

    /// The simulator the picker has selected, when it is still booted.
    public var selectedSimulator: Simulator? {
        guard let selected else { return nil }

        return simulators.first { $0.udid == selected }
    }

    /// Where the proxy is, for the interface to show.
    public var proxyAddress: String { endpoint.description }

    /// Whether Connect can be pressed.
    public var canConnect: Bool { !isWorking && selectedSimulator != nil }

    /// Whether Disconnect can be pressed.
    public var canDisconnect: Bool { !isWorking && state.udid != nil }

    /// What the status line says, for the capture mode Connect last saw.
    ///
    /// The panel uses ``presentation(transparentCaptureListening:)`` with the
    /// mode in force now, which is the same value until capture is switched.
    public var statusText: String {
        presentation(transparentCaptureListening: reportedTransparentCapture).statusText
    }

    /// Where the selected simulator stands, for the capture mode in force now.
    ///
    /// CA trust is whatever Connect already did. The route is read again, so a
    /// system proxy that changed after Connect is what is shown. Explicit
    /// proxy wins over transparent capture: that is the connection a simulator
    /// makes when this Mac's HTTP and HTTPS proxy points at the listener.
    public func presentation(transparentCaptureListening: Bool) -> SimulatorPresentation {
        _ = routeStamp
        guard let udid = trustedUDID else {
            return SimulatorPresentation(
                connection: state,
                statusText: idleStatusText,
                note: note,
                trafficPath: nil
            )
        }

        return routedPresentation(
            name: name(of: udid),
            udid: udid,
            transparentCaptureListening: transparentCaptureListening
        )
    }

    /// The system proxy may have changed. The next presentation reads it.
    public func noteRouteMayHaveChanged() {
        if let route = try? routes.currentRoute() {
            checkedRoute = route
            routeWasUnreadable = false
        }
        routeStamp += 1
    }

    /// Asks `simctl` which simulators are booted.
    ///
    /// Keeps the selection when the simulator it names is still booted, so a
    /// refresh does not move the picker under someone's hand, and drops the
    /// session when the connected simulator has been shut down — a simulator
    /// that is gone is not connected to anything.
    public func refresh() async {
        guard !isWorking else { return }

        isWorking = true
        defer {
            isWorking = false
            hasRefreshed = true
        }

        let booted: [Simulator]
        do {
            booted = try await catalog.bootedSimulators()
        } catch let error as SimulatorToolsError {
            note = SimulatorNote(text: error.message, isFailure: true)

            return
        } catch {
            note = SimulatorNote(
                text: SimulatorToolsError.deviceListUnreadable.message,
                isFailure: true
            )

            return
        }

        simulators = booted

        if let selected, !booted.contains(where: { $0.udid == selected }) {
            self.selected = nil
        }
        if selected == nil {
            selected = booted.first?.udid
        }

        if let connected = state.udid, !booted.contains(where: { $0.udid == connected }) {
            clearTrust()
            state = .notConnected
            note = SimulatorNote(
                text: "The connected simulator is no longer booted.",
                isFailure: true
            )

            return
        }

        note = booted.isEmpty
            ? SimulatorNote(text: SimulatorToolsError.noBootedSimulator.message, isFailure: true)
            : nil
    }

    /// Trusts ProxyLab's CA in the selected simulator, then reports how that
    /// simulator's traffic will be captured.
    ///
    /// - Parameters:
    ///   - coreIsRunning: whether the window's IPC connection is up, which is
    ///     this application's one answer to whether ProxyLab is running.
    ///   - transparentCaptureListening: whether transparent capture is on.
    ///     It is the route only when this Mac's HTTP and HTTPS proxy does not
    ///     already point at the explicit listener. The proxy is only read.
    public func connect(coreIsRunning: Bool, transparentCaptureListening: Bool = false) async {
        guard !isWorking else { return }

        guard let simulator = selectedSimulator else {
            note = SimulatorNote(
                text: SimulatorToolsError.noBootedSimulator.message,
                isFailure: true
            )

            return
        }

        guard coreIsRunning else {
            clearTrust()
            state = .notConnected
            note = SimulatorNote(
                text: "ProxyLab is not running. Start the core — the connection indicator "
                    + "at the top of this window turns green — and connect again.",
                isFailure: true
            )

            return
        }

        guard let certificatePath else {
            clearTrust()
            state = .notConnected
            note = SimulatorNote(
                text: "ProxyLab's CA certificate cannot be located: there is no home directory "
                    + "to look under. Set \(CoreLocations.caDirectoryVariable) to the "
                    + "core's CA directory.",
                isFailure: true
            )

            return
        }

        isWorking = true
        state = .connecting
        clearTrust()
        defer { isWorking = false }

        do {
            try await trust.trustRootCertificate(at: certificatePath, on: simulator.udid)
        } catch let error as SimulatorToolsError {
            state = .notConnected
            note = SimulatorNote(text: error.message, isFailure: true)

            return
        } catch {
            state = .notConnected
            note = SimulatorNote(
                text: "The simulator's trusted certificates could not be changed.",
                isFailure: true
            )

            return
        }

        // Trusted from here on, whatever the route turns out to be: saying so
        // is the difference between "connect failed" and "connect did the half
        // it is allowed to do". The route is read either way and never written.
        // Transparent capture does not consult it.
        trustedUDID = simulator.udid
        reportedTransparentCapture = transparentCaptureListening
        if let route = try? routes.currentRoute() {
            checkedRoute = route
            routeWasUnreadable = false
        } else {
            checkedRoute = nil
            routeWasUnreadable = true
        }

        let presented = presentation(transparentCaptureListening: transparentCaptureListening)
        state = presented.connection
        note = presented.note
    }

    /// Ends the session, and says what it did not undo.
    public func disconnect() {
        guard let udid = state.udid else { return }

        let name = name(of: udid)
        clearTrust()
        state = .notConnected
        note = SimulatorNote(
            text: "\(name) disconnected. Its trust of the ProxyLab CA is left in place — "
                + "removing it means erasing that simulator's whole keychain, which "
                + "ProxyLab does not do. This Mac's HTTP and HTTPS proxy is restored "
                + "when capture stops, not by disconnecting a simulator.",
            isFailure: false
        )
    }

    /// What to say when the simulator trusts the CA but will not reach the
    /// proxy.
    ///
    /// The whole of the limitation in three sentences: what is true now, why
    /// ProxyLab cannot fix it, and the one thing that would. Said only when
    /// transparent capture is not the route.
    private func routeAdvice(name: String, route: ProxyRoute) -> String {
        "\(name) trusts ProxyLab's CA, but its traffic will not reach the proxy. "
            + "A simulator has no proxy settings of its own — it uses this Mac's, which are "
            + "currently \(route.summary). ProxyLab sets this Mac's HTTP and HTTPS proxy to "
            + "\(endpoint) while capture is listening."
    }

    /// Status while no CA trust is being reported.
    private var idleStatusText: String {
        switch state {
        case .connecting:
            "Connecting…"
        case .notConnected:
            simulators.isEmpty ? "No simulator booted" : "Not connected"
        case .connected, .certificateTrusted:
            "Not connected"
        }
    }

    /// Trust is already done. Explicit proxy is the simulator's route when the
    /// system proxy points at the listener. Transparent capture is the route
    /// only otherwise. The two are not both announced.
    private func routedPresentation(
        name: String,
        udid: String,
        transparentCaptureListening: Bool
    ) -> SimulatorPresentation {
        let route = liveRoute()
        if route?.reaches(endpoint) == true {
            return SimulatorPresentation(
                connection: .connected(udid: udid),
                statusText: "Connected — \(name) via \(endpoint)",
                note: SimulatorNote(
                    text: "\(name) trusts ProxyLab's CA and routes through \(endpoint). "
                        + "Its HTTP and HTTPS traffic appears in the list.",
                    isFailure: false
                ),
                trafficPath: .explicitProxy
            )
        }

        if transparentCaptureListening {
            return SimulatorPresentation(
                connection: .connected(udid: udid),
                statusText: "Connected — \(name) via transparent capture",
                note: SimulatorNote(
                    text: "\(name) trusts ProxyLab's CA. This Mac's HTTP and HTTPS proxy "
                        + "does not point at \(endpoint), so the simulator is captured "
                        + "through transparent capture.",
                    isFailure: false
                ),
                trafficPath: .transparentCapture
            )
        }

        if routeWasUnreadable && route == nil {
            return SimulatorPresentation(
                connection: .certificateTrusted(udid: udid),
                statusText: "CA trusted — \(name) is not routed to ProxyLab",
                note: SimulatorNote(
                    text: "\(name) trusts ProxyLab's CA. "
                        + SimulatorToolsError.proxyRouteUnreadable.message,
                    isFailure: true
                ),
                trafficPath: nil
            )
        }

        let shown = route ?? checkedRoute ?? .direct
        return SimulatorPresentation(
            connection: .certificateTrusted(udid: udid),
            statusText: "CA trusted — \(name) is not routed to ProxyLab",
            note: SimulatorNote(text: routeAdvice(name: name, route: shown), isFailure: true),
            trafficPath: nil
        )
    }

    /// The route in force now, falling back to the one read at Connect.
    private func liveRoute() -> ProxyRoute? {
        if let route = try? routes.currentRoute() {
            return route
        }
        return routeWasUnreadable ? nil : checkedRoute
    }

    /// Forgets a trusted session. The CA in the simulator is untouched.
    private func clearTrust() {
        trustedUDID = nil
        checkedRoute = nil
        routeWasUnreadable = false
        reportedTransparentCapture = false
    }

    /// What to call a simulator that may since have been shut down.
    private func name(of udid: String) -> String {
        simulators.first { $0.udid == udid }?.name ?? "The simulator"
    }
}
