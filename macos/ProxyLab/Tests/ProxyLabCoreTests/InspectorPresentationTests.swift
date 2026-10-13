//  What the detail pane says about an exchange, decided before a view draws it.
//
//  The summary, the two overviews, the headers, the query, the cookies, the
//  masked authorization, a failure, and which sections exist. Drawing is a
//  separate suite. Nothing here prints a header, a body, or a URL.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Traffic inspector presentation")
struct InspectorPresentationTests {
    private func reading(
        method: String = "GET",
        url: String = "https://api.example.com/v1/customer/category?category_id=abc-123&tag=a&tag=b",
        requestVersion: HTTPVersion = .http2,
        requestHeaders: Headers = Headers([
            Header(name: "Accept", value: .text("application/json")),
            Header(name: "Authorization", value: .text("Bearer super-secret-token")),
            Header(name: "Cookie", value: .text("session_id=abc; locale=en")),
        ]),
        requestBody: BodyCapture = .complete(Data(#"{"n":1}"#.utf8)),
        status: UInt16? = 200,
        reason: String? = "OK",
        responseVersion: HTTPVersion = .http2,
        responseHeaders: Headers = Headers([
            Header(name: "Content-Type", value: .text("application/json; charset=utf-8")),
            Header(name: "Content-Encoding", value: .text("gzip")),
            Header(name: "Set-Cookie", value: .text("session_id=abc; Path=/; Secure; HttpOnly")),
            Header(name: "Set-Cookie", value: .text("locale=en; Expires=Wed, 21 Oct 2015 07:28:00 GMT")),
        ]),
        responseBody: BodyCapture = .complete(Data("hi".utf8)),
        error: TrafficError? = nil,
        timing: Timing = Timing(
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 882_000_000),
            upstreamResponseMicros: 20_000,
            completedMicros: 84_000
        )
    ) -> (TrafficExchange, InspectorReading) {
        let exchange = Fixtures.exchange(
            id: "a",
            method: method,
            url: url,
            requestVersion: requestVersion,
            requestHeaders: requestHeaders,
            requestBody: requestBody,
            status: status,
            reason: reason,
            responseVersion: responseVersion,
            responseHeaders: responseHeaders,
            responseBody: responseBody,
            error: error,
            timing: timing
        )

        return (exchange, InspectorReading.make(exchange: exchange, inspection: BodyInspection.of(exchange)))
    }

    private func fact(_ facts: [OverviewFact], _ name: String) -> OverviewFact? {
        facts.first { $0.name == name }
    }

    // MARK: - Request overview

    @Test("the request overview is the useful facts, and not the storage id")
    func requestOverview() {
        let (exchange, prepared) = reading()
        let facts = prepared.requestFacts

        #expect(fact(facts, "Method")?.value == "GET")
        #expect(fact(facts, "URL")?.value == exchange.request.url)
        #expect(fact(facts, "Host")?.value == "api.example.com")
        #expect(fact(facts, "Path")?.value == "/v1/customer/category")
        #expect(fact(facts, "HTTP Version")?.value == "HTTP/2")
        #expect(fact(facts, "Time")?.value == TrafficDisplay.time(exchange.timing.startedAt))
        #expect(fact(facts, "Destination")?.value == "api.example.com:443")
        #expect(facts.contains { $0.name == "Exchange ID" } == false)
        #expect(prepared.headline.method == "GET")
        #expect(prepared.headline.host == "api.example.com")
        #expect(prepared.headline.path == "/v1/customer/category")
        #expect(prepared.headline.secure)
    }

    // MARK: - Response overview

    @Test("the response overview uses the shared duration, size, and status formatters")
    func responseOverview() throws {
        let (exchange, prepared) = reading()
        let facts = try #require(prepared.responseFacts)

        #expect(fact(facts, "Status")?.value == "200 OK")
        #expect(fact(facts, "Status")?.note == TrafficDisplay.statusClass(200))
        #expect(fact(facts, "HTTP Version")?.value == "HTTP/2")
        #expect(fact(facts, "Content Type")?.value == "application/json; charset=utf-8")
        #expect(fact(facts, "Content Encoding")?.value == "gzip")
        #expect(
            fact(facts, "Duration")?.value
                == TrafficDisplay.duration(milliseconds: exchange.timing.durationMilliseconds)
        )
        #expect(
            fact(facts, "Captured Size")?.value
                == TrafficDisplay.size(exchange.response?.body.size ?? 0)
        )
        #expect(prepared.headline.statusText == "200 OK")
        #expect(prepared.headline.version == "HTTP/2")
        #expect(prepared.headline.duration == fact(facts, "Duration")?.value)
        #expect(prepared.headline.contentType == "application/json")
        #expect(prepared.headline.responseSize == TrafficDisplay.size(exchange.response?.body.size ?? 0))
    }

    @Test("a mix of protocol versions stays visible in the summary")
    func mixedVersions() {
        let (_, prepared) = reading(requestVersion: .http11, responseVersion: .http2)

        #expect(prepared.headline.version == "HTTP/1.1 → HTTP/2")
        #expect(fact(prepared.requestFacts, "HTTP Version")?.value == "HTTP/1.1")
        #expect(fact(prepared.responseFacts ?? [], "HTTP Version")?.value == "HTTP/2")
    }

    // MARK: - Headers

    @Test("headers keep their order, repeated names, and original values")
    func headers() {
        let headers = Headers([
            Header(name: "Accept", value: .text("application/json")),
            Header(name: "Set-Cookie", value: .text("a=1")),
            Header(name: "Set-Cookie", value: .text("b=2")),
            Header(name: "X-Bytes", value: .bytes(Data([0, 1, 2]))),
        ])
        let presentation = HeaderPresentation(headers: headers)

        #expect(presentation.rows.map(\.name) == ["Accept", "Set-Cookie", "Set-Cookie", "X-Bytes"])
        #expect(presentation.rows.map(\.value) == [
            "application/json", "a=1", "b=2", "<3 bytes, not text>",
        ])
        #expect(presentation.rows[0].copyName == "Accept")
        #expect(presentation.rows[0].copyValue == "application/json")
        #expect(presentation.rows[0].copyHeader == "Accept: application/json")
        #expect(presentation.copyAll == ExchangeText.headers(headers))
        #expect(presentation.matching("set-cookie").map(\.value) == ["a=1", "b=2"])
        #expect(presentation.matching("  ").count == 4)
        #expect(presentation.matching("nope").isEmpty)
    }

    // MARK: - Query

    @Test("duplicate query keys stay, and decoding matches QueryParameters")
    func query() {
        let url = "https://api.example.com/search?tag=a&tag=b&flag&empty=&q=iPhone+17"
        let parameters = QueryParameters.of(url: url)
        let (_, prepared) = reading(url: url)

        #expect(prepared.query == parameters)
        #expect(parameters.map(\.name) == ["tag", "tag", "flag", "empty", "q"])
        #expect(parameters.map(\.value) == ["a", "b", nil, "", "iPhone 17"])
        #expect(parameters.map(\.parameterText) == ["tag=a", "tag=b", "flag", "empty=", "q=iPhone 17"])
        #expect(parameters[4].rawValue == "iPhone+17")
    }

    // MARK: - Authorization

    @Test("authorization shows the scheme and a mask, not the credential")
    func authorization() {
        let token = "super-secret-token"
        let basic = "dXNlcjpwYXNz"
        let headers = Headers([
            Header(name: "Authorization", value: .text("Bearer \(token)")),
            Header(name: "Proxy-Authorization", value: .text("Basic \(basic)")),
            Header(name: "Accept", value: .text("application/json")),
        ])
        let auth = AuthorizationPresentation(headers: headers)
        let shown = auth.entries.map { "\($0.header) \($0.scheme) \($0.summary)" }.joined(separator: "\n")

        #expect(auth.entries.map(\.header) == ["Authorization", "Proxy-Authorization"])
        #expect(auth.entries[0].scheme == "Bearer")
        #expect(auth.entries[0].summary == "Bearer ••••••••")
        #expect(auth.entries[1].scheme == "Basic")
        #expect(auth.entries[1].summary == "Basic ••••••••")
        #expect(!shown.contains(token))
        #expect(!shown.contains(basic))
        #expect(!shown.contains("user:pass"))

        let bare = AuthorizationPresentation(
            headers: Headers([Header(name: "Authorization", value: .text("just-a-token"))])
        )
        #expect(bare.entries[0].scheme == "Unspecified")
        #expect(bare.entries[0].summary == "••••••••")
        #expect(!bare.entries[0].summary.contains("just-a-token"))
        #expect(!bare.entries[0].scheme.contains("just-a-token"))
    }

    @Test("the header list still shows the authorization value it was given")
    func authorizationStaysInHeaders() {
        let (_, prepared) = reading()

        #expect(prepared.requestHeaders.rows.contains { $0.value == "Bearer super-secret-token" })
        #expect(!prepared.authorization.entries[0].summary.contains("super-secret-token"))
    }

    // MARK: - Cookies

    @Test("request and response cookies keep every pair")
    func cookies() {
        let (_, prepared) = reading()

        #expect(prepared.requestCookies.cookies.map(\.name) == ["session_id", "locale"])
        #expect(prepared.requestCookies.cookies.map(\.value) == ["abc", "en"])
        #expect(prepared.responseCookies.cookies.map(\.name) == ["session_id", "locale"])
        #expect(prepared.responseCookies.cookies[0].attributes.map(\.name) == ["Path", "Secure", "HttpOnly"])
        #expect(prepared.responseCookies.cookies[0].attributes.map(\.value) == ["/", "true", "true"])
        #expect(
            prepared.responseCookies.cookies[1].attributes.first?.value
                == "Wed, 21 Oct 2015 07:28:00 GMT"
        )
    }

    // MARK: - Failure

    @Test("a TLS failure is a stage and a sentence, not the core's kind name")
    func failure() {
        let exchange = Fixtures.exchange(
            id: "a",
            url: "https://api.example.com/v1",
            status: nil,
            reason: nil,
            error: TrafficError(
                kind: .clientTLS,
                summary: "the client did not accept the proxy's certificate"
            ),
            timing: Timing(
                startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                completedMicros: 112_000
            )
        )
        let failure = FailurePresentation(exchange: exchange)
        let prepared = InspectorReading.make(
            exchange: exchange,
            inspection: BodyInspection.of(exchange)
        )

        #expect(failure.title == "TLS handshake failed")
        #expect(failure.detail == "The client did not accept the proxy's certificate.")
        #expect(failure.stage == "TLS")
        #expect(failure.facts.first { $0.name == "Destination" }?.value == "api.example.com:443")
        #expect(
            failure.facts.first { $0.name == "Duration" }?.value
                == TrafficDisplay.duration(milliseconds: 112)
        )
        #expect(!failure.title.contains("client_tls"))
        #expect(!failure.detail.contains("client_tls"))
        #expect(prepared.failure == failure)
        #expect(prepared.headline.statusText == "TLS handshake failed")
        #expect(prepared.responseFacts == nil)
        #expect(prepared.pendingDetail == nil)
    }

    @Test("a summary that looks like an internal error is not the sentence shown")
    func internalFailure() {
        let exchange = Fixtures.exchange(
            id: "a",
            status: nil,
            reason: nil,
            error: TrafficError(
                kind: .upstreamTLS,
                summary: "thread panicked at src/tls/session.rs:40"
            )
        )
        let failure = FailurePresentation(exchange: exchange)

        #expect(failure.title == "TLS handshake failed")
        #expect(failure.detail == "The origin's TLS handshake failed.")
        #expect(!failure.detail.contains("panicked"))
        #expect(!failure.detail.contains(".rs"))
    }

    // MARK: - Sections

    @Test("messages is offered only for a handshake or an event stream")
    func sections() {
        let (ordinary, _) = reading()

        #expect(InspectorSections.faces(of: ordinary) == [.request, .response])
        #expect(
            InspectorSections.faces(of: Fixtures.websocketExchange(id: "ws"))
                == [.request, .response, .messages]
        )
        #expect(
            InspectorSections.faces(of: Fixtures.sseExchange(id: "sse"))
                == [.request, .response, .messages]
        )
        #expect(
            RequestSection.allCases.map(\.rawValue)
                == ["Overview", "Headers", "Query", "Body", "Cookies", "Auth", "Raw"]
        )
        #expect(
            ResponseSection.allCases.map(\.rawValue)
                == ["Overview", "Headers", "Body", "Cookies", "Raw"]
        )
    }

    // MARK: - Actions

    @Test("inspector actions follow the traffic row menu")
    func actions() {
        let (ordinary, _) = reading()
        let normal = InspectorActions(summary: ExchangeSummary(exchange: ordinary))
        #expect(normal.canReplay)
        #expect(normal.canCopyCurl)
        #expect(normal.canSave)
        #expect(normal.canCopyURL)
        #expect(normal.canBreakpoint)
        #expect(normal.canApplyRule)

        let socket = InspectorActions(
            summary: ExchangeSummary(exchange: Fixtures.websocketExchange(id: "ws"))
        )
        #expect(!socket.canReplay)
        #expect(!socket.canCopyCurl)
        #expect(socket.canSave)
        #expect(socket.canBreakpoint)
        #expect(socket.canApplyRule)

        let events = InspectorActions(summary: ExchangeSummary(exchange: Fixtures.sseExchange(id: "sse")))
        #expect(!events.canReplay)
        #expect(!events.canCopyCurl)

        let failed = Fixtures.failedExchange(id: "f")
        let failure = InspectorActions(summary: ExchangeSummary(exchange: failed))
        #expect(failure.canReplay)
        #expect(failure.canCopyCurl)
    }

    // MARK: - Narrow width

    @Test("a narrow inspector stacks keys and a wide one does not")
    func width() {
        let narrow = InspectorMetrics.contentWidth(paneWidth: 380)
        let wide = InspectorMetrics.contentWidth(paneWidth: 900)

        #expect(InspectorMetrics.stacksKeys(contentWidth: narrow))
        #expect(!InspectorMetrics.stacksKeys(contentWidth: wide))
        #expect(narrow < InspectorMetrics.stackBreakpoint)
        #expect(wide > InspectorMetrics.keyWidth)
    }

    // MARK: - Reconstructed message

    @Test("the raw section is a reconstruction, and says so")
    func reconstructed() throws {
        let (_, prepared) = reading(
            method: "POST",
            url: "https://api.example.com/v1/users?page=2",
            requestVersion: .http11,
            requestHeaders: Headers([Header(name: "Accept", value: .text("application/json"))]),
            responseVersion: .http11,
            responseHeaders: Headers([Header(name: "Content-Type", value: .text("application/json"))])
        )

        #expect(ReconstructedMessage.label == "Reconstructed")
        #expect(prepared.rawRequest.note.contains("Not the original wire bytes"))
        #expect(prepared.rawRequest.text.hasPrefix("POST /v1/users?page=2 HTTP/1.1"))
        #expect(prepared.rawRequest.text.contains("Accept: application/json"))
        #expect(prepared.rawRequest.text.contains("shown in Body"))
        let response = try #require(prepared.rawResponse)
        #expect(response.text.hasPrefix("HTTP/1.1 \(TrafficDisplay.status(200, reason: "OK"))"))
        #expect(response.note.contains("Not the original wire bytes"))
    }
}
