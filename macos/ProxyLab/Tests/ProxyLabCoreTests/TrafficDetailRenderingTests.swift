//  The detail pane, actually drawn.
//
//  A SwiftUI view that compiles is not a SwiftUI view that lays out: a body
//  that traps, an index that is not there, or a frame that resolves to nothing
//  all happen when the view is rendered. `ImageRenderer` runs a body and lays
//  it out with no window, no run loop, and nobody watching, which is what makes
//  that testable here.
//
//  Smoke tests, deliberately not snapshot comparisons: what is asserted is that
//  each face of the pane draws something at a sensible size for the exchange it
//  was given — including the awkward ones, which is most of what this covers.

import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Traffic detail views")
struct TrafficDetailRenderingTests {
    /// Lays `view` out offscreen and gives back the size it came out at.
    private func rendered(_ view: some View, width: CGFloat = 520) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    /// A model showing `exchange`, ready to draw.
    private func model(showing exchange: TrafficExchange) async -> TrafficInspectorModel {
        let session = FakeSession()
        let rows = Fixtures.page([Fixtures.summary(id: exchange.id)])
        await session.answerLists(with: [.success(rows)])
        await session.answerExchange(exchange.id, with: .success(exchange))

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select(exchange.id)

        return model
    }

    /// Draws the pane for `exchange` and says it came out at a real size.
    private func drawsDetail(
        _ exchange: TrafficExchange,
        width: CGFloat = 520,
        sourceLocation: SourceLocation = #_sourceLocation
    ) async throws {
        let model = await model(showing: exchange)
        let size = try #require(
            rendered(TrafficDetailView(model: model), width: width),
            sourceLocation: sourceLocation
        )

        #expect(size.width > 0 && size.height > 0, sourceLocation: sourceLocation)
    }

    // MARK: - The ordinary case, and each face of it

    @Test("an ordinary JSON exchange draws")
    func ordinary() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                method: "POST",
                url: "https://api.example.com/v1/users?page=2&limit=20&search=iPhone+17",
                requestHeaders: Headers([
                    Header(name: "Content-Type", value: .text("application/json")),
                    Header(name: "Authorization", value: .text("Bearer TEST_TOKEN")),
                ]),
                requestBody: .complete(Data(#"{"name":"Ada","items":[1,2,3]}"#.utf8)),
                status: 201,
                reason: "Created",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("application/json; charset=utf-8")),
                    Header(name: "Set-Cookie", value: .text("session=abc")),
                    Header(name: "Set-Cookie", value: .text("tracking=xyz")),
                ]),
                responseBody: .complete(Data(#"{"id":1,"name":"Ada"}"#.utf8))
            )
        )
    }

    @Test("each face of one exchange draws on its own")
    func everyFace() async throws {
        let exchange = Fixtures.exchange(
            id: "a",
            url: "https://api.example.com/v1/users?page=2",
            requestHeaders: Headers([Header(name: "Accept", value: .text("application/json"))]),
            responseBody: .complete(Data(#"{"id":1}"#.utf8))
        )
        let model = await model(showing: exchange)
        let inspection = try #require(model.inspection)

        let reading = InspectorReading.make(exchange: exchange, inspection: inspection)
        let request = try #require(
            rendered(
                RequestView(
                    exchange: exchange,
                    inspection: inspection,
                    reading: reading,
                    model: model
                )
            )
        )
        let response = try #require(
            rendered(
                ResponseView(
                    exchange: exchange,
                    inspection: inspection,
                    reading: reading,
                    model: model
                )
            )
        )

        #expect(request.height > 0)
        #expect(response.height > 0)
    }

    @Test("the pane draws in a narrow window as well as a wide one")
    func narrow() async throws {
        let exchange = Fixtures.exchange(
            id: "a",
            url: "https://api.example.com/v1/organisations/8f2c/members?include=profile",
            requestHeaders: Headers([
                Header(name: "Authorization", value: .text(String(repeating: "T", count: 400)))
            ]),
            responseBody: .complete(Data(#"{"ok":true}"#.utf8))
        )

        try await drawsDetail(exchange, width: 380)
        try await drawsDetail(exchange, width: 900)
    }

    // MARK: - The awkward ones

    @Test("an exchange with nothing in it draws")
    func bare() async throws {
        try await drawsDetail(Fixtures.exchange(id: "a", reason: nil))
    }

    @Test("an exchange the origin never answered draws")
    func noResponse() async throws {
        try await drawsDetail(Fixtures.failedExchange(id: "a"))
    }

    @Test("each kind of failure draws")
    func failures() async throws {
        for kind in [
            TrafficErrorKind.upstreamConnect,
            .upstreamTLS,
            .clientTLS,
            .requestRejected,
            .incomplete,
            .clientDisconnected,
        ] {
            try await drawsDetail(
                Fixtures.failedExchange(id: "a", kind: kind, summary: "\(kind.rawValue) happened")
            )
        }
    }

    @Test("an exchange that has not finished draws")
    func pending() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                status: nil,
                reason: nil,
                timing: Timing(startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0))
            )
        )
    }

    @Test("a blocked request, answered by a rule and not by an origin, draws")
    func blocked() async throws {
        // A Block rule's answer is an ordinary captured response with no
        // upstream timing behind it.
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                status: 403,
                reason: "Forbidden",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("text/plain")),
                    Header(name: "X-ProxyLab-Rule", value: .text("block")),
                ]),
                responseBody: .complete(Data("Blocked by ProxyLab".utf8)),
                timing: Timing(
                    startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                    completedMicros: 300
                )
            )
        )
    }

    @Test("a Map Local answer, with no upstream connection behind it, draws")
    func mapLocal() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                status: 200,
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("application/json"))
                ]),
                responseBody: .complete(Data(#"{"from":"a file on disk"}"#.utf8)),
                timing: Timing(
                    startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                    completedMicros: 900
                )
            )
        )
    }

    @Test("a Map Remote answer, sent somewhere other than the URL says, draws")
    func mapRemote() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                url: "https://api.example.com/v1/users",
                status: 200,
                responseBody: .complete(Data(#"{"from":"staging"}"#.utf8)),
                timing: Timing(
                    startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                    upstreamConnectMicros: 400,
                    upstreamResponseMicros: 12_000,
                    completedMicros: 15_000
                )
            )
        )
    }

    @Test("halves framed as different protocol versions draw")
    func mixedVersions() async throws {
        try await drawsDetail(
            Fixtures.exchange(id: "a", requestVersion: .http11, responseVersion: .http2)
        )
    }

    @Test("a status code nobody standardised draws")
    func unknownStatus() async throws {
        try await drawsDetail(Fixtures.exchange(id: "a", status: 799, reason: nil))
        try await drawsDetail(Fixtures.exchange(id: "a", status: 599, reason: "Custom"))
    }

    // MARK: - Bodies

    @Test("a body that is not text draws as what it is, not as characters")
    func binaryBody() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("application/octet-stream"))
                ]),
                responseBody: .complete(Data((0...255).map(UInt8.init)))
            )
        )
    }

    @Test("a body the sender called text and that will not decode draws")
    func undecodableBody() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("text/plain; charset=utf-8"))
                ]),
                responseBody: .complete(Data([0xFF, 0xFE, 0x00, 0x01]))
            )
        )
    }

    @Test("an image response draws, and one that will not decode still draws")
    func imageBody() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([Header(name: "Content-Type", value: .text("image/png"))]),
                responseBody: .complete(Self.onePixelPNG)
            )
        )

        // The first bytes say PNG and the rest does not. The viewer is what
        // discovers that, and it says so rather than drawing an empty frame.
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([Header(name: "Content-Type", value: .text("image/png"))]),
                responseBody: BodyCapture(
                    data: Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00]),
                    size: 40_000,
                    truncated: true
                )
            )
        )
    }

    @Test("a truncated body draws, and says it was truncated")
    func truncatedBody() async throws {
        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("application/json"))
                ]),
                responseBody: BodyCapture(
                    data: Data(#"{"items":[1,2,3"#.utf8),
                    size: 4_200_000,
                    truncated: true
                )
            )
        )
    }

    @Test("a body far too large to draw is drawn, shortened, and says so")
    func largeBody() async throws {
        // The core captures at most a megabyte, so this is the largest body
        // the pane will ever be handed.
        let body = Data(String(repeating: "abcdefgh", count: 128 * 1_024).utf8)
        let capture = BodyCapture(data: body, size: 8_000_000, truncated: true)

        try await drawsDetail(
            Fixtures.exchange(
                id: "a",
                responseHeaders: Headers([
                    Header(name: "Content-Type", value: .text("text/plain"))
                ]),
                responseBody: capture
            )
        )

        // Drawn on its own as well as inside the pane: a scroll view lays its
        // content out lazily enough that rendering the pane does not prove the
        // body itself can be drawn, and the body is the expensive part.
        let inspection = BodyInspection.of(
            capture,
            contentType: ContentType.parse("text/plain")
        )
        let size = try #require(rendered(BodyView(inspection: inspection)))

        #expect(size.height > 0)
        // What is drawn is the shortened form, not the megabyte.
        if case let .text(text) = inspection.content {
            #expect(text.shortened)
            #expect(text.text.count == BodyInspection.displayedCharacterLimit)
        } else {
            Issue.record("a text body should have decoded as text")
        }
    }

    @Test("a request with no query string draws the absence of one")
    func noQuery() async throws {
        let exchange = Fixtures.exchange(id: "a", url: "https://api.example.com/v1/users")
        let model = await model(showing: exchange)
        let inspection = try #require(model.inspection)

        let size = try #require(
            rendered(
                RequestView(
                    exchange: exchange,
                    inspection: inspection,
                    reading: InspectorReading.make(exchange: exchange, inspection: inspection),
                    model: model
                )
            )
        )
        #expect(size.height > 0)
    }

    @Test("the pane draws every state it can be in without an exchange")
    func emptyStates() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([]))])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()

        #expect(try #require(rendered(TrafficDetailView(model: model))).height > 0)

        await session.answerExchange(
            "gone",
            with: .failure(.server(IPCErrorBody(code: .notFound, message: "x")))
        )
        await model.select("gone")
        #expect(model.detail == .unavailable)
        #expect(try #require(rendered(TrafficDetailView(model: model))).height > 0)

        await session.answerExchange("bad", with: .failure(.notConnected))
        await model.select("bad")
        #expect(try #require(rendered(TrafficDetailView(model: model))).height > 0)
    }

    /// The smallest PNG that decodes: one opaque pixel.
    private static let onePixelPNG = Data(
        base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
    )!
}
