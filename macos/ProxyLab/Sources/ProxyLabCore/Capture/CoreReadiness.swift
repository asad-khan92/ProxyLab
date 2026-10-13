//  Whether the core can take traffic, asked over the IPC socket it serves.
//
//      connect ──▶ diagnostics ──▶ localhost listening and the bridge listening
//
//  A connection that is not there yet is "not yet": the process may still be
//  opening its socket. A diagnostics answer whose bridge is unavailable is
//  final — the process has finished binding and the bridge is not there, so
//  transparent capture must not be turned on. The wait is a series of those
//  questions until one of them answers or the bound passes. Nothing here
//  treats a sleep as the core being ready.

import Foundation

/// What a readiness check concluded.
public enum CoreReadinessOutcome: Equatable, Sendable {
    /// The explicit listener and the transparent bridge are both listening.
    case ready
    /// The bound passed with no ready answer.
    case timedOut
    /// The core answered, and the bridge is not listening.
    case bridgeUnavailable(String)
    /// The owned process exited while the check was waiting.
    case coreExited
    /// The caller asked to stop waiting.
    case cancelled
}

/// Waiting until the core reports that it can receive traffic.
@MainActor
public protocol CoreReadinessWaiting: AnyObject {
    func waitUntilReady(
        isProcessRunning: @escaping @MainActor () -> Bool,
        isCancelled: @escaping @MainActor () -> Bool
    ) async -> CoreReadinessOutcome
}

/// Reads `diagnostics` until both listeners are up, or the bound passes.
@MainActor
public final class IPCCoreReadiness: CoreReadinessWaiting {
    /// How long a core is given to answer ready.
    public var timeout: Duration = .seconds(20)
    /// How long one diagnostics question is given.
    ///
    /// A socket that accepts and never answers must not hold this past the
    /// deadline. The next question is what decides.
    public var probeLimit: Duration = .seconds(2)
    /// How long to wait between answers that were "not yet".
    ///
    /// The next diagnostics call is what decides. This is only the gap
    /// between those calls.
    public var poll: Duration = .milliseconds(100)

    private let socketPath: String

    public init(socketPath: String) {
        self.socketPath = socketPath
    }

    public func waitUntilReady(
        isProcessRunning: @escaping @MainActor () -> Bool,
        isCancelled: @escaping @MainActor () -> Bool
    ) async -> CoreReadinessOutcome {
        let deadline = ContinuousClock.now.advanced(by: timeout)

        while ContinuousClock.now < deadline {
            if isCancelled() { return .cancelled }
            if !isProcessRunning() { return .coreExited }

            if let outcome = await probe() {
                return outcome
            }

            if isCancelled() { return .cancelled }
            if !isProcessRunning() { return .coreExited }

            let remaining = deadline - ContinuousClock.now
            if remaining <= .zero { break }
            try? await Task.sleep(for: min(poll, remaining))
        }

        if isCancelled() { return .cancelled }
        if !isProcessRunning() { return .coreExited }
        return .timedOut
    }

    /// One diagnostics answer, or `nil` when the socket is not there yet.
    ///
    /// The question is bounded. A core that accepts the socket and never
    /// answers is "not yet", and the deadline outside this call still applies.
    private func probe() async -> CoreReadinessOutcome? {
        let client = IPCClient.overSocket(at: socketPath)
        let outcome = await LifecycleBound.value(limit: probeLimit) {
            await self.ask(client)
        }
        await client.disconnect()
        return outcome.flatMap { $0 }
    }

    private func ask(_ client: IPCClient) async -> CoreReadinessOutcome? {
        do {
            try await client.connect()
        } catch {
            return nil
        }

        do {
            let report = try await client.diagnostics()
            return Self.interpret(report)
        } catch {
            return nil
        }
    }

    /// What one diagnostics answer means for capture startup.
    ///
    /// `nil` means the core has not finished saying so — a state other than
    /// listening or unavailable. Unavailable is an answer: the bridge will
    /// not appear later in this process.
    nonisolated static func interpret(_ report: CoreDiagnostics) -> CoreReadinessOutcome? {
        if report.localhost.state == "listening", report.transparent.state == "listening" {
            return .ready
        }
        if report.transparent.state == "unavailable" {
            return .bridgeUnavailable(
                report.transparent.message ?? "The transparent bridge is not listening."
            )
        }
        return nil
    }
}
