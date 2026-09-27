//  The client's cURL and replay operations, over a transport a test writes to.
//
//  The same shape as the other client tests: a request goes out, the test
//  writes the answer the core would have written, and what comes back is
//  asserted. What is covered here that the protocol tests do not cover is the
//  round trip through the client itself — the request id, the correlation, and
//  the unwrapping of the result into the type the caller asked for.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("cURL client")
struct CurlClientTests {
    private func connected() async throws -> (IPCClient, FakeTransport) {
        let transport = FakeTransport()
        let client = IPCClient(makeTransport: { transport })
        try await client.connect()

        return (client, transport)
    }

    /// A response line of `type`, carrying `result`, for `id`.
    private func response(_ id: String, _ type: String, _ result: String) -> String {
        #"{"version":1,"request_id":"\#(id)","ok":true,"type":"\#(type)","result":\#(result)}"#
    }

    @Test("a parsed command comes back as the request it describes")
    func parse() async throws {
        let (client, transport) = try await connected()

        async let parsed = client.parseCurl("curl 'https://example.com/api'")
        let id = try await transport.nextRequestID()
        await transport.deliver(
            response(
                id,
                "parse_curl",
                #"{"request":{"method":"GET","url":"https://example.com/api","headers":[],"body":""},"notes":[]}"#
            ))

        let result = try await parsed
        #expect(result.request?.method == "GET")
        #expect(result.request?.url == "https://example.com/api")
        #expect(result.problem == nil)
    }

    @Test("a command that does not parse is a result, not a thrown error")
    func parseProblem() async throws {
        let (client, transport) = try await connected()

        async let parsed = client.parseCurl("curl --nope https://example.com/")
        let id = try await transport.nextRequestID()
        await transport.deliver(
            response(
                id,
                "parse_curl",
                #"{"problem":{"code":"unsupported_option","message":"not implemented (--nope)","option":"--nope"}}"#
            ))

        let result = try await parsed
        #expect(result.request == nil)
        #expect(result.problem?.option == "--nope")
    }

    @Test("the command sent is the one the caller gave, on one line")
    func sendsTheCommand() async throws {
        let (client, transport) = try await connected()
        let command = "curl 'https://example.com/api' \\\n  -H 'Accept: */*'"

        async let parsed = client.parseCurl(command)
        let line = await transport.nextSent()
        let sent = try JSONSerialization.jsonObject(with: line) as? [String: Any]
        let id = try #require(sent?["request_id"] as? String)
        await transport.deliver(
            response(
                id,
                "parse_curl",
                #"{"request":{"method":"GET","url":"https://example.com/","headers":[],"body":""}}"#
            ))
        _ = try await parsed

        // One JSON object on one line, whatever newlines the command had in
        // it: the newlines are inside a JSON string.
        let text = try #require(String(data: line, encoding: .utf8))
        #expect(text.hasSuffix("\n"))
        #expect(text.dropLast().contains("\n") == false)

        #expect(sent?["curl"] as? String == command)
    }

    @Test("a command for a stored request comes back with its truncation flag")
    func fromTraffic() async throws {
        let (client, transport) = try await connected()

        async let generated = client.curl(forExchange: "a")
        let id = try await transport.nextRequestID()
        await transport.deliver(
            response(id, "curl_from_traffic", #"{"curl":"curl 'x'","truncated":true}"#)
        )

        let result = try await generated
        #expect(result.curl == "curl 'x'")
        #expect(result.truncated)
    }

    @Test("a replay comes back with its exchange and its status")
    func replay() async throws {
        let (client, transport) = try await connected()

        async let replayed = client.replay(
            ReplayRequest(method: "POST", url: "https://example.com/api")
        )
        let id = try await transport.nextRequestID()
        await transport.deliver(
            response(
                id,
                "replay_request",
                #"{"exchange_id":"0199c0de-0000-7000-8000-00000000002a","status":201,"error":null}"#
            ))

        let result = try await replayed
        #expect(result.exchangeID == "0199c0de-0000-7000-8000-00000000002a")
        #expect(result.status == 201)
        #expect(result.succeeded)
    }

    @Test("a replay the core refuses is thrown, with the code it carried")
    func refused() async throws {
        let (client, transport) = try await connected()

        async let replayed = client.replay(ReplayRequest(method: "GET", url: "ftp://x.test/"))
        let id = try await transport.nextRequestID()
        await transport.deliver(
            #"{"version":1,"request_id":"\#(id)","ok":false,"error":{"code":"invalid_request","message":"ProxyLab sends http:// and https:// requests"}}"#
        )

        // Awaited first, so the thrown value is a plain `Result` the
        // expectation can look at: an `async let` cannot be captured by one.
        let outcome: Result<ReplayResult, any Error>
        do {
            outcome = .success(try await replayed)
        } catch {
            outcome = .failure(error)
        }

        guard case let .failure(error) = outcome else {
            Issue.record("the refusal was not thrown")
            return
        }
        #expect((error as? IPCError)?.serverCode == .invalidRequest)
    }

    @Test("a replay in flight does not stop another request being answered")
    func concurrent() async throws {
        let (client, transport) = try await connected()

        async let replayed = client.replay(ReplayRequest(method: "GET", url: "http://x.test/"))
        let replayID = try await transport.nextRequestID()

        async let cleared = client.clearTraffic()
        let clearID = try await transport.nextRequestID()

        // The second answered first: a slow replay is not a reason for the
        // rest of the app to wait.
        await transport.deliver(Fixtures.clearResponse(requestID: clearID, removed: 3))
        await transport.deliver(
            response(
                replayID,
                "replay_request",
                #"{"exchange_id":"0199c0de-0000-7000-8000-00000000002a","status":200,"error":null}"#
            ))

        #expect(try await cleared.removed == 3)
        #expect(try await replayed.status == 200)
    }
}
