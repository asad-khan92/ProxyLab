//  A `simctl` that never runs.
//
//  Records what it was asked to run and answers what the test said to answer,
//  so every decision in this package is tested without a booted simulator and
//  without Xcode.

import Foundation

@testable import SimulatorTools

/// A command runner with canned answers.
final class FakeRunner: SimulatorCommandRunner, @unchecked Sendable {
    private let lock = NSLock()
    private var answers: [Result<CommandOutcome, any Error>]
    private var _invocations: [[String]] = []

    /// Every argument list it was given, in order.
    var invocations: [[String]] {
        lock.withLock { _invocations }
    }

    /// Answers each call with the next outcome, repeating the last one.
    init(answering answers: [Result<CommandOutcome, any Error>]) {
        self.answers = answers
    }

    /// Answers every call with one successful outcome.
    convenience init(succeedingWith output: Data = Data()) {
        self.init(answering: [.success(CommandOutcome(
            status: 0,
            standardOutput: output,
            standardError: ""
        ))])
    }

    /// Answers every call with one failure of `simctl`'s own.
    convenience init(failingWith status: Int32, saying message: String) {
        self.init(answering: [.success(CommandOutcome(
            status: status,
            standardOutput: Data(),
            standardError: message
        ))])
    }

    func run(_ arguments: [String]) async throws -> CommandOutcome {
        // `withLock` rather than `lock()`/`unlock()`: this is an async
        // function, and a lock held across a suspension point is one the
        // compiler is right to refuse.
        let answer = lock.withLock { () -> Result<CommandOutcome, any Error>? in
            _invocations.append(arguments)

            return answers.count > 1 ? answers.removeFirst() : answers.first
        }

        guard let answer else {
            return CommandOutcome(status: 0, standardOutput: Data(), standardError: "")
        }

        return try answer.get()
    }
}

/// A proxy configuration that is whatever the test says it is.
struct FakeRoute: ProxyRouteReading {
    let route: Result<ProxyRoute, any Error>

    init(_ route: ProxyRoute) { self.route = .success(route) }
    init(failing error: any Error) { route = .failure(error) }

    func currentRoute() throws -> ProxyRoute { try route.get() }
}
