//  The client half of the IPC boundary.
//
//      request  ──▶ request_id ──▶ … ──▶ response with that request_id
//      subscribe ─▶                      traffic_added, traffic_lost, …
//
//  One connection carries both, interleaved, and that is the whole reason this
//  is an actor with a table of outstanding requests rather than a
//  send-then-read-a-line function. A `traffic_added` event may arrive between
//  a request going out and its answer coming back, and answering the request
//  with the event would be the kind of bug that only shows up on a busy proxy.
//
//  So there is exactly one reader. It decodes each line, and:
//
//      has `ok`  ──▶ a response ──▶ the request waiting on that `request_id`
//      has none  ──▶ an event    ──▶ the notification stream
//
//  # Several requests may be in flight
//
//  Every request gets an id of its own and is resumed by the reader when its
//  answer arrives, so nothing serialises requests behind one another and no
//  answer can be taken for another's.
//
//  # When the connection ends
//
//  Every outstanding request is failed, and the notification stream is told.
//  Nothing is left waiting on a socket that is gone: a request whose answer
//  can never arrive is a spinner that never stops.

import Foundation

/// Something the client reports without having been asked.
public enum IPCNotification: Sendable {
    /// The core sent a traffic event.
    case event(IPCEvent)
    /// The connection ended when it was not asked to.
    case connectionLost(IPCError)
}

/// A connection to the core, speaking the IPC protocol.
public actor IPCClient {
    /// Makes a new transport each time a connection is opened.
    ///
    /// A closure rather than a transport, because reconnecting needs a fresh
    /// socket: the old one is closed and cannot be reopened.
    private let makeTransport: @Sendable () -> any IPCTransport

    /// Events and connection losses, for as long as this client exists.
    ///
    /// One stream across every connection, so a consumer set up once does not
    /// have to be rebuilt each time the core is reconnected to.
    public nonisolated let notifications: AsyncStream<IPCNotification>
    private nonisolated let notify: AsyncStream<IPCNotification>.Continuation

    private var transport: (any IPCTransport)?
    private var reader: Task<Void, Never>?
    private var pending: [String: CheckedContinuation<IPCResponse, any Error>] = [:]
    private var nextRequestNumber: UInt64 = 0

    /// Which connection is current.
    ///
    /// A reader that ends belongs to the connection it was started for, and a
    /// late failure from a connection that has already been replaced must not
    /// take the new one down with it.
    private var generation: UInt64 = 0

    /// Whether the connection that is ending was asked to.
    private var closingDeliberately = false

    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    /// A client that connects through `makeTransport`.
    public init(makeTransport: @escaping @Sendable () -> any IPCTransport) {
        self.makeTransport = makeTransport
        (notifications, notify) = AsyncStream<IPCNotification>.makeStream(
            // Events are dropped rather than queued without bound if the app
            // ever falls behind, which is what the core does for a subscriber
            // that cannot keep up. The oldest goes: the newest traffic is the
            // traffic someone is watching for.
            bufferingPolicy: .bufferingNewest(4_096)
        )
    }

    /// A client that connects to the core's socket at `path`.
    public static func overSocket(at path: String) -> IPCClient {
        IPCClient(makeTransport: { UnixSocketTransport(path: path) })
    }

    deinit {
        notify.finish()
    }

    /// Whether there is a connection right now.
    public var isConnected: Bool { transport != nil }

    /// Opens a connection, replacing any there was.
    public func connect() async throws {
        await disconnect()

        let transport = makeTransport()
        let stream = try await transport.open()

        generation += 1
        let generation = self.generation
        self.transport = transport
        closingDeliberately = false

        reader = Task { [weak self] in
            var framer = LineFramer()

            do {
                for try await chunk in stream {
                    for line in try framer.append(chunk) {
                        await self?.receive(line)
                    }
                }
                await self?.connectionEnded(generation, because: .connectionClosed)
            } catch let error as IPCError {
                await self?.connectionEnded(generation, because: error)
            } catch {
                await self?.connectionEnded(
                    generation,
                    because: .transportFailure(error.localizedDescription)
                )
            }
        }
    }

    /// Closes the connection, if there is one.
    ///
    /// Deliberate, so no connection-lost notification is sent: the caller
    /// already knows.
    public func disconnect() async {
        guard transport != nil else { return }

        closingDeliberately = true
        generation += 1

        let transport = self.transport
        self.transport = nil
        reader?.cancel()
        reader = nil

        await transport?.close()
        failPending(with: .connectionClosed)
    }

    // MARK: - Operations

    /// A page of the traffic list, newest first.
    public func listTraffic(_ parameters: ListTrafficParameters = ListTrafficParameters())
        async throws -> TrafficListResult
    {
        guard case let .listTraffic(page) = try await send(.listTraffic(parameters)) else {
            throw IPCError.malformedMessage
        }

        return page
    }

    /// One exchange, whole.
    public func traffic(id: String) async throws -> TrafficExchange {
        guard case let .getTraffic(exchange) = try await send(.getTraffic(exchangeID: id)) else {
            throw IPCError.malformedMessage
        }

        return exchange
    }

    /// A page of stream messages on one exchange, oldest first.
    public func listStreamMessages(
        exchangeID: String,
        after: UInt64? = nil,
        limit: Int? = nil
    ) async throws -> StreamMessageListResult {
        guard case let .listStreamMessages(page) = try await send(
            .listStreamMessages(exchangeID: exchangeID, after: after, limit: limit)
        ) else {
            throw IPCError.malformedMessage
        }

        return page
    }

    /// One stream message, payload included.
    public func streamMessage(id: String) async throws -> StreamMessage {
        guard case let .getStreamMessage(message) = try await send(
            .getStreamMessage(streamMessageID: id)
        ) else {
            throw IPCError.malformedMessage
        }

        return message
    }

    /// Removes one exchange.
    public func deleteTraffic(id: String) async throws -> DeletedResult {
        guard case let .deleteTraffic(deleted) = try await send(.deleteTraffic(exchangeID: id))
        else {
            throw IPCError.malformedMessage
        }

        return deleted
    }

    /// Removes every stored exchange.
    public func clearTraffic() async throws -> ClearedResult {
        guard case let .clearTraffic(cleared) = try await send(.clearTraffic) else {
            throw IPCError.malformedMessage
        }

        return cleared
    }

    /// Starts receiving traffic events on this connection.
    public func subscribe() async throws {
        guard case .subscribe = try await send(.subscribe) else {
            throw IPCError.malformedMessage
        }
    }

    // MARK: - Rules

    /// Every rule, in the order the core consults them in.
    public func listRules() async throws -> [Rule] {
        guard case let .listRules(list) = try await send(.listRules) else {
            throw IPCError.malformedMessage
        }

        return list.rules
    }

    /// One rule, as the core holds it now.
    public func rule(id: String) async throws -> Rule {
        guard case let .getRule(rule) = try await send(.getRule(ruleID: id)) else {
            throw IPCError.malformedMessage
        }

        return rule
    }

    /// Stores a new rule, and gives back the rule the core minted.
    public func createRule(_ rule: NewRule) async throws -> Rule {
        guard case let .createRule(created) = try await send(.createRule(rule)) else {
            throw IPCError.malformedMessage
        }

        return created
    }

    /// Changes the fields `update` names, and gives back the whole rule.
    public func updateRule(id: String, _ update: RuleUpdate) async throws -> Rule {
        guard case let .updateRule(updated) = try await send(.updateRule(ruleID: id, update))
        else {
            throw IPCError.malformedMessage
        }

        return updated
    }

    /// Removes one rule.
    public func deleteRule(id: String) async throws -> RuleDeletedResult {
        guard case let .deleteRule(deleted) = try await send(.deleteRule(ruleID: id)) else {
            throw IPCError.malformedMessage
        }

        return deleted
    }

    // MARK: - cURL and replay

    /// Reads a cURL command as the request it describes.
    ///
    /// A command that does not parse comes back as a result carrying the
    /// reason, not as a thrown error: the core was asked to read a command
    /// and the answer is that it describes no request.
    public func parseCurl(_ command: String) async throws -> CurlParseResult {
        guard case let .parseCurl(parsed) = try await send(.parseCurl(command)) else {
            throw IPCError.malformedMessage
        }

        return parsed
    }

    /// The cURL command that would send a stored request again.
    ///
    /// Written by the core, from the request the core stored. The app never
    /// assembles one out of the parts of an exchange: the core is what knows
    /// what the request was.
    public func curl(forExchange id: String) async throws -> CurlGenerated {
        guard case let .curlFromTraffic(generated) = try await send(
            .curlFromTraffic(exchangeID: id)
        ) else {
            throw IPCError.malformedMessage
        }

        return generated
    }

    /// Sends `request` through the proxy, as a new exchange.
    public func replay(_ request: ReplayRequest) async throws -> ReplayResult {
        guard case let .replayRequest(outcome) = try await send(.replayRequest(request)) else {
            throw IPCError.malformedMessage
        }

        return outcome
    }

    // MARK: - The local network

    /// Whether the proxy is on the local network, and which of this Mac's
    /// addresses it could use.
    ///
    /// Asked for rather than remembered: the addresses are a property of the
    /// machine at that moment, and a Mac that has joined another network has
    /// different ones.
    public func lanStatus() async throws -> LanStatusResult {
        guard case let .lanStatus(status) = try await send(.lanStatus) else {
            throw IPCError.malformedMessage
        }

        return status
    }

    /// Puts the proxy on the local network at `address`, as well as on
    /// loopback.
    ///
    /// The core decides whether the address is one of this Mac's usable
    /// ones; an address that is not is a thrown ``IPCError`` carrying
    /// ``IPCErrorCode/lanUnavailable``.
    public func enableLan(address: String) async throws -> LanStatusResult {
        guard case let .enableLan(status) = try await send(.enableLan(address: address)) else {
            throw IPCError.malformedMessage
        }

        return status
    }

    /// Takes the proxy off the local network, leaving loopback alone.
    ///
    /// The answer comes back once the listening socket is closed, so a status
    /// saying LAN exposure is off is one that can be shown as off.
    public func disableLan() async throws -> LanStatusResult {
        guard case let .disableLan(status) = try await send(.disableLan) else {
            throw IPCError.malformedMessage
        }

        return status
    }

    // MARK: - Breakpoints

    /// Every configured breakpoint, in the order they are consulted in.
    public func listBreakpoints() async throws -> [Breakpoint] {
        guard case let .listBreakpoints(list) = try await send(.listBreakpoints) else {
            throw IPCError.malformedMessage
        }

        return list.breakpoints
    }

    /// Configures a new breakpoint and gives back the one the core minted.
    public func createBreakpoint(_ breakpoint: NewBreakpoint) async throws -> Breakpoint {
        guard case let .createBreakpoint(created) = try await send(
            .createBreakpoint(breakpoint)
        ) else {
            throw IPCError.malformedMessage
        }

        return created
    }

    /// Changes the fields `update` names, leaving the rest as they were.
    public func updateBreakpoint(
        id: String,
        _ update: BreakpointUpdate
    ) async throws -> Breakpoint {
        guard case let .updateBreakpoint(updated) = try await send(
            .updateBreakpoint(breakpointID: id, update)
        ) else {
            throw IPCError.malformedMessage
        }

        return updated
    }

    /// Removes one breakpoint, leaving whatever it is already holding alone.
    public func deleteBreakpoint(id: String) async throws -> BreakpointDeletedResult {
        guard case let .deleteBreakpoint(deleted) = try await send(
            .deleteBreakpoint(breakpointID: id)
        ) else {
            throw IPCError.malformedMessage
        }

        return deleted
    }

    /// Starts receiving the exchanges breakpoints hold still.
    ///
    /// Also what makes the core pause at all, which is why it is asked for
    /// rather than assumed: a core nobody is watching holds nothing still, so
    /// an app that crashes cannot strand a request.
    public func watchBreakpoints() async throws {
        guard case .watchBreakpoints = try await send(.watchBreakpoints) else {
            throw IPCError.malformedMessage
        }
    }

    /// Lets a paused exchange carry on, with whatever it carries changed.
    ///
    /// Exactly one of the two edits is acted on — the one matching the half
    /// the pause is for — and neither has to be there: continuing with no
    /// edit is what somebody who only wanted to look does.
    public func continueBreakpoint(
        pauseID: String,
        request: RequestEdit? = nil,
        response: ResponseEdit? = nil,
        websocket: WebSocketEdit? = nil
    ) async throws -> PauseResolvedResult {
        guard case let .continueBreakpoint(resolved) = try await send(
            .continueBreakpoint(
                pauseID: pauseID,
                request: request,
                response: response,
                websocket: websocket
            )
        ) else {
            throw IPCError.malformedMessage
        }

        return resolved
    }

    /// Stops a paused exchange here: nothing is sent, and nothing delivered.
    public func cancelBreakpoint(pauseID: String) async throws -> PauseResolvedResult {
        guard case let .cancelBreakpoint(resolved) = try await send(
            .cancelBreakpoint(pauseID: pauseID)
        ) else {
            throw IPCError.malformedMessage
        }

        return resolved
    }

    // MARK: - Throttling

    /// How the core is shaping the traffic it carries.
    public func throttleStatus() async throws -> ThrottleSettings {
        guard case let .throttleStatus(settings) = try await send(.throttleStatus) else {
            throw IPCError.malformedMessage
        }

        return settings
    }

    /// Shapes traffic this way from the next exchange on.
    public func setThrottle(_ settings: ThrottleSettings) async throws -> ThrottleSettings {
        guard case let .setThrottle(applied) = try await send(.setThrottle(settings)) else {
            throw IPCError.malformedMessage
        }

        return applied
    }

    // MARK: - Saved requests

    /// Every saved request, in the order they were saved.
    public func listSavedRequests() async throws -> [SavedRequest] {
        guard case let .listSavedRequests(list) = try await send(.listSavedRequests) else {
            throw IPCError.malformedMessage
        }

        return list.savedRequests
    }

    /// One saved request, as the core holds it now.
    public func savedRequest(id: String) async throws -> SavedRequest {
        guard case let .getSavedRequest(saved) = try await send(
            .getSavedRequest(savedRequestID: id)
        ) else {
            throw IPCError.malformedMessage
        }

        return saved
    }

    /// Keeps `request`, and gives back the one the core minted.
    public func createSavedRequest(_ request: NewSavedRequest) async throws -> SavedRequest {
        guard case let .createSavedRequest(saved) = try await send(
            .createSavedRequest(request)
        ) else {
            throw IPCError.malformedMessage
        }

        return saved
    }

    /// Keeps the request of one captured exchange, under `name`.
    ///
    /// The request is the core's: it stored the request and is what knows what
    /// it was, so nothing here assembles one out of the parts of a row. A
    /// captured body the core cut short comes back marked truncated.
    public func saveRequest(fromExchange id: String, name: String) async throws -> SavedRequest {
        guard case let .saveRequestFromTraffic(saved) = try await send(
            .saveRequestFromTraffic(exchangeID: id, name: name)
        ) else {
            throw IPCError.malformedMessage
        }

        return saved
    }

    /// Changes the fields `update` names, and gives back the whole request.
    public func updateSavedRequest(
        id: String,
        _ update: SavedRequestUpdate
    ) async throws -> SavedRequest {
        guard case let .updateSavedRequest(saved) = try await send(
            .updateSavedRequest(savedRequestID: id, update)
        ) else {
            throw IPCError.malformedMessage
        }

        return saved
    }

    /// Removes one saved request.
    public func deleteSavedRequest(id: String) async throws -> SavedRequestDeletedResult {
        guard case let .deleteSavedRequest(deleted) = try await send(
            .deleteSavedRequest(savedRequestID: id)
        ) else {
            throw IPCError.malformedMessage
        }

        return deleted
    }

    // MARK: - HAR

    /// Writes captured traffic out as a HAR file at the path the export names.
    ///
    /// The core writes the file. This app sends a path a person picked in a
    /// save panel and never assembles a HAR document itself — see
    /// ``HarExportRequest``.
    public func exportHar(_ export: HarExportRequest) async throws -> HarExportResult {
        guard case let .exportHar(result) = try await send(.exportHar(export)) else {
            throw IPCError.malformedMessage
        }

        return result
    }

    /// Reads the HAR file at `path`, and keeps what it describes as traffic.
    ///
    /// Nothing is sent to the network: an imported entry is a record, and
    /// replaying one is the separate, deliberate act every other exchange
    /// offers.
    public func importHar(path: String) async throws -> HarImportResult {
        guard case let .importHar(result) = try await send(.importHar(path: path)) else {
            throw IPCError.malformedMessage
        }

        return result
    }

    // MARK: - Correlation

    /// Sends one request and waits for the answer with its id.
    private func send(_ operation: IPCOperation) async throws -> IPCResult {
        guard let transport else { throw IPCError.notConnected }

        nextRequestNumber += 1
        let id = String(nextRequestNumber)

        var line = try encoder.encode(IPCRequest(requestID: id, operation: operation))
        line.append(UInt8(ascii: "\n"))

        let response: IPCResponse = try await withCheckedThrowingContinuation { resume in
            // Registered before the write, so an answer that arrives while
            // the write is still settling has somewhere to go.
            pending[id] = resume

            Task {
                do {
                    try await transport.send(line)
                } catch {
                    self.failRequest(id, with: error)
                }
            }
        }

        if let error = response.error { throw IPCError.server(error) }
        guard let result = response.result else { throw IPCError.malformedMessage }

        return result
    }

    /// Reads one line from the server and gives it to whatever is waiting.
    private func receive(_ line: Data) async {
        let message: IPCMessage
        do {
            message = try decoder.decode(IPCMessage.self, from: line)
        } catch let error as IPCError {
            // A line this build cannot read means the app and the core
            // disagree about the protocol, and carrying on would mean
            // guessing. The connection ends and the app reconnects.
            await connectionEnded(generation, because: error)
            return
        } catch {
            await connectionEnded(generation, because: .malformedMessage)
            return
        }

        switch message {
        case let .event(event):
            notify.yield(.event(event))
        case let .response(response):
            // A response the server could not correlate — it carries no
            // request id — answers nothing this client sent, because every
            // request it sends carries one.
            guard let id = response.requestID, let resume = pending.removeValue(forKey: id) else {
                return
            }

            resume.resume(returning: response)
        }
    }

    /// Fails one outstanding request, leaving the rest alone.
    private func failRequest(_ id: String, with error: any Error) {
        pending.removeValue(forKey: id)?.resume(throwing: error)
    }

    /// The connection this reader belonged to has ended.
    private func connectionEnded(_ generation: UInt64, because error: IPCError) async {
        // A connection that has already been replaced is not this one.
        guard generation == self.generation, transport != nil else { return }

        let transport = self.transport
        self.transport = nil
        self.generation += 1
        reader = nil

        await transport?.close()
        failPending(with: error)

        if !closingDeliberately {
            notify.yield(.connectionLost(error))
        }
    }

    /// Fails everything that was waiting on an answer.
    private func failPending(with error: IPCError) {
        let waiting = pending
        pending.removeAll()

        for resume in waiting.values {
            resume.resume(throwing: error)
        }
    }
}
