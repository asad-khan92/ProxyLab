//  What the detail pane is handed, and what its actions do.
//
//  The rule these are protecting: the exchange the pane is showing and the
//  reading of its bodies are always about the same request, and they arrive
//  together. Traffic that keeps arriving while an exchange is open does not
//  disturb either; a delete, a clear, or a filter that hides the selection
//  takes both away.
//
//  Nothing here writes a file or touches a clipboard. Both are protocols, and
//  both are stood in for.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Traffic detail")
struct TrafficDetailModelTests {
    /// A model already showing `exchange`, with a clipboard and a save
    /// destination a test can read back.
    private func model(
        showing exchange: TrafficExchange,
        alsoListing others: [ExchangeSummary] = []
    ) async -> (TrafficInspectorModel, FakeClipboard, FakeBodyDestination) {
        let session = FakeSession()
        let rows = [Fixtures.summary(id: exchange.id)] + others
        await session.answerLists(with: [.success(Fixtures.page(rows))])
        await session.answerExchange(exchange.id, with: .success(exchange))

        let clipboard = FakeClipboard()
        let destination = FakeBodyDestination()
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: clipboard,
            reconnect: .manual
        )
        model.destination = destination

        await model.start()
        await model.select(exchange.id)

        return (model, clipboard, destination)
    }

    private func jsonExchange(id: String = "a") -> TrafficExchange {
        Fixtures.exchange(
            id: id,
            method: "POST",
            url: "https://api.example.com/v1/users?page=2&tag=a&tag=b",
            requestHeaders: Headers([
                Header(name: "Content-Type", value: .text("application/json")),
                Header(name: "Authorization", value: .text("Bearer TEST_TOKEN")),
            ]),
            requestBody: .complete(Data(#"{"name":"Ada"}"#.utf8)),
            status: 201,
            reason: "Created",
            responseHeaders: Headers([
                Header(name: "Content-Type", value: .text("application/json; charset=utf-8"))
            ]),
            responseBody: .complete(Data(#"{"id":1}"#.utf8))
        )
    }

    // MARK: - The bodies arrive with the exchange

    @Test("an exchange and the reading of its bodies arrive together")
    func inspectionArrivesWithDetail() async throws {
        let (model, _, _) = await model(showing: jsonExchange())
        let inspection = try #require(model.inspection)

        #expect(model.detail == .loaded(jsonExchange()))
        #expect(inspection.exchangeID == "a")
        #expect(inspection.request.contentType?.essence == "application/json")
        #expect(inspection.response?.contentType?.charset == "utf-8")
    }

    @Test("an exchange with no response has no response body to show")
    func failedExchange() async throws {
        let (model, _, _) = await model(showing: Fixtures.failedExchange(id: "a"))
        let inspection = try #require(model.inspection)

        #expect(inspection.response == nil)
        #expect(model.detail == .loaded(Fixtures.failedExchange(id: "a")))
    }

    @Test("selecting nothing leaves no bodies behind")
    func deselect() async {
        let (model, _, _) = await model(showing: jsonExchange())
        await model.select(nil)

        #expect(model.detail == .none)
        #expect(model.inspection == nil)
    }

    @Test("deleting the selected exchange takes its bodies with it")
    func delete() async {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(jsonExchange()))
        await session.answerDelete(with: .success(DeletedResult(exchangeID: "a", deleted: true)))

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")
        #expect(model.inspection != nil)

        await model.deleteSelected()

        #expect(model.detail == .none)
        #expect(model.inspection == nil)
    }

    @Test("clearing the traffic clears what was being inspected")
    func clear() async {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(jsonExchange()))

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")
        await model.clearAll()

        #expect(model.detail == .none)
        #expect(model.inspection == nil)
    }

    @Test("traffic arriving does not disturb the exchange being inspected")
    func liveTraffic() async throws {
        let (model, _, _) = await model(showing: jsonExchange())
        let before = try #require(model.inspection)

        for index in 0..<5 {
            await model.handle(
                .event(
                    .trafficAdded(
                        Fixtures.summary(id: "new-\(index)", startedAtSeconds: 1_700_000_100)
                    )
                )
            )
        }

        #expect(model.selectedID == "a")
        #expect(model.inspection == before)
        #expect(model.detail == .loaded(jsonExchange()))
        #expect(model.exchanges.count == 6)
    }

    @Test("an exchange that is gone leaves no stale bodies on screen")
    func goneExchange() async {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange(
            "a",
            with: .failure(.server(IPCErrorBody(code: .notFound, message: "gone")))
        )

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")

        #expect(model.detail == .unavailable)
        #expect(model.inspection == nil)
    }

    @Test("a pinned host stays the filter while an exchange is inspected")
    func pinnedHost() async {
        let (model, _, _) = await model(showing: jsonExchange())
        await model.setHost("api.example.com")

        #expect(model.filters.host == "api.example.com")
        // The row is still listed under that host, so the selection survives.
        #expect(model.selectedID == "a")
        #expect(model.inspection?.exchangeID == "a")
    }

    // MARK: - Copying

    @Test("a copy puts the text on the clipboard and says so")
    func copyText() async {
        let (model, clipboard, _) = await model(showing: jsonExchange())
        model.copy("https://api.example.com/v1/users", as: "the URL")

        #expect(clipboard.last == "https://api.example.com/v1/users")
        #expect(model.lastAction == "Copied the URL.")
        #expect(model.lastError == nil)
    }

    @Test("the headers copied are the ones the message carried")
    func copyHeaders() async throws {
        let (model, clipboard, _) = await model(showing: jsonExchange())
        let exchange = jsonExchange()
        model.copy(ExchangeText.headers(exchange.request.headers), as: "the request headers")

        let copied = try #require(clipboard.last)
        #expect(copied.contains("Content-Type: application/json"))
        // Credentials are shown and copied, because that is what the tool is
        // for. They are never written to a log.
        #expect(copied.contains("Authorization: Bearer TEST_TOKEN"))
    }

    @Test("the body copied is the captured body, not the formatted one")
    func copyBody() async throws {
        let (model, clipboard, _) = await model(showing: jsonExchange())
        let inspection = try #require(model.inspection)
        model.copy(ExchangeText.body(inspection.request), as: "the request body")

        #expect(clipboard.last == #"{"name":"Ada"}"#)
    }

    // MARK: - Saving

    @Test("saving a body writes the captured bytes under a name from the URL")
    func save() async throws {
        let (model, _, destination) = await model(showing: jsonExchange())
        let inspection = try #require(model.inspection?.response)

        model.saveBody(inspection, url: jsonExchange().request.url, half: .response)

        #expect(destination.saves.count == 1)
        #expect(destination.saves[0].data == Data(#"{"id":1}"#.utf8))
        #expect(destination.saves[0].name == "users.json")
        #expect(model.lastAction == "Saved to /tmp/saved.bin.")
    }

    @Test("a truncated body says what was saved was not all of it")
    func saveTruncated() async throws {
        let exchange = Fixtures.exchange(
            id: "a",
            responseBody: BodyCapture(
                data: Data(repeating: 0x41, count: 64),
                size: 4_200_000,
                truncated: true
            )
        )
        let (model, _, _) = await model(showing: exchange)
        let inspection = try #require(model.inspection?.response)

        model.saveBody(inspection, url: exchange.request.url, half: .response)

        #expect(model.lastAction?.contains("truncated") == true)
    }

    @Test("a cancelled save writes nothing and is not a failure")
    func saveCancelled() async throws {
        let (model, _, destination) = await model(showing: jsonExchange())
        destination.answer = .success(nil)
        let inspection = try #require(model.inspection?.request)

        model.saveBody(inspection, url: jsonExchange().request.url, half: .request)

        #expect(model.lastError == nil)
        #expect(model.lastAction == nil)
    }

    @Test("a save that failed says why, without naming what was in the body")
    func saveFailed() async throws {
        let (model, _, destination) = await model(showing: jsonExchange())
        destination.answer = .failure(FakeWriteFailure())
        let inspection = try #require(model.inspection?.request)

        model.saveBody(inspection, url: jsonExchange().request.url, half: .request)

        #expect(model.lastError?.contains("the disk said no") == true)
        #expect(model.lastError?.contains("Ada") == false)
    }

    // MARK: - Stream messages

    @Test("a WebSocket handshake loads its message timeline with the exchange")
    func streamTimelineLoads() async throws {
        let session = FakeSession()
        let exchange = Fixtures.websocketExchange(id: "a")
        let summary = Fixtures.streamSummary(id: "m1", exchangeID: "a")
        let message = Fixtures.streamMessage(
            id: "m1",
            exchangeID: "a",
            payload: .complete(Data(#"{"id":1}"#.utf8))
        )
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(exchange))
        await session.answerStreamList(
            "a",
            with: StreamMessageListResult(messages: [summary])
        )
        await session.answerStreamMessage("m1", with: .success(message))

        let clipboard = FakeClipboard()
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: clipboard,
            reconnect: .manual
        )
        await model.start()
        await model.select("a")

        #expect(model.streamMessages.map(\.id) == ["m1"])
        #expect(model.selectedStreamID == "m1")
        #expect(model.streamDetail == .loaded(message))
        #expect(model.streamInspection != nil)

        model.copySelectedStreamPayload()

        #expect(clipboard.last == #"{"id":1}"#)
        #expect(model.lastAction == "Copied the payload.")
    }

    @Test("a live stream message lands on the selected exchange's timeline")
    func liveStreamMessage() async throws {
        let session = FakeSession()
        let exchange = Fixtures.sseExchange(id: "a")
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(exchange))

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")
        #expect(model.streamMessages.isEmpty)

        let incoming = Fixtures.streamSummary(
            id: "e1",
            exchangeID: "a",
            direction: .serverToClient,
            kind: .sse,
            eventName: "tick",
            eventID: "42",
            retryMs: 3_000,
            payloadSize: 4
        )
        await model.handle(.event(.streamMessageAdded(incoming)))

        #expect(model.streamMessages == [incoming])
        #expect(model.selectedStreamID == nil)
    }

    @Test("a stream message for another exchange is ignored")
    func streamMessageForOtherExchange() async {
        let (model, _, _) = await model(showing: jsonExchange())

        await model.handle(
            .event(
                .streamMessageAdded(
                    Fixtures.streamSummary(id: "m1", exchangeID: "other")
                )
            )
        )

        #expect(model.streamMessages.isEmpty)
    }

    @Test("deselecting an exchange takes its stream timeline with it")
    func deselectClearsStreams() async {
        let session = FakeSession()
        let exchange = Fixtures.websocketExchange(id: "a")
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(exchange))
        await session.answerStreamList(
            "a",
            with: StreamMessageListResult(
                messages: [Fixtures.streamSummary(id: "m1", exchangeID: "a")]
            )
        )
        await session.answerStreamMessage(
            "m1",
            with: .success(
                Fixtures.streamMessage(
                    id: "m1",
                    exchangeID: "a",
                    payload: .complete(Data("hi".utf8))
                )
            )
        )

        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")
        #expect(!model.streamMessages.isEmpty)

        await model.select(nil)

        #expect(model.streamMessages.isEmpty)
        #expect(model.selectedStreamID == nil)
        #expect(model.streamDetail == .none)
        #expect(model.streamInspection == nil)
    }
}
