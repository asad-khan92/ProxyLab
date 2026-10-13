//  A macOS that answers transparent capture requests with whatever a test
//  told it to.
//
//  The real control needs a signed extension in /Applications and a person to
//  approve it, which a test run has neither of and must never cause. What the
//  model does with an answer — what it shows, what it lets be pressed, what it
//  refuses to claim — needs none of that.

import Foundation
import TransparentProxyKit

@testable import ProxyLabCore

enum TransparentCall: Equatable {
    case snapshot
    case activate
    case turnOn(UInt16)
    case turnOff
    case providerStatus
}

@MainActor
final class FakeTransparentProxyControl: TransparentProxyControl {
    var readiness: TransparentProxyReadiness
    var onChange: (@MainActor () -> Void)?

    private(set) var calls: [TransparentCall] = []

    /// What the system says; changed by the fake's own operations the way
    /// the real system would change it.
    var current: TransparentProxySnapshot
    var activation: Result<ActivationOutcome, TransparentProxyFailure> = .success(.installed)
    var turnOnError: Error?
    var turnOffError: Error?
    var status: ProviderStatus?
    /// When set, Turn On waits until ``finishTurnOn()``.
    var hangTurnOn = false
    /// When set, Turn Off waits until ``finishTurnOff()``.
    var hangTurnOff = false
    private var turnOnContinuation: CheckedContinuation<Void, Never>?
    private var turnOffContinuation: CheckedContinuation<Void, Never>?
    private var turnGeneration: UInt64 = 0

    init(
        readiness: TransparentProxyReadiness = .ready,
        extensionState: ExtensionState = .notInstalled,
        tunnel: TunnelState = .notConfigured,
        requiresActivation: Bool? = nil
    ) {
        self.readiness = readiness
        current = TransparentProxySnapshot(
            extensionState: extensionState,
            tunnel: tunnel,
            requiresActivation: requiresActivation
        )
    }

    func snapshot() async -> TransparentProxySnapshot {
        calls.append(.snapshot)
        return current
    }

    func activate() async throws -> ActivationOutcome {
        calls.append(.activate)
        let outcome = try activation.get()
        switch outcome {
        case .installed:
            current.extensionState = .installed
            current.requiresActivation = false
        case .awaitingApproval: current.extensionState = .awaitingApproval
        case .willCompleteAfterReboot: current.extensionState = .willCompleteAfterReboot
        }
        return outcome
    }

    func beginTurnOn() -> UInt64 {
        turnGeneration += 1
        return turnGeneration
    }

    func abandonInFlightWork() {
        turnGeneration += 1
    }

    func turnOn(_ configuration: ProviderConfiguration, generation: UInt64) async throws {
        calls.append(.turnOn(configuration.bridgePort))
        if hangTurnOn {
            await withCheckedContinuation { turnOnContinuation = $0 }
        }
        guard turnGeneration == generation else { return }
        if let turnOnError { throw turnOnError }
        current.tunnel = .on
    }

    func turnOff() async throws {
        calls.append(.turnOff)
        if hangTurnOff {
            await withCheckedContinuation { turnOffContinuation = $0 }
        }
        if let turnOffError { throw turnOffError }
        current.tunnel = .off
    }

    /// Lets a hung Turn On finish. Safe when nothing is waiting.
    func finishTurnOn() {
        let continuation = turnOnContinuation
        turnOnContinuation = nil
        continuation?.resume()
    }

    /// Lets a hung Turn Off finish. Safe when nothing is waiting.
    func finishTurnOff() {
        let continuation = turnOffContinuation
        turnOffContinuation = nil
        continuation?.resume()
    }

    func providerStatus() async -> ProviderStatus? {
        calls.append(.providerStatus)
        return status
    }

    /// What the system does when somebody approves the extension in System
    /// Settings: it is installed now, and it says something changed.
    func approve() {
        current.extensionState = .installed
        current.requiresActivation = false
        onChange?()
    }

    /// The count of one kind of call.
    func count(_ call: TransparentCall) -> Int {
        calls.filter { $0 == call }.count
    }
}
