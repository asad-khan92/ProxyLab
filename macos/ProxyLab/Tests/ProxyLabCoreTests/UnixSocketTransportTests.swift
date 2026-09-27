//  The real socket, against a real socket.
//
//  Everything else in this package is tested through `FakeTransport`, which
//  proves the protocol and not the plumbing. These prove the plumbing: that
//  the transport connects to an `AF_UNIX` path, that bytes written arrive,
//  that bytes sent back are delivered, and that a socket that is not there
//  fails as a connection failure rather than as a crash.
//
//  The server they run against is `ScriptedSocketServer`, not the Rust core:
//  these are unit tests and need nothing installed to run.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Unix socket transport", .serialized)
struct UnixSocketTransportTests {
    @Test("a socket that is not there is a connection failure")
    func nothingListening() async throws {
        let path = NSTemporaryDirectory() + "pl-gone-\(UUID().uuidString.prefix(8)).sock"
        let transport = UnixSocketTransport(path: path)

        await #expect(throws: IPCError.cannotConnect(path: path)) { _ = try await transport.open() }
    }

    @Test("a socket path longer than the operating system allows is refused with a reason")
    func pathTooLong() async {
        let path = "/tmp/" + String(repeating: "x", count: 200) + ".sock"
        let transport = UnixSocketTransport(path: path)

        await #expect(throws: IPCError.self) { _ = try await transport.open() }

        do {
            _ = try await transport.open()
        } catch let error as IPCError {
            guard case .socketPathTooLong = error else {
                Issue.record("expected a path-length failure, got \(error)")
                return
            }
        } catch {
            Issue.record("expected an IPC error")
        }
    }

    @Test("a request written to the socket comes back answered")
    func requestResponse() async throws {
        let server = try ScriptedSocketServer()
        try server.start { line in
            let id = requestID(in: line) ?? "?"

            return .reply([Fixtures.clearResponse(requestID: id, removed: 5)])
        }
        defer { server.stop() }

        let client = IPCClient.overSocket(at: server.path)
        try await client.connect()
        defer { Task { await client.disconnect() } }

        #expect(try await client.clearTraffic().removed == 5)
    }

    @Test("a whole exchange survives the socket, base64 bodies and all")
    func wholeExchange() async throws {
        let server = try ScriptedSocketServer()
        try server.start { line in
            .reply([Fixtures.getResponse(requestID: requestID(in: line) ?? "?")])
        }
        defer { server.stop() }

        let client = IPCClient.overSocket(at: server.path)
        try await client.connect()
        defer { Task { await client.disconnect() } }

        let exchange = try await client.traffic(id: "0199c0de-0000-7000-8000-00000000002a")

        #expect(exchange.request.headers.count == 3)
        #expect(exchange.request.body.data == Data(#"{"name":"ada"}"#.utf8))
        #expect(exchange.response?.body.truncated == true)
    }

    @Test("events sent without being asked reach the notification stream")
    func events() async throws {
        let server = try ScriptedSocketServer()
        try server.start { line in
            .reply([
                Fixtures.subscribeResponse(requestID: requestID(in: line) ?? "?"),
                Fixtures.trafficAdded(),
                Fixtures.trafficLost(4),
            ])
        }
        defer { server.stop() }

        let client = IPCClient.overSocket(at: server.path)
        try await client.connect()
        defer { Task { await client.disconnect() } }

        try await client.subscribe()

        var notifications = client.notifications.makeAsyncIterator()
        guard case let .event(.trafficAdded(summary)) = await notifications.next() else {
            Issue.record("the added exchange should have arrived")
            return
        }
        #expect(summary.method == "POST")

        guard case .event(.trafficLost(missed: 4)) = await notifications.next() else {
            Issue.record("the loss should have arrived")
            return
        }
    }

    @Test("a core that goes away leaves nothing waiting")
    func serverStops() async throws {
        let server = try ScriptedSocketServer()
        // Every line the server receives, so the test can stop it at exactly
        // the moment a request is outstanding rather than after a delay.
        let (received, arrived) = AsyncStream<String>.makeStream()
        try server.start { line in
            arrived.yield(line)

            return .silence
        }

        let client = IPCClient.overSocket(at: server.path)
        try await client.connect()

        let pending = Task { try await client.clearTraffic() }

        var requests = received.makeAsyncIterator()
        _ = await requests.next()
        server.stop()

        await #expect(throws: IPCError.self) { _ = try await pending.value }
        #expect(await client.isConnected == false)

        var notifications = client.notifications.makeAsyncIterator()
        guard case .connectionLost = await notifications.next() else {
            Issue.record("the connection loss should have been reported")
            return
        }
    }
}

/// The `request_id` of a line the client sent.
private func requestID(in line: String) -> String? {
    guard
        let object = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
    else {
        return nil
    }

    return object["request_id"] as? String
}
