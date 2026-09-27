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
//      3. check what it routes through SCDynamicStoreCopyProxies, read-only
//
//  The third is a check and not a change. A simulator has no proxy settings of
//  its own — see `ProxyRoute` for how that was established — so the only thing
//  that decides whether its traffic reaches ProxyLab is this Mac's own proxy
//  configuration, and ProxyLab does not touch that. When it already points at
//  the proxy, the simulator is connected and there is nothing left to do. When
//  it does not, the certificate is trusted all the same and the interface says
//  exactly what is missing rather than reporting a success that would not
//  capture anything.
//
//  # What Disconnect does
//
//  It ends the session this model is keeping, and that is all it honestly can
//  do. ProxyLab changed no proxy setting, so there is none to put back — the
//  preservation this step asks for is satisfied by never having written one.
//  The CA stays trusted, because `simctl keychain` can add a certificate or
//  erase the whole keychain and nothing in between, and erasing a developer's
//  simulator keychain is not the opposite of adding one certificate to it.
//  Both of those are said out loud rather than left for someone to discover.
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

    private let catalog: SimulatorCatalog
    private let trust: SimulatorTrustStore
    private let routes: any ProxyRouteReading
    private let endpoint: ProxyEndpoint
    private let certificatePath: String?

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

    /// What the status line says.
    public var statusText: String {
        switch state {
        case .notConnected:
            simulators.isEmpty ? "No simulator booted" : "Not connected"
        case .connecting:
            "Connecting…"
        case let .connected(udid):
            "Connected — \(name(of: udid)) via \(endpoint)"
        case let .certificateTrusted(udid):
            "CA trusted — \(name(of: udid)) is not routed to ProxyLab"
        }
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
        defer { isWorking = false }

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

    /// Trusts ProxyLab's CA in the selected simulator, then reports what that
    /// simulator will connect through.
    ///
    /// - Parameter coreIsRunning: whether the window's IPC connection is up,
    ///   which is this application's one answer to whether ProxyLab is
    ///   running.
    public func connect(coreIsRunning: Bool) async {
        guard !isWorking else { return }

        guard let simulator = selectedSimulator else {
            note = SimulatorNote(
                text: SimulatorToolsError.noBootedSimulator.message,
                isFailure: true
            )

            return
        }

        guard coreIsRunning else {
            state = .notConnected
            note = SimulatorNote(
                text: "ProxyLab is not running. Start the core — the connection indicator "
                    + "at the top of this window turns green — and connect again.",
                isFailure: true
            )

            return
        }

        guard let certificatePath else {
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
        // it is allowed to do".
        guard let route = try? routes.currentRoute() else {
            state = .certificateTrusted(udid: simulator.udid)
            note = SimulatorNote(
                text: "\(simulator.name) trusts ProxyLab's CA. "
                    + SimulatorToolsError.proxyRouteUnreadable.message,
                isFailure: true
            )

            return
        }

        guard route.reaches(endpoint) else {
            state = .certificateTrusted(udid: simulator.udid)
            note = SimulatorNote(text: routeAdvice(for: simulator, route: route), isFailure: true)

            return
        }

        state = .connected(udid: simulator.udid)
        note = SimulatorNote(
            text: "\(simulator.name) trusts ProxyLab's CA and routes through \(endpoint). "
                + "Its HTTP and HTTPS traffic appears in the list.",
            isFailure: false
        )
    }

    /// Ends the session, and says what it did not undo.
    public func disconnect() {
        guard let udid = state.udid else { return }

        let name = name(of: udid)
        state = .notConnected
        note = SimulatorNote(
            text: "\(name) disconnected. ProxyLab changed no proxy setting, so there is "
                + "nothing to restore. Its trust of the ProxyLab CA is left in place — "
                + "removing it means erasing that simulator's whole keychain, which "
                + "ProxyLab does not do.",
            isFailure: false
        )
    }

    /// What to say when the simulator trusts the CA but will not reach the
    /// proxy.
    ///
    /// The whole of the limitation in three sentences: what is true now, why
    /// ProxyLab cannot fix it, and the one thing that would.
    private func routeAdvice(for simulator: Simulator, route: ProxyRoute) -> String {
        "\(simulator.name) trusts ProxyLab's CA, but its traffic will not reach the proxy. "
            + "A simulator has no proxy settings of its own — it uses this Mac's, which are "
            + "currently \(route.summary). Set this Mac's HTTP and HTTPS proxy to "
            + "\(endpoint) to capture it; ProxyLab does not change that setting for you."
    }

    /// What to call a simulator that may since have been shut down.
    private func name(of udid: String) -> String {
        simulators.first { $0.udid == udid }?.name ?? "The simulator"
    }
}
