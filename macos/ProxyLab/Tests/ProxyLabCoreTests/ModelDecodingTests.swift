//  The models against the JSON the core writes.
//
//  These are the tests that fail when `proxy-storage` changes shape, which is
//  the point of writing the fixtures out by hand: the IPC protocol carries the
//  traffic model's own serialized form, so a field renamed in Rust has to show
//  up as a failure here rather than as a blank row in the interface.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Model decoding")
struct ModelDecodingTests {
    private let decoder = JSONDecoder()

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try decoder.decode(type, from: Data(json.utf8))
    }

    @Test("a traffic summary decodes every column the list shows")
    func summary() throws {
        let summary = try decode(ExchangeSummary.self, Fixtures.summaryJSON)

        #expect(summary.id == "0199c0de-0000-7000-8000-00000000002a")
        #expect(summary.method == "POST")
        #expect(summary.url == "https://api.example.com/v1/users?active=true")
        #expect(summary.status == 201)
        #expect(summary.reason == "Created")
        #expect(summary.scheme == .https)
        #expect(summary.destination == Destination(host: "api.example.com", port: 443))
        #expect(summary.client == "127.0.0.1:54321")
        #expect(summary.version == .http11)
        #expect(summary.responseVersion == .http11)
        #expect(summary.completed)
        #expect(summary.error == nil)
        #expect(summary.durationMicros == 84_000)
        #expect(summary.durationMilliseconds == 84)
        #expect(summary.startedAt == Timestamp(seconds: 1_700_000_000, nanos: 500_000_000))
        #expect(summary.responseAt?.nanos == 600_000_000)
        #expect(summary.succeeded)
    }

    @Test("a whole exchange decodes with its headers, bodies, and timing")
    func exchange() throws {
        let exchange = try decode(TrafficExchange.self, Fixtures.exchangeJSON)

        #expect(exchange.request.method == "POST")
        #expect(exchange.request.version == .http11)
        #expect(exchange.request.headers.count == 3)
        #expect(exchange.response?.status == 201)
        #expect(exchange.response?.reason == "Created")
        #expect(exchange.error == nil)
        #expect(exchange.timing.upstreamConnectMicros == nil)
        #expect(exchange.timing.upstreamResponseMicros == 2_000)
        #expect(exchange.timing.durationMilliseconds == 84)
    }

    @Test("headers keep their order and their repetitions")
    func headerOrder() throws {
        let exchange = try decode(TrafficExchange.self, Fixtures.exchangeJSON)
        let response = try #require(exchange.response)

        #expect(response.headers.count == 2)
        #expect(response.headers.entries.map(\.name) == ["Set-Cookie", "Set-Cookie"])
        #expect(response.headers.entries[0].value == .text("session=abc"))
        #expect(response.headers.entries[1].value == .text("tracking=xyz"))
        // The first of a repeated name, matched without regard to case.
        #expect(response.headers.text("set-cookie") == "session=abc")
    }

    @Test("a header value that is not text decodes as the bytes it arrived as")
    func binaryHeaderValue() throws {
        let exchange = try decode(TrafficExchange.self, Fixtures.exchangeJSON)
        let raw = try #require(exchange.request.headers.entries.last)

        #expect(raw.name == "X-Raw")
        #expect(raw.value == .bytes(Data([0xFF])))
        #expect(raw.value.text == nil)
        #expect(raw.value.byteCount == 1)
    }

    @Test("a captured body decodes from base64, with its real size")
    func body() throws {
        let exchange = try decode(TrafficExchange.self, Fixtures.exchangeJSON)

        #expect(exchange.request.body.data == Data(#"{"name":"ada"}"#.utf8))
        #expect(exchange.request.body.size == 14)
        #expect(!exchange.request.body.truncated)

        let response = try #require(exchange.response)
        #expect(response.body.data == Data(#"{"id":1}"#.utf8))
        // The size is the whole body; the capture is what was kept of it.
        #expect(response.body.size == 4_096)
        #expect(response.body.capturedByteCount == 8)
        #expect(response.body.truncated)
    }

    @Test("an exchange with no response carries the reason there is none")
    func failedExchange() throws {
        let exchange = try decode(TrafficExchange.self, Fixtures.failedExchangeJSON)

        #expect(exchange.response == nil)
        #expect(exchange.error?.kind == .upstreamConnect)
        #expect(exchange.error?.summary == "could not connect to the destination server")
        #expect(exchange.connection.destination == nil)
        #expect(exchange.connection.scheme == .http)
        #expect(exchange.timing.durationMilliseconds == nil)
    }

    @Test("an HTTP version this build has no name for decodes as unknown")
    func unknownVersion() throws {
        let json = Fixtures.summaryJSON.replacingOccurrences(
            of: #""version":"HTTP/1.1""#,
            with: #""version":"HTTP/9.9""#
        )

        #expect(try decode(ExchangeSummary.self, json).version == .unknown)
    }

    @Test("a failure kind this build has no name for keeps its name")
    func unknownErrorKind() throws {
        let json = Fixtures.failedExchangeJSON.replacingOccurrences(
            of: #""kind":"upstream_connect""#,
            with: #""kind":"something_new""#
        )
        let exchange = try decode(TrafficExchange.self, json)

        #expect(exchange.error?.kind == TrafficErrorKind(rawValue: "something_new"))
        #expect(exchange.error?.summary == "could not connect to the destination server")
    }

    @Test("a timestamp is the moment the core recorded")
    func timestamp() {
        let timestamp = Timestamp(seconds: 1_700_000_000, nanos: 500_000_000)

        #expect(timestamp.milliseconds == 1_700_000_000_500)
        #expect(timestamp.date.timeIntervalSince1970 == 1_700_000_000.5)
    }
}
