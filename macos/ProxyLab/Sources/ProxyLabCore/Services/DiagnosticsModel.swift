//  Asking where things stand, and keeping what recently went wrong.
//
//  The core's answer is asked for when the sheet is opened and when Refresh
//  is pressed. The device and Network Extension lines are the models the
//  window already has — refresh asks those models to look again, and does
//  not grow a second copy of their state. Problems this process sees and the
//  core cannot (a missing adb, a simulator that would not answer, an IPC
//  call that failed) are kept in a short in-memory list and forgotten when
//  the app quits.

import AndroidTools
import Foundation
import Observation
import SimulatorTools
import TransparentProxyKit

/// The core's diagnostics answer.
public protocol DiagnosticsSession: Sendable {
    func diagnostics() async throws -> CoreDiagnostics
}

extension IPCClient: DiagnosticsSession {}

/// The diagnostics sheet.
@MainActor
@Observable
public final class DiagnosticsModel {
    /// The core's last answer, when the last ask succeeded.
    public private(set) var core: CoreDiagnostics?
    /// Whether an ask is in flight.
    public private(set) var isRefreshing = false

    private let session: any DiagnosticsSession
    private let traffic: TrafficInspectorModel
    private let simulator: SimulatorSessionModel
    private let android: AndroidSessionModel
    private let iPhone: IPhoneSetupModel
    private let transparent: TransparentProxyModel
    private let trust: (any CaptureTrustReporting)?
    private let clipboard: any Clipboard
    private let identity: DiagnosticsIdentity
    private let clock: @Sendable () -> UInt64
    private var recent = RecentDiagnostics()

    /// The capture lifecycle's in-memory stages. Empty when this sheet has
    /// no lifecycle beside it.
    public var lifecycleTrace: @MainActor () -> CaptureLifecycleTrace? = { nil }

    public init(
        session: any DiagnosticsSession,
        traffic: TrafficInspectorModel,
        simulator: SimulatorSessionModel,
        android: AndroidSessionModel,
        iPhone: IPhoneSetupModel,
        transparent: TransparentProxyModel,
        trust: (any CaptureTrustReporting)? = nil,
        clipboard: any Clipboard = SystemClipboard(),
        identity: DiagnosticsIdentity = .current,
        clock: @escaping @Sendable () -> UInt64 = {
            UInt64(Date().timeIntervalSince1970 * 1000)
        }
    ) {
        self.session = session
        self.traffic = traffic
        self.simulator = simulator
        self.android = android
        self.iPhone = iPhone
        self.transparent = transparent
        self.trust = trust
        self.clipboard = clipboard
        self.identity = identity
        self.clock = clock
    }

    /// The sheet, from the state held right now.
    public var snapshot: DiagnosticsSnapshot {
        DiagnosticsPresentation.make(
            identity: identity,
            connection: traffic.connection,
            core: core,
            networkExtension: networkInput,
            simulator: simulatorInput,
            android: androidInput,
            trust: trust?.diagnostic,
            lifecycle: lifecycleTrace(),
            localProblems: recent.entries
        )
    }

    /// Asks the core, and the device models when `includingDevices` is set.
    ///
    /// Opening the sheet asks only the core. Refresh also asks the simulator,
    /// the emulator, the LAN status, and the Network Extension, which is the
    /// same look those panels already do.
    public func refresh(includingDevices: Bool) async {
        guard !isRefreshing else { return }

        isRefreshing = true
        defer { isRefreshing = false }

        trust?.refreshDiagnostic()
        await refreshCore()
        if includingDevices {
            await simulator.refresh()
            await android.refresh()
            await iPhone.refresh()
            await transparent.refresh()
        }
        noteCurrentProblems()
    }

    /// Copies the sanitized report.
    public func copyReport() {
        clipboard.copy(snapshot.report)
    }

    // MARK: - The ask

    private func refreshCore() async {
        guard case .connected = traffic.connection else {
            core = nil
            if case let .disconnected(message) = traffic.connection, let message, !message.isEmpty {
                recent.record(
                    level: "error",
                    component: "ipc",
                    operation: "connect",
                    message: message,
                    atMs: clock()
                )
            }
            return
        }

        do {
            core = try await session.diagnostics()
        } catch {
            core = nil
            recent.record(
                level: "error",
                component: "ipc",
                operation: "diagnostics",
                message: sentence(for: error),
                atMs: clock()
            )
        }
    }

    private func noteCurrentProblems() {
        let at = clock()
        if let problem = DiagnosticsPresentation.simulator(simulatorInput).problem {
            recent.record(level: "error", component: "ios", operation: "connect", message: problem, atMs: at)
        }
        if let problem = DiagnosticsPresentation.android(androidInput).problem {
            recent.record(
                level: "error",
                component: "android",
                operation: "configure",
                message: problem,
                atMs: at
            )
        }
        if let problem = DiagnosticsPresentation.networkExtension(networkInput).problem {
            recent.record(
                level: problem.level,
                component: "network_extension",
                operation: "status",
                message: problem.message,
                atMs: at
            )
        }
    }

    private func sentence(for error: Error) -> String {
        if let error = error as? IPCError { return error.message }

        return "The diagnostics request failed."
    }

    // MARK: - Inputs, read from the models that own them

    private var simulatorInput: SimulatorDiagnosticInput {
        let label = simulator.selected.flatMap { selected in
            simulator.simulators.first { $0.udid == selected }?.label
        }

        let presented = simulator.presentation(
            transparentCaptureListening: transparent.snapshot?.tunnel == .on
        )

        return SimulatorDiagnosticInput(
            hasRefreshed: simulator.hasRefreshed,
            bootedCount: simulator.simulators.count,
            selectedLabel: label,
            state: presented.connection,
            note: presented.note,
            proxyAddress: simulator.proxyAddress,
            trafficPath: presented.trafficPath
        )
    }

    private var androidInput: AndroidDiagnosticInput {
        AndroidDiagnosticInput(
            adbAvailable: android.isAdbAvailable,
            hasRefreshed: android.hasRefreshed,
            runningCount: android.emulators.count,
            selectedLabel: android.selectedEmulator?.label,
            state: android.state,
            note: android.note,
            proxyAddress: android.proxyAddress,
            adbMissingMessage: android.isAdbAvailable ? nil : AndroidToolsError.adbNotFound.message
        )
    }

    private var networkInput: NetworkExtensionDiagnosticInput {
        NetworkExtensionDiagnosticInput(
            readiness: transparent.readiness,
            snapshot: transparent.snapshot,
            bridge: transparent.providerStatus?.bridge,
            bridgeFailure: transparent.providerStatus?.lastBridgeFailure,
            lastError: transparent.lastError,
            systemDetail: transparent.lastErrorDetail,
            flows: transparent.providerStatus?.counters
        )
    }
}
