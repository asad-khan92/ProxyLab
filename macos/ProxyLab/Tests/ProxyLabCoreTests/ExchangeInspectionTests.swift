//  The timing, the versions, and the text an exchange can be copied as.
//
//  The rule these are protecting: nothing is presented that was not measured
//  or captured. There are no DNS, TCP, or TLS phases here because the core
//  does not record them; an exchange that never finished has no total; and a
//  copy carries what the exchange carried, including the parts that are not
//  text — as Base64, never as characters nobody sent.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Exchange timing")
struct ExchangeTimingTests {
    private func timing(
        connect: UInt64? = nil,
        head: UInt64? = nil,
        completed: UInt64? = nil
    ) -> Timing {
        Timing(
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            upstreamConnectMicros: connect,
            upstreamResponseMicros: head,
            completedMicros: completed
        )
    }

    @Test("the marks are the moments the core recorded, in order")
    func marks() {
        let marks = ExchangeTiming.marks(of: timing(connect: 100, head: 2_000, completed: 84_000))

        #expect(marks.map(\.name) == ["Upstream connect", "Response head", "Completed"])
        #expect(marks.map(\.micros) == [100, 2_000, 84_000])
    }

    @Test("an intercepted request has no upstream connect, and none is invented")
    func noConnectMark() {
        // Its connection was opened by the `CONNECT` before it, so there is
        // no measurement to show — and zero would be a measurement.
        let marks = ExchangeTiming.marks(of: timing(head: 2_000, completed: 84_000))

        #expect(marks.map(\.name) == ["Response head", "Completed"])
    }

    @Test("an exchange with nothing measured has no marks at all")
    func noMarks() {
        #expect(ExchangeTiming.marks(of: timing()).isEmpty)
        #expect(ExchangeTiming.total(of: timing()) == nil)
    }

    @Test("the spans are the stretches measured at both ends")
    func spans() {
        let spans = ExchangeTiming.spans(of: timing(head: 2_000, completed: 84_000))

        #expect(spans.map(\.name) == ["To response head", "Response body"])
        #expect(spans.map(\.micros) == [2_000, 82_000])
    }

    @Test("an exchange that never finished has one span, not a guessed second")
    func openSpan() {
        let spans = ExchangeTiming.spans(of: timing(head: 2_000))

        #expect(spans.map(\.name) == ["To response head"])
    }

    @Test("an exchange with no response head has no spans")
    func noSpans() {
        #expect(ExchangeTiming.spans(of: timing(connect: 100)).isEmpty)
        #expect(ExchangeTiming.spans(of: timing()).isEmpty)
    }

    @Test("a completion that predates the head is not a negative span")
    func impossibleOrder() {
        // Clock readings that disagree are not something to subtract into a
        // number that would underflow.
        let spans = ExchangeTiming.spans(of: timing(head: 2_000, completed: 1_000))

        #expect(spans.map(\.name) == ["To response head"])
    }

    @Test("a duration is shown at a scale a person reads")
    func durations() {
        #expect(ExchangeTiming.duration(micros: 500) == "0.50 ms")
        #expect(ExchangeTiming.duration(micros: 84_000) == "84 ms")
        #expect(ExchangeTiming.duration(micros: 1_240_000) == "1.24 s")
    }
}

@MainActor
@Suite("Exchange versions")
struct ExchangeVersionTests {
    @Test("one version is shown once when both halves agree")
    func same() {
        let exchange = Fixtures.exchange(id: "a", requestVersion: .http11, responseVersion: .http11)

        #expect(ExchangeVersions.of(exchange).label == "HTTP/1.1")
    }

    @Test("HTTP/2 is spelled the way everything a developer reads spells it")
    func httpTwo() {
        let exchange = Fixtures.exchange(id: "a", requestVersion: .http2, responseVersion: .http2)

        // The core's own word is `HTTP/2.0`.
        #expect(exchange.request.version.rawValue == "HTTP/2.0")
        #expect(ExchangeVersions.of(exchange).label == "HTTP/2")
    }

    @Test("halves that were framed differently are both shown")
    func mixed() {
        let exchange = Fixtures.exchange(id: "a", requestVersion: .http11, responseVersion: .http2)
        let pair = ExchangeVersions.of(exchange)

        #expect(pair.label == "HTTP/1.1 → HTTP/2")
        #expect(pair.help.contains("HTTP/1.1"))
        #expect(pair.help.contains("HTTP/2"))
    }

    @Test("an exchange with no response shows the version it does have")
    func requestOnly() {
        let exchange = Fixtures.failedExchange(id: "a")

        #expect(ExchangeVersions.of(exchange).label == "HTTP/1.1")
    }

    @Test("a version this build has no name for is said to be unknown")
    func unknown() {
        let version = try? JSONDecoder().decode(HTTPVersion.self, from: Data("\"HTTP/9\"".utf8))

        #expect(version == .unknown)
        #expect(ExchangeVersions.name(.unknown) == "Unknown version")
    }
}

@Suite("Exchange as text")
struct ExchangeTextTests {
    @Test("headers are copied as they were carried, repeats included")
    func headers() {
        let text = ExchangeText.headers(
            Headers([
                Header(name: "Content-Type", value: .text("application/json")),
                Header(name: "Set-Cookie", value: .text("session=abc")),
                Header(name: "Set-Cookie", value: .text("tracking=xyz")),
            ])
        )

        #expect(
            text == """
                Content-Type: application/json
                Set-Cookie: session=abc
                Set-Cookie: tracking=xyz
                """
        )
    }

    @Test("a header value that is not text is described rather than dropped")
    func bytesHeader() {
        let text = ExchangeText.headers(
            Headers([Header(name: "X-Raw", value: .bytes(Data([0xFF, 0x00])))])
        )

        #expect(text == "X-Raw: <2 bytes, not text>")
    }

    @Test("no headers is no text")
    func noHeaders() {
        #expect(ExchangeText.headers(Headers()).isEmpty)
    }

    @Test("query parameters are copied decoded, and a bare name stays bare")
    func queryParameters() {
        let text = ExchangeText.queryParameters(
            QueryParameters.of(url: "https://example.com/p?q=iPhone+17&flag&empty=")
        )

        #expect(
            text == """
                q: iPhone 17
                flag
                empty: 
                """
        )
    }

    @Test("a text body is copied whole, not shortened the way the window shows it")
    func longTextBody() {
        let source = String(repeating: "x", count: BodyInspection.displayedCharacterLimit + 1_000)
        let inspection = BodyInspection.of(.complete(Data(source.utf8)))

        #expect(ExchangeText.body(inspection).count == source.count)
    }

    @Test("a body in another encoding is copied as the characters it decoded to")
    func encodedBody() {
        let inspection = BodyInspection.of(
            .complete(Data([0x43, 0x6F, 0x70, 0x79, 0xA9])),
            contentType: ContentType.parse("text/plain; charset=windows-1252")
        )

        #expect(ExchangeText.body(inspection) == "Copy©")
    }

    @Test("a body that is not text is copied as Base64, never as characters")
    func binaryBody() {
        let bytes = Data([0xFF, 0xFE, 0x00, 0x01])
        let inspection = BodyInspection.of(.complete(bytes))

        #expect(ExchangeText.body(inspection) == bytes.base64EncodedString())
    }

    @Test("a body that was never there copies as nothing")
    func emptyBody() {
        #expect(ExchangeText.body(BodyInspection.of(.empty)).isEmpty)
    }

    @Test("a saved body is named after the URL and the type it was sent as")
    func filenames() {
        #expect(
            ExchangeText.suggestedFilename(
                url: "https://api.example.com/v1/avatar",
                contentType: ContentType.parse("image/png"),
                half: .response
            ) == "avatar.png"
        )
        #expect(
            ExchangeText.suggestedFilename(
                url: "https://api.example.com/v1/users",
                contentType: ContentType.parse("application/json"),
                half: .request
            ) == "users-request.json"
        )
    }

    @Test("a name the URL already spells with an extension keeps that one")
    func existingExtension() {
        #expect(
            ExchangeText.suggestedFilename(
                url: "https://example.com/logo.svg",
                contentType: ContentType.parse("image/svg+xml"),
                half: .response
            ) == "logo.svg"
        )
    }

    @Test("a URL with no path is named after its host")
    func hostFallback() {
        #expect(
            ExchangeText.suggestedFilename(
                url: "https://example.com/",
                contentType: nil,
                half: .response
            ) == "example.com"
        )
    }

    @Test("a media type macOS has no extension for is given no extension")
    func unknownType() {
        // Better a file the person names themselves than one called `.txt`
        // that is not text.
        #expect(
            ExchangeText.suggestedFilename(
                url: "https://example.com/thing",
                contentType: ContentType.parse("application/x-made-up"),
                half: .response
            ) == "thing"
        )
    }
}
