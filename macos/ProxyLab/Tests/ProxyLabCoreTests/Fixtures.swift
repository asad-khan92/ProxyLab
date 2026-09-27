//  The JSON the core actually sends, and the models behind it.
//
//  Every string here is the shape `core/ipc` writes: the same keys, the same
//  spelling, the same nesting. They are written out by hand rather than
//  produced by the encoder under test, because a test that builds its input
//  with the code it is testing agrees with itself no matter what the server
//  sends.

import Foundation

@testable import ProxyLabCore

enum Fixtures {
    /// A summary as `traffic_added` and `list_traffic` carry it.
    static let summaryJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000002a",\
        "started_at":{"seconds":1700000000,"nanos":500000000},\
        "response_at":{"seconds":1700000000,"nanos":600000000},\
        "duration_micros":84000,\
        "client":"127.0.0.1:54321",\
        "scheme":"https",\
        "destination":{"host":"api.example.com","port":443},\
        "method":"POST",\
        "url":"https://api.example.com/v1/users?active=true",\
        "version":"HTTP/1.1",\
        "status":201,\
        "reason":"Created",\
        "response_version":"HTTP/1.1",\
        "completed":true,\
        "error":null}
        """

    /// A whole exchange as `get_traffic` carries it.
    static let exchangeJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000002a",\
        "connection":{"client":"127.0.0.1:54321","scheme":"https",\
        "destination":{"host":"api.example.com","port":443}},\
        "request":{"method":"POST","url":"https://api.example.com/v1/users",\
        "version":"HTTP/1.1",\
        "headers":[{"name":"Content-Type","value":{"text":"application/json"}},\
        {"name":"Authorization","value":{"text":"Bearer sk-secret"}},\
        {"name":"X-Raw","value":{"bytes":"/w=="}}],\
        "body":{"data":"eyJuYW1lIjoiYWRhIn0=","size":14,"truncated":false}},\
        "response":{"status":201,"reason":"Created","version":"HTTP/1.1",\
        "headers":[{"name":"Set-Cookie","value":{"text":"session=abc"}},\
        {"name":"Set-Cookie","value":{"text":"tracking=xyz"}}],\
        "body":{"data":"eyJpZCI6MX0=","size":4096,"truncated":true}},\
        "error":null,\
        "timing":{"started_at":{"seconds":1700000000,"nanos":500000000},\
        "upstream_connect_micros":null,"upstream_response_micros":2000,\
        "completed_micros":84000}}
        """

    /// An exchange the origin never answered.
    static let failedExchangeJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000002b",\
        "connection":{"client":"127.0.0.1:54322","scheme":"http","destination":null},\
        "request":{"method":"GET","url":"http://nowhere.invalid/","version":"HTTP/1.1",\
        "headers":[],"body":{"data":"","size":0,"truncated":false}},\
        "response":null,\
        "error":{"kind":"upstream_connect",\
        "summary":"could not connect to the destination server"},\
        "timing":{"started_at":{"seconds":1700000001,"nanos":0},\
        "upstream_connect_micros":100,"upstream_response_micros":null,\
        "completed_micros":null}}
        """

    /// A successful `list_traffic` response carrying one exchange.
    static func listResponse(requestID: String, cursor: Bool = false) -> String {
        let next = cursor
            ? #"{"started_at_ms":1700000000500,"id":"0199c0de-0000-7000-8000-00000000002a"}"#
            : "null"

        return """
            {"version":1,"request_id":"\(requestID)","ok":true,"type":"list_traffic",\
            "result":{"exchanges":[\(summaryJSON)],"next_cursor":\(next),"total":1}}
            """
    }

    /// A successful `get_traffic` response.
    static func getResponse(requestID: String, exchange: String = exchangeJSON) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"get_traffic",\
        "result":{"exchange":\(exchange)}}
        """
    }

    /// A successful `delete_traffic` response.
    static func deleteResponse(requestID: String, id: String, deleted: Bool = true) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"delete_traffic",\
        "result":{"exchange_id":"\(id)","deleted":\(deleted)}}
        """
    }

    /// A successful `clear_traffic` response.
    static func clearResponse(requestID: String, removed: UInt64 = 3) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"clear_traffic",\
        "result":{"removed":\(removed)}}
        """
    }

    /// A successful `subscribe` response.
    static func subscribeResponse(requestID: String) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"subscribe",\
        "result":{"subscribed":true}}
        """
    }

    /// A refusal.
    static func errorResponse(requestID: String?, code: String, message: String) -> String {
        let id = requestID.map { "\"\($0)\"" } ?? "null"

        return """
            {"version":1,"request_id":\(id),"ok":false,\
            "error":{"code":"\(code)","message":"\(message)"}}
            """
    }

    /// A `traffic_added` event.
    static func trafficAdded(_ summary: String = summaryJSON) -> String {
        #"{"version":1,"type":"traffic_added","exchange":\#(summary)}"#
    }

    /// A `traffic_lost` event.
    static func trafficLost(_ missed: UInt64) -> String {
        #"{"version":1,"type":"traffic_lost","missed":\#(missed)}"#
    }

    /// A `stream_message_added` event carrying `message`.
    static func streamMessageAdded(_ message: String) -> String {
        #"{"version":1,"type":"stream_message_added","message":\#(message)}"#
    }

    /// A `stream_messages_lost` event.
    static func streamMessagesLost(_ missed: UInt64) -> String {
        #"{"version":1,"type":"stream_messages_lost","missed":\#(missed)}"#
    }

    /// A stream-message summary as the core sends it.
    static let streamSummaryJSON = """
        {"id":"0199c0de-0000-7000-8000-000000000101",\
        "exchange_id":"0199c0de-0000-7000-8000-00000000002a",\
        "sequence":0,\
        "captured_at":{"seconds":1700000000,"nanos":500000000},\
        "direction":"client_to_server",\
        "kind":"websocket_text",\
        "payload_size":8,\
        "payload_captured_len":8,\
        "payload_truncated":false}
        """

    // MARK: - Models

    /// A summary built here, for tests that need one without decoding.
    static func summary(
        id: String,
        startedAtSeconds: UInt64 = 1_700_000_000,
        nanos: UInt32 = 0,
        method: String = "GET",
        url: String = "https://api.example.com/v1/users",
        status: UInt16? = 200
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: startedAtSeconds, nanos: nanos),
            durationMicros: 1_000,
            client: "127.0.0.1:54321",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: method,
            url: url,
            version: .http11,
            status: status,
            completed: true
        )
    }

    /// An exchange built here, in whatever shape a test needs.
    ///
    /// Everything defaulted to the ordinary case, so a test that is about one
    /// field names that field and nothing else.
    static func exchange(
        id: String,
        method: String = "GET",
        url: String = "https://api.example.com/v1/users",
        requestVersion: HTTPVersion = .http11,
        requestHeaders: Headers = Headers(),
        requestBody: BodyCapture = .empty,
        status: UInt16? = 200,
        reason: String? = "OK",
        responseVersion: HTTPVersion = .http11,
        responseHeaders: Headers = Headers(),
        responseBody: BodyCapture = .empty,
        error: TrafficError? = nil,
        scheme: Scheme = .https,
        timing: Timing = Timing(
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            upstreamResponseMicros: 2_000,
            completedMicros: 84_000
        )
    ) -> TrafficExchange {
        TrafficExchange(
            id: id,
            connection: ConnectionInfo(
                client: "127.0.0.1:54321",
                scheme: scheme,
                destination: Destination(host: "api.example.com", port: 443)
            ),
            request: CapturedRequest(
                method: method,
                url: url,
                version: requestVersion,
                headers: requestHeaders,
                body: requestBody
            ),
            response: status.map {
                CapturedResponse(
                    status: $0,
                    reason: reason,
                    version: responseVersion,
                    headers: responseHeaders,
                    body: responseBody
                )
            },
            error: error,
            timing: timing
        )
    }

    /// An exchange the origin never answered.
    static func failedExchange(
        id: String,
        kind: TrafficErrorKind = .upstreamConnect,
        summary: String = "could not connect to the destination server"
    ) -> TrafficExchange {
        exchange(
            id: id,
            url: "http://nowhere.invalid/",
            status: nil,
            reason: nil,
            error: TrafficError(kind: kind, summary: summary),
            scheme: .http,
            timing: Timing(
                startedAt: Timestamp(seconds: 1_700_000_001, nanos: 0),
                upstreamConnectMicros: 100
            )
        )
    }

    /// A page carrying `exchanges`.
    static func page(_ exchanges: [ExchangeSummary], next: TrafficCursor? = nil) -> TrafficListResult
    {
        TrafficListResult(
            exchanges: exchanges,
            nextCursor: next,
            total: UInt64(exchanges.count)
        )
    }

    /// A WebSocket handshake, as the inspector recognises one.
    static func websocketExchange(id: String) -> TrafficExchange {
        exchange(
            id: id,
            method: "GET",
            url: "http://example.com/socket",
            status: 101,
            reason: "Switching Protocols",
            responseHeaders: Headers([
                Header(name: "Upgrade", value: .text("websocket")),
                Header(name: "Connection", value: .text("Upgrade")),
            ]),
            scheme: .http
        )
    }

    /// An event-stream response, as the inspector recognises one.
    static func sseExchange(id: String) -> TrafficExchange {
        exchange(
            id: id,
            url: "https://api.example.com/events",
            responseHeaders: Headers([
                Header(name: "Content-Type", value: .text("text/event-stream"))
            ])
        )
    }

    /// A stream-message summary built here.
    static func streamSummary(
        id: String,
        exchangeID: String,
        sequence: UInt64 = 0,
        direction: StreamDirection = .clientToServer,
        kind: StreamKind = .websocketText,
        eventName: String? = nil,
        eventID: String? = nil,
        retryMs: UInt64? = nil,
        payloadSize: UInt64 = 8
    ) -> StreamMessageSummary {
        StreamMessageSummary(
            id: id,
            exchangeID: exchangeID,
            sequence: sequence,
            capturedAt: Timestamp(seconds: 1_700_000_000, nanos: 500_000_000),
            direction: direction,
            kind: kind,
            eventName: eventName,
            eventID: eventID,
            retryMs: retryMs,
            payloadSize: payloadSize,
            payloadCapturedLen: payloadSize,
            payloadTruncated: false
        )
    }

    /// A stream message with a payload, as `get_stream_message` carries it.
    static func streamMessage(
        id: String,
        exchangeID: String,
        sequence: UInt64 = 0,
        direction: StreamDirection = .clientToServer,
        kind: StreamKind = .websocketText,
        eventName: String? = nil,
        eventID: String? = nil,
        retryMs: UInt64? = nil,
        payload: BodyCapture
    ) -> StreamMessage {
        StreamMessage(
            id: id,
            exchangeID: exchangeID,
            sequence: sequence,
            capturedAt: Timestamp(seconds: 1_700_000_000, nanos: 500_000_000),
            direction: direction,
            kind: kind,
            eventName: eventName,
            eventID: eventID,
            retryMs: retryMs,
            payload: payload
        )
    }
}
