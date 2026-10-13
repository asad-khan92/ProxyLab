//  The wire protocol in both directions.
//
//  What a request must carry, what a response must be read as, and the one
//  rule that keeps the two apart: an event is a message with no `ok`, and
//  answers nothing.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Request encoding")
struct RequestEncodingTests {
    private let encoder = JSONEncoder()

    /// The request as a dictionary, which is how the server reads it.
    private func encoded(_ operation: IPCOperation, id: String = "r1") throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: id, operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("every request names the protocol version, its id, and its operation")
    func envelope() throws {
        let operations: [IPCOperation] = [
            .listTraffic(ListTrafficParameters()),
            .getTraffic(exchangeID: "0199c0de-0000-7000-8000-00000000002a"),
            .deleteTraffic(exchangeID: "0199c0de-0000-7000-8000-00000000002a"),
            .clearTraffic,
            .subscribe,
        ]

        for operation in operations {
            let json = try encoded(operation, id: "r7")

            #expect(json["version"] as? Int == 1)
            #expect(json["request_id"] as? String == "r7")
            #expect(json["type"] as? String == operation.name)
        }
    }

    @Test("the operation names are the ones the server implements")
    func operationNames() {
        #expect(IPCOperation.listTraffic(ListTrafficParameters()).name == "list_traffic")
        #expect(IPCOperation.getTraffic(exchangeID: "x").name == "get_traffic")
        #expect(IPCOperation.deleteTraffic(exchangeID: "x").name == "delete_traffic")
        #expect(IPCOperation.clearTraffic.name == "clear_traffic")
        #expect(IPCOperation.subscribe.name == "subscribe")
    }

    @Test("an operation that takes no parameters is sent none")
    func noParameters() throws {
        // The server refuses a parameter it does not know, and a key whose
        // value is null is still a key.
        for operation in [IPCOperation.clearTraffic, .subscribe, .diagnostics] {
            let json = try encoded(operation)

            #expect(Set(json.keys) == ["version", "request_id", "type"])
        }
    }

    @Test("a list request sends only the filters that were set")
    func listDefaults() throws {
        let json = try encoded(.listTraffic(ListTrafficParameters()))

        #expect(Set(json.keys) == ["version", "request_id", "type"])
    }

    @Test("a list request carries its page size and every filter it was given")
    func listParameters() throws {
        let parameters = ListTrafficParameters(
            limit: 25,
            cursor: TrafficCursor(startedAtMs: 1_700_000_000_500, id: "abc"),
            host: "api.example.com",
            method: "GET",
            status: .class(4),
            startedAtOrAfter: Timestamp(seconds: 1_700_000_000, nanos: 0),
            startedBefore: Timestamp(seconds: 1_700_003_600, nanos: 0)
        )
        let json = try encoded(.listTraffic(parameters))

        #expect(json["limit"] as? Int == 25)
        #expect(json["host"] as? String == "api.example.com")
        #expect(json["method"] as? String == "GET")
        #expect((json["status"] as? [String: Any])?["class"] as? Int == 4)
        #expect((json["cursor"] as? [String: Any])?["started_at_ms"] as? Int == 1_700_000_000_500)
        #expect((json["cursor"] as? [String: Any])?["id"] as? String == "abc")
        #expect(
            (json["started_at_or_after"] as? [String: Any])?["seconds"] as? Int == 1_700_000_000
        )
        #expect((json["started_before"] as? [String: Any])?["seconds"] as? Int == 1_700_003_600)
    }

    @Test("a list request carries a search the way the server reads it")
    func searchParameter() throws {
        let json = try encoded(.listTraffic(ListTrafficParameters(search: "api/users")))

        #expect(json["search"] as? String == "api/users")
    }

    @Test("a list request names which end of the order it wants")
    func orderParameter() throws {
        let newest = try encoded(.listTraffic(ListTrafficParameters(order: .newest)))
        let oldest = try encoded(.listTraffic(ListTrafficParameters(order: .oldest)))

        #expect(newest["order"] as? String == "newest")
        #expect(oldest["order"] as? String == "oldest")
    }

    @Test("a search the user did not type is not sent")
    func absentSearchAndOrder() throws {
        // The server refuses parameters it does not know and reads a missing
        // order as newest-first, which is what every client sent before the
        // parameter existed.
        let json = try encoded(.listTraffic(ListTrafficParameters(limit: 10)))

        #expect(json["search"] == nil)
        #expect(json["order"] == nil)
        #expect(Set(json.keys) == ["version", "request_id", "type", "limit"])
    }

    @Test("a search is sent as the characters that were typed")
    func searchIsNotReinterpreted() throws {
        for term in ["50%", "a_b", "back\\slash", "q=1&r=2", "ünïcodé", "\"quoted\""] {
            let json = try encoded(.listTraffic(ListTrafficParameters(search: term)))

            #expect(json["search"] as? String == term, "\(term)")
        }
    }

    @Test("an exact status filter is spelled the way the server reads it")
    func exactStatusFilter() throws {
        let json = try encoded(.listTraffic(ListTrafficParameters(status: .exact(404))))

        #expect((json["status"] as? [String: Any])?["exact"] as? Int == 404)
    }

    @Test("search flags and extra filters are sent only when they are set")
    func extraSearchFilters() throws {
        let json = try encoded(
            .listTraffic(
                ListTrafficParameters(
                    search: #"user-\d+"#,
                    searchRegex: true,
                    searchCaseSensitive: true,
                    contentType: "json",
                    httpVersion: .http2,
                    websocket: true,
                    sse: true,
                    failed: true
                )
            )
        )

        #expect(json["search"] as? String == #"user-\d+"#)
        #expect(json["search_regex"] as? Bool == true)
        #expect(json["search_case_sensitive"] as? Bool == true)
        #expect(json["content_type"] as? String == "json")
        #expect(json["http_version"] as? String == "HTTP/2.0")
        #expect(json["websocket"] as? Bool == true)
        #expect(json["sse"] as? Bool == true)
        #expect(json["failed"] as? Bool == true)

        let plain = try encoded(.listTraffic(ListTrafficParameters(search: "users")))
        #expect(plain["search_regex"] == nil)
        #expect(plain["websocket"] == nil)
        #expect(plain["failed"] == nil)
    }

    @Test("an operation about one exchange names it, and nothing else")
    func exchangeParameter() throws {
        let id = "0199c0de-0000-7000-8000-00000000002a"

        for operation in [IPCOperation.getTraffic(exchangeID: id), .deleteTraffic(exchangeID: id)] {
            let json = try encoded(operation)

            #expect(json["exchange_id"] as? String == id)
            #expect(Set(json.keys) == ["version", "request_id", "type", "exchange_id"])
        }
    }

    @Test("listing stream messages names the exchange and the optional page")
    func listStreamMessages() throws {
        let json = try encoded(
            .listStreamMessages(exchangeID: "ex-1", after: 12, limit: 50)
        )

        #expect(json["type"] as? String == "list_stream_messages")
        #expect(json["exchange_id"] as? String == "ex-1")
        #expect(json["after"] as? Int == 12)
        #expect(json["limit"] as? Int == 50)
    }

    @Test("getting one stream message names only that message")
    func getStreamMessage() throws {
        let json = try encoded(.getStreamMessage(streamMessageID: "msg-1"))

        #expect(json["type"] as? String == "get_stream_message")
        #expect(json["stream_message_id"] as? String == "msg-1")
        #expect(Set(json.keys) == ["version", "request_id", "type", "stream_message_id"])
    }

    @Test("continuing a WebSocket pause names the payload apart from HTTP edits")
    func continueWebSocket() throws {
        let json = try encoded(
            .continueBreakpoint(
                pauseID: "pause-1",
                request: nil,
                response: nil,
                websocket: WebSocketEdit(payload: Data("edited".utf8))
            )
        )

        #expect(json["type"] as? String == "continue_breakpoint")
        #expect(json["pause_id"] as? String == "pause-1")
        #expect((json["websocket"] as? [String: Any])?["payload"] as? String == "ZWRpdGVk")
        #expect(json["request"] == nil)
        #expect(json["response"] == nil)
    }
}

@Suite("Response decoding")
struct ResponseDecodingTests {
    private let decoder = JSONDecoder()

    private func decode(_ json: String) throws -> IPCMessage {
        try decoder.decode(IPCMessage.self, from: Data(json.utf8))
    }

    private func response(_ json: String) throws -> IPCResponse {
        guard case let .response(response) = try decode(json) else {
            Issue.record("expected a response")
            throw IPCError.malformedMessage
        }

        return response
    }

    @Test("a page of traffic decodes with its cursor and its total")
    func listResult() throws {
        let response = try response(Fixtures.listResponse(requestID: "r1", cursor: true))

        #expect(response.requestID == "r1")
        guard case let .listTraffic(page) = try #require(response.result) else {
            Issue.record("expected a traffic page")
            return
        }

        #expect(page.exchanges.count == 1)
        #expect(page.total == 1)
        #expect(page.nextCursor?.startedAtMs == 1_700_000_000_500)
    }

    @Test("an empty page is a page")
    func emptyList() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"list_traffic",\
            "result":{"exchanges":[],"next_cursor":null,"total":0}}
            """

        guard case let .listTraffic(page) = try #require(try response(json).result) else {
            Issue.record("expected a traffic page")
            return
        }

        #expect(page.exchanges.isEmpty)
        #expect(page.nextCursor == nil)
        #expect(page.total == 0)
    }

    @Test("a whole exchange decodes out of its wrapper")
    func getResult() throws {
        guard
            case let .getTraffic(exchange) = try #require(
                try response(Fixtures.getResponse(requestID: "r2")).result
            )
        else {
            Issue.record("expected an exchange")
            return
        }

        #expect(exchange.id == "0199c0de-0000-7000-8000-00000000002a")
        #expect(exchange.response?.status == 201)
    }

    @Test("a delete says which exchange, and whether there was one")
    func deleteResult() throws {
        let id = "0199c0de-0000-7000-8000-00000000002a"

        guard
            case let .deleteTraffic(deleted) = try #require(
                try response(Fixtures.deleteResponse(requestID: "r3", id: id, deleted: false))
                    .result
            )
        else {
            Issue.record("expected a delete result")
            return
        }

        #expect(deleted.exchangeID == id)
        #expect(!deleted.deleted)
    }

    @Test("a clear says how many exchanges it removed")
    func clearResult() throws {
        guard
            case let .clearTraffic(cleared) = try #require(
                try response(Fixtures.clearResponse(requestID: "r4", removed: 12)).result
            )
        else {
            Issue.record("expected a clear result")
            return
        }

        #expect(cleared.removed == 12)
    }

    @Test("a subscription is confirmed")
    func subscribeResult() throws {
        #expect(try response(Fixtures.subscribeResponse(requestID: "r5")).result == .subscribe)
    }

    @Test("a refusal carries a code to match on and a message to read")
    func errorResponse() throws {
        let response = try response(
            Fixtures.errorResponse(
                requestID: "r6",
                code: "not_found",
                message: "there is no stored exchange with that id"
            )
        )

        #expect(response.result == nil)
        #expect(response.error?.code == .notFound)
        #expect(response.error?.message == "there is no stored exchange with that id")
    }

    @Test("a failure the server could not correlate answers no request")
    func uncorrelatedError() throws {
        let response = try response(
            Fixtures.errorResponse(
                requestID: nil,
                code: "invalid_json",
                message: "a request must be one JSON object on one line"
            )
        )

        #expect(response.requestID == nil)
        #expect(response.error?.code == .invalidJSON)
    }

    @Test("an error code this build has no name for is still a code")
    func unknownErrorCode() throws {
        let response = try response(
            Fixtures.errorResponse(requestID: "r1", code: "something_new", message: "…")
        )

        #expect(response.error?.code == IPCErrorCode(rawValue: "something_new"))
    }

    @Test("a message naming another protocol version is refused rather than guessed at")
    func versionMismatch() {
        let json = #"{"version":2,"request_id":"r1","ok":true,"type":"clear_traffic","result":{}}"#

        #expect(throws: IPCError.unsupportedProtocolVersion(2)) {
            _ = try decode(json)
        }
    }

    @Test("a result this build cannot read is a protocol disagreement")
    func unknownResultType() {
        let json = #"{"version":1,"request_id":"r1","ok":true,"type":"replay","result":{}}"#

        #expect(throws: IPCError.malformedMessage) {
            _ = try decode(json)
        }
    }

    @Test("a line that is not JSON is not a message")
    func notJSON() {
        for line in ["not json", "{", "[]", "42"] {
            #expect(throws: (any Error).self) { _ = try decode(line) }
        }
    }
}

@Suite("Event decoding")
struct EventDecodingTests {
    private let decoder = JSONDecoder()

    private func event(_ json: String) throws -> IPCEvent {
        guard case let .event(event) = try decoder.decode(IPCMessage.self, from: Data(json.utf8))
        else {
            Issue.record("expected an event")
            throw IPCError.malformedMessage
        }

        return event
    }

    @Test("a finished exchange arrives as the row a list would show")
    func trafficAdded() throws {
        guard case let .trafficAdded(summary) = try event(Fixtures.trafficAdded()) else {
            Issue.record("expected traffic_added")
            return
        }

        #expect(summary.id == "0199c0de-0000-7000-8000-00000000002a")
        #expect(summary.method == "POST")
        #expect(summary.status == 201)
    }

    @Test("a subscriber that fell behind is told how much it missed")
    func trafficLost() throws {
        #expect(try event(Fixtures.trafficLost(12)) == .trafficLost(missed: 12))
    }

    @Test("a captured stream message arrives as the row a timeline would show")
    func streamMessageAdded() throws {
        guard case let .streamMessageAdded(summary) = try event(
            Fixtures.streamMessageAdded(Fixtures.streamSummaryJSON)
        ) else {
            Issue.record("expected stream_message_added")
            return
        }

        #expect(summary.id == "0199c0de-0000-7000-8000-000000000101")
        #expect(summary.exchangeID == "0199c0de-0000-7000-8000-00000000002a")
        #expect(summary.direction == .clientToServer)
        #expect(summary.kind == .websocketText)
        #expect(summary.payloadSize == 8)
    }

    @Test("a subscriber that fell behind on stream messages is told how many")
    func streamMessagesLost() throws {
        #expect(try event(Fixtures.streamMessagesLost(4)) == .streamMessagesLost(missed: 4))
    }

    @Test("an event is told apart from a response by having no ok")
    func discrimination() throws {
        // Both carry `type`, and it means different things in each: the
        // operation being answered in one, the event's own name in the other.
        let event = try decoder.decode(
            IPCMessage.self,
            from: Data(Fixtures.trafficAdded().utf8)
        )
        let response = try decoder.decode(
            IPCMessage.self,
            from: Data(Fixtures.clearResponse(requestID: "r1").utf8)
        )

        guard case .event = event else {
            Issue.record("a message with no ok is an event")
            return
        }
        guard case .response = response else {
            Issue.record("a message with ok is a response")
            return
        }
    }

    @Test("an event this build has no name for is a protocol disagreement")
    func unknownEvent() {
        let json = #"{"version":1,"type":"connection_opened","connection":{}}"#

        #expect(throws: IPCError.malformedMessage) {
            _ = try decoder.decode(IPCMessage.self, from: Data(json.utf8))
        }
    }

    @Test("an event naming another protocol version is refused")
    func versionMismatch() {
        let json = #"{"version":7,"type":"traffic_lost","missed":1}"#

        #expect(throws: IPCError.unsupportedProtocolVersion(7)) {
            _ = try decoder.decode(IPCMessage.self, from: Data(json.utf8))
        }
    }
}

@Suite("Line framing")
struct LineFramerTests {
    @Test("a whole line in one read is one message")
    func oneLine() throws {
        var framer = LineFramer()

        #expect(try framer.append(Data("{\"a\":1}\n".utf8)) == [Data("{\"a\":1}".utf8)])
    }

    @Test("several messages in one read are all of them")
    func severalLines() throws {
        var framer = LineFramer()
        let lines = try framer.append(Data("one\ntwo\nthree\n".utf8))

        #expect(lines.map { String(decoding: $0, as: UTF8.self) } == ["one", "two", "three"])
    }

    @Test("a message split across reads is held until it is whole")
    func splitLine() throws {
        var framer = LineFramer()

        #expect(try framer.append(Data("{\"a\":".utf8)).isEmpty)
        #expect(try framer.append(Data("1}".utf8)).isEmpty)
        #expect(try framer.append(Data("\n".utf8)) == [Data("{\"a\":1}".utf8)])
    }

    @Test("a read that ends mid-message keeps what it has")
    func partialAfterComplete() throws {
        var framer = LineFramer()
        let first = try framer.append(Data("one\ntw".utf8))

        #expect(first.map { String(decoding: $0, as: UTF8.self) } == ["one"])
        #expect(try framer.append(Data("o\n".utf8)).map { String(decoding: $0, as: UTF8.self) }
            == ["two"])
    }

    @Test("blank lines carry no message")
    func blankLines() throws {
        var framer = LineFramer()

        #expect(try framer.append(Data("\n\n".utf8)).isEmpty)
        #expect(try framer.append(Data("a\n\nb\n".utf8)).count == 2)
    }

    @Test("a line longer than the limit is refused rather than collected")
    func oversized() {
        var framer = LineFramer(limit: 8)

        #expect(throws: IPCError.messageTooLarge) {
            _ = try framer.append(Data(String(repeating: "x", count: 9).utf8))
        }
    }

    @Test("a run of complete lines is not measured against the limit")
    func manyShortLines() throws {
        var framer = LineFramer(limit: 8)
        let lines = try framer.append(Data("abc\ndef\nghi\n".utf8))

        #expect(lines.count == 3)
    }
}
