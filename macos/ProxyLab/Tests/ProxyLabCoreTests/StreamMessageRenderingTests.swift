//  The Messages face, actually drawn.
//
//  Smoke tests: a WebSocket handshake and an event stream both have a
//  timeline that lays out at a real size, including the selected payload.

import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Stream message views")
struct StreamMessageRenderingTests {
    private func rendered(_ view: some View, width: CGFloat = 520) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    private func model(
        showing exchange: TrafficExchange,
        messages: [StreamMessageSummary] = [],
        payload: StreamMessage? = nil
    ) async -> TrafficInspectorModel {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: exchange.id)]))])
        await session.answerExchange(exchange.id, with: .success(exchange))
        await session.answerStreamList(
            exchange.id,
            with: StreamMessageListResult(messages: messages)
        )
        if let payload {
            await session.answerStreamMessage(payload.id, with: .success(payload))
        }

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

    @Test("a WebSocket handshake with a JSON text message draws")
    func websocketJSON() async throws {
        let exchange = Fixtures.websocketExchange(id: "a")
        let summary = Fixtures.streamSummary(id: "m1", exchangeID: "a")
        let payload = Fixtures.streamMessage(
            id: "m1",
            exchangeID: "a",
            payload: .complete(Data(#"{"id":1}"#.utf8))
        )
        let model = await model(showing: exchange, messages: [summary], payload: payload)
        let size = try #require(rendered(StreamMessagesView(exchange: exchange, model: model)))

        #expect(size.width > 0 && size.height > 0)
        #expect(model.streamInspection?.content != .binary)
    }

    @Test("a binary WebSocket message draws as bytes")
    func websocketBinary() async throws {
        let exchange = Fixtures.websocketExchange(id: "a")
        let summary = Fixtures.streamSummary(
            id: "m1",
            exchangeID: "a",
            kind: .websocketBinary,
            payloadSize: 3
        )
        let payload = Fixtures.streamMessage(
            id: "m1",
            exchangeID: "a",
            kind: .websocketBinary,
            payload: .complete(Data([0x00, 0x01, 0xFF]))
        )
        let model = await model(showing: exchange, messages: [summary], payload: payload)
        let size = try #require(rendered(StreamMessagesView(exchange: exchange, model: model)))

        #expect(size.height > 0)
        #expect(model.streamInspection?.content == .binary)
    }

    @Test("an SSE event draws its fields")
    func sseEvent() async throws {
        let exchange = Fixtures.sseExchange(id: "a")
        let summary = Fixtures.streamSummary(
            id: "e1",
            exchangeID: "a",
            direction: .serverToClient,
            kind: .sse,
            eventName: "tick",
            eventID: "42",
            retryMs: 3_000,
            payloadSize: 4
        )
        let payload = Fixtures.streamMessage(
            id: "e1",
            exchangeID: "a",
            direction: .serverToClient,
            kind: .sse,
            eventName: "tick",
            eventID: "42",
            retryMs: 3_000,
            payload: .complete(Data("pong".utf8))
        )
        let model = await model(showing: exchange, messages: [summary], payload: payload)
        let size = try #require(rendered(StreamMessagesView(exchange: exchange, model: model)))

        #expect(size.height > 0)
        #expect(model.streamMessages.first?.eventName == "tick")
        #expect(model.streamMessages.first?.eventID == "42")
        #expect(model.streamMessages.first?.retryMs == 3_000)
    }

    @Test("the whole detail pane draws a WebSocket handshake")
    func websocketDetailPane() async throws {
        let exchange = Fixtures.websocketExchange(id: "a")
        let model = await model(showing: exchange)
        let size = try #require(rendered(TrafficDetailView(model: model)))

        #expect(size.width > 0 && size.height > 0)
    }
}
