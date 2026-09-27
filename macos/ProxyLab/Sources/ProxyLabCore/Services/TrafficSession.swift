//  What the interface is allowed to ask the core.
//
//  The traffic operations the IPC protocol has, and nothing else: there is no
//  second path from a view to the core, and adding one would mean adding it
//  here first.
//
//  Writing the traffic out as a HAR file and reading one back in are here for
//  the same reason cURL is: they are the traffic screen's, they are about the
//  exchanges this list is showing, and a protocol of their own would be a
//  fifth thing to stand in for in a test for no separation gained.
//
//  Copying a request out as cURL, reading one back in, and sending one again
//  are here rather than in a protocol of their own. They belong to the traffic
//  screen — a request is copied out of an exchange, and a replay appears in
//  the list beside it — and a fourth protocol would be a fourth thing to stand
//  in for in a test for no separation gained.
//
//  It is a protocol for one reason — the view model's behaviour is testable
//  without a socket, a server, or a database. `IPCClient` is the only
//  implementation that ships.

import Foundation

/// A connection to the core, as the interface uses it.
public protocol TrafficSession: Sendable {
    /// Traffic events, and the loss of the connection.
    var notifications: AsyncStream<IPCNotification> { get }

    /// Opens the connection.
    func connect() async throws
    /// Closes it.
    func disconnect() async
    /// Asks for every exchange the proxy finishes from now on.
    func subscribe() async throws
    /// A page of the traffic list, newest first.
    func listTraffic(_ parameters: ListTrafficParameters) async throws -> TrafficListResult
    /// One exchange, whole.
    func traffic(id: String) async throws -> TrafficExchange
    /// A page of stream messages on one exchange, oldest first.
    func listStreamMessages(exchangeID: String, after: UInt64?, limit: Int?) async throws
        -> StreamMessageListResult
    /// One stream message, payload included.
    func streamMessage(id: String) async throws -> StreamMessage
    /// Removes one exchange.
    func deleteTraffic(id: String) async throws -> DeletedResult
    /// Removes every stored exchange.
    func clearTraffic() async throws -> ClearedResult
    /// Reads a cURL command as the request it describes.
    ///
    /// A command that does not parse comes back as a result saying so, not as
    /// a thrown error.
    func parseCurl(_ command: String) async throws -> CurlParseResult
    /// The cURL command that would send a stored request again.
    func curl(forExchange id: String) async throws -> CurlGenerated
    /// Sends a request through the proxy, as a new exchange.
    func replay(_ request: ReplayRequest) async throws -> ReplayResult
    /// Writes captured traffic out as a HAR file, at the path the export
    /// names.
    ///
    /// The core writes the file: a HAR document is the traffic with its
    /// bodies in it, and the app has no second copy of the format.
    func exportHar(_ export: HarExportRequest) async throws -> HarExportResult
    /// Reads the HAR file at `path`, and keeps what it describes as traffic.
    func importHar(path: String) async throws -> HarImportResult
}

extension IPCClient: TrafficSession {}
