//  A core that answers whatever a test told it to.
//
//  The view model's job is what it does with an answer, not how the answer got
//  there, so these tests give it answers directly. Nothing here opens a
//  socket, and nothing waits: every call returns what it was configured to
//  return, and the test knows when it happened because it awaited it.

import Foundation

@testable import ProxyLabCore

/// A `TrafficSession` whose answers are set by the test.
actor FakeSession: TrafficSession {
    nonisolated let notifications: AsyncStream<IPCNotification>
    private nonisolated let notify: AsyncStream<IPCNotification>.Continuation

    private var connectFailure: IPCError?
    private var subscribeFailure: IPCError?
    private var pages: [Result<TrafficListResult, IPCError>] = []
    private var exchanges: [String: Result<TrafficExchange, IPCError>] = [:]
    private var streamLists: [String: StreamMessageListResult] = [:]
    private var streamMessages: [String: Result<StreamMessage, IPCError>] = [:]
    private var deleteResult: Result<DeletedResult, IPCError>?
    private var clearResult: Result<ClearedResult, IPCError> = .success(ClearedResult(removed: 0))
    private var parseResults: [Result<CurlParseResult, IPCError>] = []
    private var curlResults: [String: Result<CurlGenerated, IPCError>] = [:]
    private var replayResult: Result<ReplayResult, IPCError> = .success(
        ReplayResult(exchangeID: "replayed", status: 200, error: nil)
    )
    private var exportResult: Result<HarExportResult, IPCError> = .success(
        HarExportResult(entries: 1, bytes: 512, truncated: false)
    )
    private var importResult: Result<HarImportResult, IPCError> = .success(
        HarImportResult(imported: 1, skipped: 0)
    )

    private(set) var connectCount = 0
    private(set) var subscribeCount = 0
    private(set) var listCount = 0
    private(set) var listRequests: [ListTrafficParameters] = []
    private(set) var deletedIDs: [String] = []
    private(set) var clearCount = 0
    private(set) var disconnectCount = 0
    private(set) var parsedCommands: [String] = []
    private(set) var curlRequests: [String] = []
    private(set) var replayed: [ReplayRequest] = []
    private(set) var exports: [HarExportRequest] = []
    private(set) var imports: [String] = []

    /// One value per `listTraffic` call, so a test can wait for the call to
    /// have happened rather than for a length of time.
    nonisolated let listCalls: AsyncStream<Int>
    private nonisolated let listed: AsyncStream<Int>.Continuation

    init() {
        (notifications, notify) = AsyncStream<IPCNotification>.makeStream()
        (listCalls, listed) = AsyncStream<Int>.makeStream()
    }

    /// Waits until `listTraffic` has been called `count` times.
    ///
    /// What a test waits on instead of sleeping: it resolves when the call
    /// happens and not a moment before or after.
    func waitForListCalls(_ count: Int) async {
        guard listCount < count else { return }

        for await calls in listCalls where calls >= count {
            return
        }
    }

    // MARK: - What the test sets

    /// Makes `connect` fail, or stop failing when given `nil`.
    func failConnect(with error: IPCError?) { connectFailure = error }

    /// Makes `subscribe` fail.
    func failSubscribe(with error: IPCError) { subscribeFailure = error }

    /// The answers to successive `listTraffic` calls, in order.
    ///
    /// The last one is repeated once they run out, so a test that only cares
    /// about the first page sets one.
    func answerLists(with answers: [Result<TrafficListResult, IPCError>]) { pages = answers }

    /// The answer to `traffic(id:)` for one exchange.
    func answerExchange(_ id: String, with answer: Result<TrafficExchange, IPCError>) {
        exchanges[id] = answer
    }

    /// The answer to `deleteTraffic`.
    func answerDelete(with answer: Result<DeletedResult, IPCError>) { deleteResult = answer }

    /// The answer to `clearTraffic`.
    func answerClear(with answer: Result<ClearedResult, IPCError>) { clearResult = answer }

    /// The answers to successive `parseCurl` calls, in order.
    ///
    /// The last is repeated once they run out, so a test that only cares
    /// about the first sets one.
    func answerParses(with answers: [Result<CurlParseResult, IPCError>]) { parseResults = answers }

    /// The answer to `curl(forExchange:)` for one exchange.
    func answerCurl(_ id: String, with answer: Result<CurlGenerated, IPCError>) {
        curlResults[id] = answer
    }

    /// The answer to `replay`.
    func answerReplay(with answer: Result<ReplayResult, IPCError>) { replayResult = answer }

    /// The answer to `exportHar`.
    func answerExport(with answer: Result<HarExportResult, IPCError>) { exportResult = answer }

    /// The answer to `importHar`.
    func answerImport(with answer: Result<HarImportResult, IPCError>) { importResult = answer }

    // MARK: - TrafficSession

    func connect() async throws {
        connectCount += 1
        if let connectFailure { throw connectFailure }
    }

    func disconnect() async {
        disconnectCount += 1
    }

    func subscribe() async throws {
        subscribeCount += 1
        if let subscribeFailure { throw subscribeFailure }
    }

    func listTraffic(_ parameters: ListTrafficParameters) async throws -> TrafficListResult {
        listCount += 1
        listRequests.append(parameters)
        listed.yield(listCount)

        guard !pages.isEmpty else {
            return TrafficListResult(exchanges: [], nextCursor: nil, total: 0)
        }

        return try (pages.count == 1 ? pages[0] : pages.removeFirst()).get()
    }

    func traffic(id: String) async throws -> TrafficExchange {
        guard let answer = exchanges[id] else {
            throw IPCError.server(
                IPCErrorBody(code: .notFound, message: "there is no stored exchange with that id")
            )
        }

        return try answer.get()
    }

    func listStreamMessages(exchangeID: String, after: UInt64?, limit: Int?) async throws
        -> StreamMessageListResult
    {
        streamLists[exchangeID] ?? StreamMessageListResult(messages: [])
    }

    func streamMessage(id: String) async throws -> StreamMessage {
        guard let answer = streamMessages[id] else {
            throw IPCError.server(
                IPCErrorBody(
                    code: .notFound,
                    message: "there is no stored stream message with that id"
                )
            )
        }

        return try answer.get()
    }

    func answerStreamList(_ exchangeID: String, with page: StreamMessageListResult) {
        streamLists[exchangeID] = page
    }

    func answerStreamMessage(_ id: String, with answer: Result<StreamMessage, IPCError>) {
        streamMessages[id] = answer
    }

    func deleteTraffic(id: String) async throws -> DeletedResult {
        deletedIDs.append(id)

        guard let deleteResult else { return DeletedResult(exchangeID: id, deleted: true) }

        return try deleteResult.get()
    }

    func clearTraffic() async throws -> ClearedResult {
        clearCount += 1

        return try clearResult.get()
    }

    func parseCurl(_ command: String) async throws -> CurlParseResult {
        parsedCommands.append(command)

        guard !parseResults.isEmpty else {
            return CurlParseResult(request: nil, notes: [], problem: nil)
        }

        return try (parseResults.count == 1 ? parseResults[0] : parseResults.removeFirst()).get()
    }

    func curl(forExchange id: String) async throws -> CurlGenerated {
        curlRequests.append(id)

        guard let answer = curlResults[id] else {
            throw IPCError.server(
                IPCErrorBody(code: .notFound, message: "there is no stored exchange with that id")
            )
        }

        return try answer.get()
    }

    func replay(_ request: ReplayRequest) async throws -> ReplayResult {
        replayed.append(request)

        return try replayResult.get()
    }

    func exportHar(_ export: HarExportRequest) async throws -> HarExportResult {
        exports.append(export)

        return try exportResult.get()
    }

    func importHar(path: String) async throws -> HarImportResult {
        imports.append(path)

        return try importResult.get()
    }
}
