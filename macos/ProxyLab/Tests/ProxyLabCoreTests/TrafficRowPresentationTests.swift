//  What a traffic row decides to show, before a view draws it.
//
//  Method, status, duration, and the marks for a failure, HTTP/2, a
//  WebSocket handshake, and an event stream are assertions about that
//  decision. So is which of those survive a narrow column. Nothing here
//  opens a window.

import AppKit
import SwiftUI
import Testing

@testable import ProxyLabCore

@Suite("Traffic row presentation")
struct TrafficRowPresentationTests {
    @Test("the method is the one that was captured")
    func method() {
        #expect(row(method: "GET").method == "GET")
        #expect(row(method: "PATCH").method == "PATCH")
        #expect(row(method: " get ").method == "get")
        #expect(row(method: " ").method == "—")
    }

    @Test("a status code keeps its class, and a missing one does not invent one")
    func statusTones() {
        #expect(row(status: 204).status.text == "204")
        #expect(row(status: 204).status.tone == .success)
        #expect(row(status: 302).status.tone == .redirect)
        #expect(row(status: 404).status.tone == .clientError)
        #expect(row(status: 503).status.tone == .serverError)
        #expect(row(status: 101).status.tone == .informational)
        #expect(row(status: 99).status.tone == .other)
        #expect(row(status: nil).status.text == "—")
        #expect(row(status: nil).status.tone == .pending)
    }

    @Test("a failed exchange is a short word, not the error text")
    func failed() {
        let cases: [(TrafficErrorKind, String)] = [
            (.requestRejected, "Blocked"),
            (.proxyFailure, "Failed"),
            (.upstreamConnect, "Connect"),
            (.upstreamTLS, "TLS"),
            (.clientTLS, "TLS"),
            (.upstreamHTTP, "HTTP"),
            (.responseForwarding, "Forward"),
            (.clientDisconnected, "Closed"),
            (.incomplete, "Partial"),
            (TrafficErrorKind(rawValue: "something_new"), "Failed"),
        ]

        for (kind, label) in cases {
            let presented = row(
                status: nil,
                error: TrafficError(kind: kind, summary: "secret upstream detail")
            )

            #expect(presented.status.text == label)
            #expect(presented.status.tone == .failed)
            #expect(presented.indicators.contains { $0.role == .failure } == false)
            #expect(!presented.accessibilityLabel.contains("secret"))
            #expect(!presented.status.help.contains("secret"))
        }
    }

    @Test("the two TLS failures share a word and not an explanation")
    func tlsFailures() {
        let origin = row(
            status: nil,
            error: TrafficError(kind: .upstreamTLS, summary: "certificate expired")
        )
        let client = row(
            status: nil,
            error: TrafficError(kind: .clientTLS, summary: "client certificate expired")
        )

        #expect(origin.status.text == "TLS")
        #expect(client.status.text == "TLS")
        #expect(origin.status.help != client.status.help)
        #expect(!origin.accessibilityLabel.contains("certificate"))
    }

    @Test("a status code with a failure stays a code, and names the failure beside it")
    func failedWithStatus() {
        let presented = row(
            status: 200,
            error: TrafficError(kind: .responseForwarding, summary: "client closed the socket")
        )

        #expect(presented.status.text == "200")
        #expect(presented.status.tone == .failed)
        #expect(presented.indicators.contains { $0.role == .failure && $0.label == "Forward" })
        #expect(presented.accessibilityLabel.contains("Forward"))
        #expect(!presented.accessibilityLabel.contains("socket"))
    }

    @Test("JSON, HTML, and images are short labels, and a vendor type is left off")
    func contentKinds() {
        #expect(kind("application/json; charset=utf-8") == "JSON")
        #expect(kind("application/problem+json") == "JSON")
        #expect(kind("text/html") == "HTML")
        #expect(kind("application/xhtml+xml") == "HTML")
        #expect(kind("image/png") == "Image")
        #expect(kind("image/svg+xml") == "Image")
        #expect(kind("application/vnd.custom-thing") == nil)
    }

    @Test("HTTP/2, a WebSocket handshake, and an event stream are marked")
    func streamAndProtocol() {
        let http2 = row(version: .http2, responseVersion: .http2, width: .regular)
        #expect(http2.indicators.contains { $0.role == .version && $0.label == "HTTP/2" })

        let mixed = row(version: .http11, responseVersion: .http2, width: .regular)
        #expect(mixed.indicators.contains { $0.label == "1.1→2" })

        let socket = row(status: 101, width: .narrow)
        #expect(socket.indicators.contains { $0.role == .websocket && $0.label == "WS" })
        #expect(socket.indicators.contains { $0.role == .version } == false)
        #expect(socket.accessibilityLabel.contains("WebSocket"))

        let events = row(
            responseContentType: "text/event-stream; charset=utf-8",
            width: .narrow
        )
        #expect(events.indicators.contains { $0.role == .sse && $0.label == "SSE" })
        #expect(events.indicators.contains { $0.role == .content } == false)
        #expect(events.accessibilityLabel.contains("Server-sent events"))
    }

    @Test("a narrow column keeps the scan and drops the quieter facts")
    func narrowAndWide() {
        let summary = make(
            version: .http11,
            responseVersion: .http11,
            responseContentType: "application/json",
            durationMicros: 84_000
        )
        let narrow = TrafficRowPresentation(summary: summary, width: .narrow, domainSelected: false)
        let regular = TrafficRowPresentation(summary: summary, width: .regular, domainSelected: false)
        let wide = TrafficRowPresentation(summary: summary, width: .wide, domainSelected: false)

        #expect(narrow.method == "GET")
        #expect(narrow.status.text == "200")
        #expect(narrow.path == "/v1/users?active=true")
        #expect(narrow.duration == "84 ms")
        #expect(narrow.duration == TrafficDisplay.duration(milliseconds: 84))
        #expect(narrow.indicators.isEmpty)
        #expect(narrow.time == nil)
        #expect(narrow.durationFollowsPath == false)
        #expect(narrow.pathAlignsWithHost == false)

        let regularLabels = regular.indicators.map(\.label)
        let regularRoles = regular.indicators.map(\.role)
        #expect(regularLabels.contains("JSON"))
        #expect(regularRoles.contains(.version) == false)
        #expect(regular.time == nil)
        #expect(regular.durationFollowsPath)

        let wideLabels = wide.indicators.map(\.label)
        #expect(wideLabels.contains("JSON"))
        #expect(wideLabels.contains("HTTP/1.1"))
        #expect(wide.time == TrafficDisplay.time(summary.startedAt))
        #expect(wide.time?.contains("-") == false)
        #expect(wide.durationFollowsPath)
        #expect(wide.pathAlignsWithHost)
    }

    @Test("WebSocket and SSE stay visible when the column is narrow")
    func streamsStayWhenNarrow() {
        let socket = row(status: 101, width: .narrow)
        let events = row(responseContentType: "text/event-stream", width: .narrow)

        #expect(socket.indicators.contains { $0.label == "WS" })
        #expect(events.indicators.contains { $0.label == "SSE" })
    }

    @Test("a selected domain quiets the host and does not remove it")
    func selectedDomain() {
        let shown = row(domainSelected: false)
        let quiet = row(domainSelected: true)

        #expect(shown.host == "api.example.com")
        #expect(!shown.hostIsQuiet)
        #expect(quiet.host == "api.example.com")
        #expect(quiet.hostIsQuiet)
        #expect(quiet.path == "/v1/users?active=true")
    }

    @Test("an unfinished exchange has no duration to invent")
    func pendingDuration() {
        let presented = row(status: nil, durationMicros: nil)

        #expect(presented.duration == nil)
        #expect(presented.status.tone == .pending)
    }

    @Test("the list column's own widths land on narrow, regular, and wide")
    func widths() {
        #expect(TrafficRowWidth.fitting(280) == .narrow)
        #expect(TrafficRowWidth.fitting(319) == .narrow)
        #expect(TrafficRowWidth.fitting(320) == .regular)
        #expect(TrafficRowWidth.fitting(340) == .regular)
        #expect(TrafficRowWidth.fitting(459) == .regular)
        #expect(TrafficRowWidth.fitting(460) == .wide)
        #expect(TrafficRowWidth.fitting(520) == .wide)
    }

    private func kind(_ type: String?) -> String? {
        row(responseContentType: type, width: .regular).indicators.first { $0.role == .content }?
            .label
    }

    private func row(
        method: String = "GET",
        url: String = "https://api.example.com/v1/users?active=true",
        status: UInt16? = 200,
        version: HTTPVersion = .http11,
        responseVersion: HTTPVersion? = .http11,
        requestContentType: String? = nil,
        responseContentType: String? = nil,
        error: TrafficError? = nil,
        durationMicros: UInt64? = 84_000,
        width: TrafficRowWidth = .wide,
        domainSelected: Bool = false
    ) -> TrafficRowPresentation {
        TrafficRowPresentation(
            summary: make(
                method: method,
                url: url,
                status: status,
                version: version,
                responseVersion: responseVersion,
                requestContentType: requestContentType,
                responseContentType: responseContentType,
                error: error,
                durationMicros: durationMicros
            ),
            width: width,
            domainSelected: domainSelected
        )
    }

    private func make(
        method: String = "GET",
        url: String = "https://api.example.com/v1/users?active=true",
        status: UInt16? = 200,
        version: HTTPVersion = .http11,
        responseVersion: HTTPVersion? = .http11,
        requestContentType: String? = nil,
        responseContentType: String? = nil,
        error: TrafficError? = nil,
        durationMicros: UInt64? = 84_000
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: "row",
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 120_000_000),
            durationMicros: durationMicros,
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: method,
            url: url,
            version: version,
            status: status,
            responseVersion: responseVersion,
            requestContentType: requestContentType,
            responseContentType: responseContentType,
            completed: durationMicros != nil,
            error: error
        )
    }
}

@MainActor
@Suite("Traffic row, drawn")
struct TrafficRowRenderingTests {
    @Test("a row draws narrow and wide, including a failure and a stream")
    func draws() {
        let ordinary = Fixtures.summary(id: "a")
        let failed = ExchangeSummary(
            id: "b",
            startedAt: Timestamp(seconds: 1, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: "POST",
            url: "https://api.example.com/v1/users",
            version: .http2,
            status: nil,
            responseVersion: .http2,
            completed: false,
            error: TrafficError(kind: .upstreamConnect, summary: "connection refused")
        )
        let socket = ExchangeSummary(
            id: "c",
            startedAt: Timestamp(seconds: 1, nanos: 0),
            durationMicros: 40_000,
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: "GET",
            url: "https://api.example.com/chat",
            version: .http11,
            status: 101,
            responseVersion: .http11,
            completed: true
        )

        for (summary, width, points) in [
            (ordinary, TrafficRowWidth.narrow, 280.0),
            (failed, TrafficRowWidth.wide, 520.0),
            (socket, TrafficRowWidth.regular, 360.0),
        ] {
            let view = TrafficRow(summary: summary, width: width, domainSelected: width == .narrow)
                .frame(width: points)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 1

            #expect(renderer.nsImage != nil)
            #expect((renderer.nsImage?.size.height ?? 0) > 0)
        }
    }
}
