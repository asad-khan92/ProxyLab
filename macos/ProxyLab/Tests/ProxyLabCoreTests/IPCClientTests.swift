//  The client: one reader, several requests, and events in between.
//
//  The behaviour that matters here is correlation. A response belongs to the
//  request whose id it carries and to no other, an event belongs to nobody,
//  and a connection that ends leaves nothing waiting for an answer that cannot
//  come.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("IPC client")
struct IPCClientTests {
    /// A client and the transport a test speaks through.
    private func connected() async throws -> (IPCClient, FakeTransport) {
        let transport = FakeTransport()
        let client = IPCClient(makeTransport: { transport })
        try await client.connect()

        return (client, transport)
    }

    @Test("a request is answered by the response carrying its id")
    func requestResponse() async throws {
        let (client, transport) = try await connected()

        async let page = client.listTraffic()
        let id = try await transport.nextRequestID()
        await transport.deliver(Fixtures.listResponse(requestID: id))

        let result = try await page
        #expect(result.total == 1)
        #expect(result.exchanges.first?.method == "POST")
    }

    @Test("every request carries an id of its own")
    func distinctRequestIDs() async throws {
        let (client, transport) = try await connected()

        async let first = client.clearTraffic()
        let firstID = try await transport.nextRequestID()
        await transport.deliver(Fixtures.clearResponse(requestID: firstID, removed: 1))
        _ = try await first

        async let second = client.clearTraffic()
        let secondID = try await transport.nextRequestID()
        await transport.deliver(Fixtures.clearResponse(requestID: secondID, removed: 2))
        _ = try await second

        #expect(firstID != secondID)
    }

    @Test("two requests in flight are answered in whatever order they finish")
    func outOfOrderResponses() async throws {
        let (client, transport) = try await connected()

        async let listing = client.listTraffic()
        let listID = try await transport.nextRequestID()

        async let clearing = client.clearTraffic()
        let clearID = try await transport.nextRequestID()

        // The second request answered first: the reader must not hand this to
        // whichever request happens to be first in the table.
        await transport.deliver(Fixtures.clearResponse(requestID: clearID, removed: 9))
        await transport.deliver(Fixtures.listResponse(requestID: listID))

        #expect(try await clearing.removed == 9)
        #expect(try await listing.total == 1)
    }

    @Test("an event that arrives while a request is waiting is not its answer")
    func eventDuringRequest() async throws {
        let (client, transport) = try await connected()

        async let page = client.listTraffic()
        let id = try await transport.nextRequestID()

        // The event first, then the answer. A client that read one line and
        // called it the response would return the event instead.
        await transport.deliver(Fixtures.trafficAdded())
        await transport.deliver(Fixtures.listResponse(requestID: id))

        #expect(try await page.total == 1)

        var events = client.notifications.makeAsyncIterator()
        guard case let .event(.trafficAdded(summary)) = await events.next() else {
            Issue.record("the event should have reached the notification stream")
            return
        }

        #expect(summary.id == "0199c0de-0000-7000-8000-00000000002a")
    }

    @Test("several messages arriving in one read are all delivered")
    func batchedMessages() async throws {
        let (client, transport) = try await connected()

        async let page = client.listTraffic()
        let id = try await transport.nextRequestID()

        await transport.deliverRaw(
            Fixtures.trafficAdded() + "\n"
                + Fixtures.trafficLost(3) + "\n"
                + Fixtures.listResponse(requestID: id) + "\n"
        )

        #expect(try await page.total == 1)

        var events = client.notifications.makeAsyncIterator()
        guard case .event(.trafficAdded) = await events.next(),
            case .event(.trafficLost(missed: 3)) = await events.next()
        else {
            Issue.record("both events should have been delivered, in order")
            return
        }
    }

    @Test("a message split across reads is waited for")
    func splitMessage() async throws {
        let (client, transport) = try await connected()

        async let page = client.listTraffic()
        let id = try await transport.nextRequestID()
        let response = Fixtures.listResponse(requestID: id)
        let middle = response.index(response.startIndex, offsetBy: 20)

        await transport.deliverRaw(String(response[..<middle]))
        await transport.deliverRaw(String(response[middle...]) + "\n")

        #expect(try await page.total == 1)
    }

    @Test("a refusal is thrown with the code the server chose")
    func serverRefusal() async throws {
        let (client, transport) = try await connected()

        let exchange = Task { try await client.traffic(id: "0199c0de-0000-7000-8000-00000000002a") }
        let id = try await transport.nextRequestID()
        await transport.deliver(
            Fixtures.errorResponse(
                requestID: id,
                code: "not_found",
                message: "there is no stored exchange with that id"
            )
        )

        await #expect(throws: IPCError.self) { _ = try await exchange.value }

        do {
            _ = try await exchange.value
        } catch let error as IPCError {
            #expect(error.serverCode == .notFound)
            #expect(error.message == "there is no stored exchange with that id")
        }
    }

    @Test("a line the client cannot read ends the connection rather than being guessed at")
    func malformedMessage() async throws {
        let (client, transport) = try await connected()

        let page = Task { try await client.listTraffic() }
        _ = try await transport.nextRequestID()
        await transport.deliver("{ not json")

        await #expect(throws: IPCError.malformedMessage) { _ = try await page.value }
        #expect(await client.isConnected == false)

        var events = client.notifications.makeAsyncIterator()
        guard case .connectionLost(.malformedMessage) = await events.next() else {
            Issue.record("the connection loss should have been reported")
            return
        }
    }

    @Test("a server speaking another protocol version is reported as that")
    func versionMismatch() async throws {
        let (client, transport) = try await connected()

        let page = Task { try await client.listTraffic() }
        _ = try await transport.nextRequestID()
        await transport.deliver(#"{"version":2,"request_id":"1","ok":true,"type":"x"}"#)

        await #expect(throws: IPCError.unsupportedProtocolVersion(2)) { _ = try await page.value }
    }

    @Test("a server that hangs up fails everything that was waiting")
    func serverDisconnect() async throws {
        let (client, transport) = try await connected()

        let first = Task { try await client.listTraffic() }
        _ = try await transport.nextRequestID()
        let second = Task { try await client.clearTraffic() }
        _ = try await transport.nextRequestID()

        await transport.hangUp()

        await #expect(throws: IPCError.connectionClosed) { _ = try await first.value }
        await #expect(throws: IPCError.connectionClosed) { _ = try await second.value }
        #expect(await client.isConnected == false)
    }

    @Test("a broken socket is reported as what the socket said")
    func transportFailure() async throws {
        let (client, transport) = try await connected()

        let page = Task { try await client.listTraffic() }
        _ = try await transport.nextRequestID()
        await transport.fail(.transportFailure("Connection reset by peer"))

        await #expect(throws: IPCError.transportFailure("Connection reset by peer")) {
            _ = try await page.value
        }
    }

    @Test("a socket that cannot be opened is a connection failure, not a crash")
    func connectionFailure() async throws {
        let transport = FakeTransport(failingWith: .cannotConnect(path: "/nowhere/proxylab.sock"))
        let client = IPCClient(makeTransport: { transport })

        await #expect(throws: IPCError.cannotConnect(path: "/nowhere/proxylab.sock")) {
            try await client.connect()
        }
        #expect(await client.isConnected == false)
    }

    @Test("a request made while disconnected is refused rather than queued")
    func requestWhileDisconnected() async {
        let client = IPCClient(makeTransport: { FakeTransport() })

        await #expect(throws: IPCError.notConnected) { _ = try await client.listTraffic() }
    }

    @Test("disconnecting is not reported as a lost connection")
    func deliberateDisconnect() async throws {
        let (client, transport) = try await connected()

        await client.disconnect()
        #expect(await client.isConnected == false)
        #expect(await transport.closeCount == 1)

        // The stream is checked by putting something on it that must arrive
        // after anything the disconnect would have sent: if disconnecting had
        // reported a lost connection, this would read that instead.
        let reconnected = FakeTransport()
        let second = IPCClient(makeTransport: { reconnected })
        try await second.connect()
        await reconnected.fail(.transportFailure("reset"))

        var events = client.notifications.makeAsyncIterator()
        var secondEvents = second.notifications.makeAsyncIterator()

        guard case .connectionLost(.transportFailure("reset")) = await secondEvents.next() else {
            Issue.record("a socket that failed is a lost connection")
            return
        }

        // And the first client, which was disconnected on purpose, said
        // nothing at all: its stream has nothing buffered.
        let pending = Task { await events.next() }
        pending.cancel()
        _ = await pending.value
    }

    @Test("reconnecting opens a new socket rather than reusing the old one")
    func reconnect() async throws {
        let transports = TransportFactory()
        let client = IPCClient(makeTransport: { transports.make() })

        try await client.connect()
        let first = try #require(transports.latest)
        await first.hangUp()

        // The reader notices, and the next connect must not be answered by
        // the transport that has already ended.
        try await client.connect()
        let second = try #require(transports.latest)
        #expect(first !== second)

        async let cleared = client.clearTraffic()
        let id = try await second.nextRequestID()
        await second.deliver(Fixtures.clearResponse(requestID: id, removed: 4))

        #expect(try await cleared.removed == 4)
    }

    @Test("the subscribe operation is confirmed before events are expected")
    func subscribe() async throws {
        let (client, transport) = try await connected()

        async let subscribed: Void = client.subscribe()
        let id = try await transport.nextRequestID()
        await transport.deliver(Fixtures.subscribeResponse(requestID: id))

        try await subscribed
    }

    @Test("deleting and clearing carry the core's answer back")
    func deleteAndClear() async throws {
        let (client, transport) = try await connected()
        let exchangeID = "0199c0de-0000-7000-8000-00000000002a"

        async let deleted = client.deleteTraffic(id: exchangeID)
        let deleteID = try await transport.nextRequestID()
        await transport.deliver(Fixtures.deleteResponse(requestID: deleteID, id: exchangeID))

        let result = try await deleted
        #expect(result.exchangeID == exchangeID)
        #expect(result.deleted)

        async let cleared = client.clearTraffic()
        let clearID = try await transport.nextRequestID()
        await transport.deliver(Fixtures.clearResponse(requestID: clearID, removed: 7))

        #expect(try await cleared.removed == 7)
    }

    @Test("a response the server could not correlate answers nothing")
    func uncorrelatedResponse() async throws {
        let (client, transport) = try await connected()

        async let page = client.listTraffic()
        let id = try await transport.nextRequestID()

        // A refusal with no request id, which the server sends when it could
        // not read one. It must not be handed to the request that is waiting.
        await transport.deliver(
            Fixtures.errorResponse(requestID: nil, code: "invalid_json", message: "…")
        )
        await transport.deliver(Fixtures.listResponse(requestID: id))

        #expect(try await page.total == 1)
    }
}

/// Hands out a new transport per connection, and remembers the last one.
private final class TransportFactory: @unchecked Sendable {
    private let lock = NSLock()
    private var transports: [FakeTransport] = []

    func make() -> FakeTransport {
        let transport = FakeTransport()
        lock.withLock { transports.append(transport) }

        return transport
    }

    var latest: FakeTransport? {
        lock.withLock { transports.last }
    }
}
