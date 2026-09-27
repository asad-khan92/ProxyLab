//  A core that answers LAN requests with whatever a test told it to.
//
//  The iPhone model's job is what it does with an answer — what it shows, what
//  it lets be clicked, what it refuses to claim until the core agrees — and
//  none of that needs a socket, a network, or an address this machine really
//  has. Every call returns what it was configured to return and records that
//  it happened, so a test asserts on the requests that were made.

import Foundation

@testable import ProxyLabCore

/// One call a test can assert was made.
enum LanCall: Equatable {
    case status
    case enable(String)
    case disable
}

/// A `LanSession` whose answers are set by the test.
actor FakeLanSession: LanSession {
    /// Every call made, in order.
    private(set) var calls: [LanCall] = []

    private var statuses: [Result<LanStatusResult, IPCError>] = []
    private var enableResult: Result<LanStatusResult, IPCError>?
    private var disableResult: Result<LanStatusResult, IPCError>?

    /// Resolved when the test lets a call in flight finish, for the tests
    /// that are about what is on screen while one is.
    private var gate: CheckedContinuation<Void, Never>?
    private var gated = false

    // MARK: - What the test sets

    /// The answers to successive `lanStatus` calls, in order.
    ///
    /// The last one is repeated once they run out, so a test that only cares
    /// about the first status sets one.
    func answerStatuses(with answers: [Result<LanStatusResult, IPCError>]) { statuses = answers }

    func answerStatus(with status: LanStatusResult) { statuses = [.success(status)] }
    func answerEnable(with answer: Result<LanStatusResult, IPCError>) { enableResult = answer }
    func answerDisable(with answer: Result<LanStatusResult, IPCError>) { disableResult = answer }

    // MARK: - What the model calls

    func lanStatus() async throws -> LanStatusResult {
        calls.append(.status)

        return try await next(from: &statuses)
    }

    func enableLan(address: String) async throws -> LanStatusResult {
        calls.append(.enable(address))

        return try answer(enableResult)
    }

    func disableLan() async throws -> LanStatusResult {
        calls.append(.disable)

        return try answer(disableResult)
    }

    // MARK: - Helpers

    private func next(
        from answers: inout [Result<LanStatusResult, IPCError>]
    ) throws -> LanStatusResult {
        let answer = answers.count > 1 ? answers.removeFirst() : answers.first

        return try self.answer(answer)
    }

    private func answer(_ answer: Result<LanStatusResult, IPCError>?) throws -> LanStatusResult {
        switch answer {
        case let .success(status): status
        case let .failure(error): throw error
        case nil: throw IPCError.notConnected
        }
    }
}

/// Statuses a test can build on.
enum LanFixtures {
    static let en0 = LanAddress(interface: "en0", address: "192.168.1.42")
    static let en1 = LanAddress(interface: "en1", address: "10.0.0.7")

    /// The proxy on loopback and nothing else, with two addresses on offer.
    static let off = LanStatusResult(
        enabled: false,
        address: nil,
        port: 9_090,
        available: [en0, en1]
    )

    /// The proxy also on `en0`.
    static let on = LanStatusResult(
        enabled: true,
        address: en0.address,
        port: 9_090,
        available: [en0, en1]
    )

    /// A Mac with no usable local network address at all.
    static let noAddresses = LanStatusResult(
        enabled: false,
        address: nil,
        port: 9_090,
        available: []
    )
}
