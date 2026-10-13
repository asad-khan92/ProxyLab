//  A breakpoint started from a captured exchange.
//
//  The draft is host, path, and method. The query stays out, because matching
//  it would wait for a request that will not come again. Request and response
//  are the directions the core already has, and a WebSocket row does not
//  become a text-message breakpoint.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Breakpoint from traffic")
struct TrafficBreakpointDraftTests {
    private func summary(
        method: String = "GET",
        url: String = "https://staging-api.skonzs.io/api/users/notifications/unread-count?since=1&user=9",
        status: UInt16? = 200,
        destination: Destination? = Destination(host: "staging-api.skonzs.io", port: 443),
        contentType: String? = "application/json"
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: "exchange-1",
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .https,
            destination: destination,
            method: method,
            url: url,
            version: .http11,
            status: status,
            responseContentType: contentType,
            completed: true
        )
    }

    @Test("a normal request is prefilled with host, path, and method, and not the query")
    func prefilled() {
        let draft = TrafficBreakpointDraft.from(summary: summary())

        #expect(draft.host == "staging-api.skonzs.io")
        #expect(draft.path == "/api/users/notifications/unread-count")
        #expect(!draft.pathIsPrefix)
        #expect(draft.method == "GET")
        #expect(draft.pauseRequest)
        #expect(draft.pauseResponse)
        #expect(draft.name == "GET staging-api.skonzs.io/api/users/notifications/unread-count")
        #expect(!draft.name.contains("since"))
        #expect(!draft.path.contains("?"))

        #expect(draft.matcher.host == .exact("staging-api.skonzs.io"))
        #expect(draft.matcher.path == .exact("/api/users/notifications/unread-count"))
        #expect(draft.matcher.method == .exact("GET"))
        #expect(draft.directions == [.request, .response])
        #expect(!draft.directions.contains(.websocket))
        #expect(!draft.matcher.summary.contains("since"))
        #expect(!draft.matcher.summary.contains("user=9"))
    }

    @Test("a prefix is opt-in, and the query is still not part of the path")
    func prefixOmitsQuery() {
        var draft = TrafficBreakpointDraft.from(summary: summary())
        draft.pathIsPrefix = true

        #expect(draft.matcher.path == .prefix("/api/users/notifications/unread-count"))
        #expect(draft.matcher.summary.contains("/api/users/notifications/unread-count*"))
        #expect(!draft.matcher.summary.contains("?"))
    }

    @Test("the row menu opens that draft, and does not send it yet")
    @MainActor
    func menuOpensTheDraft() async {
        let summary = summary()
        let actions = TrafficRowMenu(summary: summary, isPinned: false)
        let session = FakeBreakpointsSession()
        let model = BreakpointsModel(session: session)

        #expect(actions.canBreakpoint)
        actions.beginBreakpoint(using: model)

        #expect(model.trafficDraft == TrafficBreakpointDraft.from(summary: summary))
        #expect(await session.calls.isEmpty)
    }

    @Test("a WebSocket handshake still drafts a request and response breakpoint")
    func websocketStaysARequestBreakpoint() {
        let draft = TrafficBreakpointDraft.from(
            summary: summary(
                url: "https://example.com/socket?token=abc",
                status: 101,
                destination: Destination(host: "example.com", port: 443),
                contentType: nil
            )
        )
        let menu = TrafficRowMenu(
            summary: summary(status: 101, contentType: nil),
            isPinned: false
        )

        #expect(menu.canBreakpoint)
        #expect(!menu.canReplay)
        #expect(draft.path == "/socket")
        #expect(!draft.path.contains("token"))
        #expect(draft.directions == [.request, .response])
        #expect(!draft.directions.contains(.websocket))
    }

    @Test("an event stream is still offered as a request breakpoint, not a message pause")
    func eventStreamStaysARequestBreakpoint() {
        let row = summary(
            url: "https://example.com/events?cursor=1",
            contentType: "text/event-stream"
        )
        let draft = TrafficBreakpointDraft.from(summary: row)
        let menu = TrafficRowMenu(summary: row, isPinned: false)

        #expect(menu.canBreakpoint)
        #expect(!menu.canReplay)
        #expect(draft.path == "/events")
        #expect(draft.directions == [.request, .response])
    }
}

@Suite("Paused exchange presentation")
struct PausedHoldLabelTests {
    private func paused(
        direction: BreakpointDirection,
        method: String = "GET",
        url: String = "https://api.example.com/orders?x=1"
    ) -> PausedExchange {
        PausedExchange(
            pauseID: "pause-1",
            breakpointID: "bp-1",
            direction: direction,
            method: method,
            url: url,
            request: direction == .request
                ? PausedRequest(
                    method: method,
                    url: url,
                    headers: Headers(),
                    body: .empty
                )
                : nil,
            response: direction == .response
                ? PausedResponse(status: 200, headers: Headers(), body: .empty)
                : nil
        )
    }

    @Test("a request hold reads Request Paused, and a response hold reads Response Paused")
    func phases() {
        let request = PausedTrafficRowContent(paused(direction: .request, method: "POST"))
        let response = PausedTrafficRowContent(paused(direction: .response))

        #expect(request.title == "Request Paused")
        #expect(request.detail.contains("POST"))
        #expect(request.detail.contains("api.example.com"))
        #expect(request.title != "Pending")
        #expect(response.title == "Response Paused")
        #expect(response.detail.contains("GET"))
        #expect(PausedHoldLabel.phase(.websocket) == "WebSocket Paused")
    }

    @Test("the toolbar count names how many are held")
    func count() {
        #expect(PausedHoldLabel.count(1) == "1 paused")
        #expect(PausedHoldLabel.count(3) == "3 paused")
    }
}
