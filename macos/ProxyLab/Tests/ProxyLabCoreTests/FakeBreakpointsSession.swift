//  A core that answers breakpoint requests with whatever a test told it to.
//
//  The breakpoints model's job is what it does with an answer and with a
//  pause — what it shows, what it sends next, what it takes off the queue —
//  and none of that needs a socket or any traffic. Every call returns what it
//  was configured to return and records that it happened, so a test asserts on
//  the requests that were made rather than on a delay.

import Foundation

@testable import ProxyLabCore

/// One call a test can assert was made.
enum BreakpointCall: Equatable {
    case list
    case create(NewBreakpoint)
    case update(String, BreakpointUpdate)
    case delete(String)
    case watch
    case resume(String, RequestEdit?, ResponseEdit?, WebSocketEdit?)
    case cancel(String)
    case throttleStatus
    case setThrottle(ThrottleSettings)
}

/// A `BreakpointsSession` whose answers are set by the test.
actor FakeBreakpointsSession: BreakpointsSession {
    /// Every call made, in order.
    private(set) var calls: [BreakpointCall] = []

    private var lists: [Result<[Breakpoint], IPCError>] = [.success([])]
    private var createResult: Result<Breakpoint, IPCError>?
    private var updateResult: Result<Breakpoint, IPCError>?
    private var deleteResult: Result<BreakpointDeletedResult, IPCError>?
    private var watchResult: Result<Void, IPCError> = .success(())
    private var resolveResult: Result<PauseResolvedResult, IPCError>?
    private var throttle: ThrottleSettings = .none
    private var throttleFailure: IPCError?

    // MARK: - What the test sets

    /// The answers to successive `listBreakpoints` calls, in order. The last
    /// is repeated once they run out.
    func answerLists(with answers: [Result<[Breakpoint], IPCError>]) { lists = answers }

    /// The answer to every `listBreakpoints` call.
    func answerList(with breakpoints: [Breakpoint]) { lists = [.success(breakpoints)] }

    func answerCreate(with answer: Result<Breakpoint, IPCError>) { createResult = answer }
    func answerUpdate(with answer: Result<Breakpoint, IPCError>) { updateResult = answer }
    func answerDelete(with answer: Result<BreakpointDeletedResult, IPCError>) {
        deleteResult = answer
    }
    func answerWatch(with answer: Result<Void, IPCError>) { watchResult = answer }
    func answerResolve(with answer: Result<PauseResolvedResult, IPCError>) {
        resolveResult = answer
    }
    func answerThrottle(with settings: ThrottleSettings) { throttle = settings }
    func failThrottle(with error: IPCError) { throttleFailure = error }

    /// The edits that were sent with a continue, in order.
    func edits() -> [(request: RequestEdit?, response: ResponseEdit?, websocket: WebSocketEdit?)] {
        calls.compactMap { call in
            guard case let .resume(_, request, response, websocket) = call else { return nil }

            return (request, response, websocket)
        }
    }

    // MARK: - BreakpointsSession

    func listBreakpoints() async throws -> [Breakpoint] {
        calls.append(.list)

        let answer = lists.count > 1 ? lists.removeFirst() : lists[0]

        return try answer.get()
    }

    func createBreakpoint(_ breakpoint: NewBreakpoint) async throws -> Breakpoint {
        calls.append(.create(breakpoint))

        guard let createResult else { throw IPCError.notConnected }

        return try createResult.get()
    }

    func updateBreakpoint(id: String, _ update: BreakpointUpdate) async throws -> Breakpoint {
        calls.append(.update(id, update))

        guard let updateResult else { throw IPCError.notConnected }

        return try updateResult.get()
    }

    func deleteBreakpoint(id: String) async throws -> BreakpointDeletedResult {
        calls.append(.delete(id))

        guard let deleteResult else { throw IPCError.notConnected }

        return try deleteResult.get()
    }

    func watchBreakpoints() async throws {
        calls.append(.watch)

        try watchResult.get()
    }

    func continueBreakpoint(
        pauseID: String,
        request: RequestEdit?,
        response: ResponseEdit?,
        websocket: WebSocketEdit?
    ) async throws -> PauseResolvedResult {
        calls.append(.resume(pauseID, request, response, websocket))

        return try (resolveResult ?? .success(
            PauseResolvedResult(pauseID: pauseID, resolved: true)
        )).get()
    }

    func cancelBreakpoint(pauseID: String) async throws -> PauseResolvedResult {
        calls.append(.cancel(pauseID))

        return try (resolveResult ?? .success(
            PauseResolvedResult(pauseID: pauseID, resolved: true)
        )).get()
    }

    func throttleStatus() async throws -> ThrottleSettings {
        calls.append(.throttleStatus)

        if let throttleFailure { throw throttleFailure }

        return throttle
    }

    func setThrottle(_ settings: ThrottleSettings) async throws -> ThrottleSettings {
        calls.append(.setThrottle(settings))

        if let throttleFailure { throw throttleFailure }
        throttle = settings

        return settings
    }
}
