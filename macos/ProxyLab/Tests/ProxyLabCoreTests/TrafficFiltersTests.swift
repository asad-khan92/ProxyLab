//  The filters, as a value.
//
//  Two halves that have to agree: what is asked of the core, and what is
//  decided locally about an exchange that arrives as an event. A disagreement
//  between them is a row that appears and then vanishes on the next reload, so
//  the local half is tested against the same cases the Rust suite tests the
//  SQL half against.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Traffic filters")
struct TrafficFiltersTests {
    /// A summary with everything a filter looks at.
    private func summary(
        id: String = "a",
        method: String = "GET",
        host: String? = "api.example.com",
        url: String = "https://api.example.com/v1/users",
        version: HTTPVersion = .http11,
        status: UInt16? = 200,
        responseVersion: HTTPVersion? = .http11,
        requestContentType: String? = nil,
        responseContentType: String? = nil,
        error: TrafficError? = nil
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            durationMicros: 1_000,
            client: "127.0.0.1:54321",
            scheme: .https,
            destination: host.map { Destination(host: $0, port: 443) },
            method: method,
            url: url,
            version: version,
            status: status,
            responseVersion: responseVersion,
            requestContentType: requestContentType,
            responseContentType: responseContentType,
            completed: error == nil,
            error: error
        )
    }

    // MARK: - Search

    @Test("an empty search matches everything")
    func emptySearch() {
        #expect(TrafficFilters().matches(summary()))
        #expect(TrafficFilters(search: "   ").matches(summary()))
        // Whitespace is not a search, so it is not sent as one either.
        #expect(TrafficFilters(search: "   ").searchTerm == nil)
        #expect(!TrafficFilters(search: "  ").isNarrowed)
    }

    @Test("a search matches part of the host")
    func searchHost() {
        #expect(TrafficFilters(search: "api.example").matches(summary()))
        #expect(!TrafficFilters(search: "cdn.example").matches(summary()))
    }

    @Test("a search matches part of the URL or path")
    func searchPath() {
        #expect(TrafficFilters(search: "/v1/users").matches(summary()))
        #expect(TrafficFilters(search: "users").matches(summary()))
        #expect(!TrafficFilters(search: "/v2/").matches(summary()))
    }

    @Test("a search matches the method")
    func searchMethod() {
        #expect(TrafficFilters(search: "POST").matches(summary(method: "POST")))
        #expect(!TrafficFilters(search: "POST").matches(summary(method: "GET")))
    }

    @Test("a search matches the status written out")
    func searchStatus() {
        #expect(TrafficFilters(search: "404").matches(summary(status: 404)))
        #expect(!TrafficFilters(search: "404").matches(summary(status: 200)))
    }

    @Test("a search does not care about case")
    func searchCase() {
        let row = summary(method: "POST", host: "API.Example.com", url: "https://API.Example.com/V1")

        for term in ["api.example", "API.EXAMPLE", "post", "POST", "/v1", "/V1"] {
            #expect(TrafficFilters(search: term).matches(row), "\(term)")
        }
    }

    @Test("a search folds case the way the core does, and no further")
    func searchFoldsASCIIOnly() {
        // SQLite's LIKE folds the ASCII letters and nothing else. Folding
        // more here would show a row that the next reload would drop.
        #expect("HOST".containsASCIICaseInsensitive("host"))
        #expect("Ärger".containsASCIICaseInsensitive("Är"))
        #expect(!"Ärger".containsASCIICaseInsensitive("är"))
    }

    @Test("a search that matches nothing matches nothing")
    func searchNoMatch() {
        #expect(!TrafficFilters(search: "nowhere").matches(summary()))
    }

    @Test("a case-sensitive search keeps the letters it was given")
    func searchCaseSensitive() {
        let row = summary(url: "https://api.example.com/Users")

        #expect(TrafficFilters(search: "Users", searchCaseSensitive: true).matches(row))
        #expect(!TrafficFilters(search: "users", searchCaseSensitive: true).matches(row))
        #expect(TrafficFilters(search: "users").matches(row))
    }

    @Test("a regex search matches the pattern")
    func searchRegex() {
        let row = summary(url: "https://api.example.com/user-12")

        #expect(TrafficFilters(search: #"user-\d+"#, searchRegex: true).matches(row))
        #expect(
            !TrafficFilters(search: #"user-\d+"#, searchRegex: true).matches(
                summary(url: "https://api.example.com/user-ab")
            )
        )
        #expect(!TrafficFilters(search: "(", searchRegex: true).matches(row))
    }

    @Test("a search is sent with the whitespace around it removed")
    func searchTrimmed() {
        #expect(TrafficFilters(search: "  login  ").searchTerm == "login")
    }

    // MARK: - Method

    @Test("a method filter keeps only that method")
    func methodFilter() {
        for method in ["GET", "POST", "DELETE"] {
            let filters = TrafficFilters(method: method)

            #expect(filters.matches(summary(method: method)))
            #expect(!filters.matches(summary(method: "PATCH")))
        }
    }

    @Test("no method filter keeps every method")
    func everyMethod() {
        let filters = TrafficFilters()

        for method in HTTPMethodFilter.all {
            #expect(filters.matches(summary(method: method)), "\(method)")
        }
    }

    @Test("the methods offered are the ones a proxy carries")
    func methodList() {
        #expect(HTTPMethodFilter.all.contains("GET"))
        #expect(HTTPMethodFilter.all.contains("POST"))
        #expect(HTTPMethodFilter.all.contains("CONNECT"))
        #expect(HTTPMethodFilter.all.contains("OPTIONS"))
    }

    // MARK: - Status

    @Test("a status class keeps only the codes in it")
    func statusClasses() {
        let cases: [(StatusClass, UInt16, UInt16)] = [
            (.success, 204, 301),
            (.redirection, 304, 200),
            (.clientError, 404, 500),
            (.serverError, 503, 404),
        ]

        for (group, inside, outside) in cases {
            let filters = TrafficFilters(status: group)

            #expect(filters.matches(summary(status: inside)), "\(group.label) and \(inside)")
            #expect(!filters.matches(summary(status: outside)), "\(group.label) and \(outside)")
        }
    }

    @Test("an exchange with no response is outside every status class")
    func statusOfNoResponse() {
        // Which is what the core's `BETWEEN` does to a null, and why such an
        // exchange is still there under no status filter at all.
        let unanswered = summary(status: nil)

        for group in StatusClass.allCases {
            #expect(!TrafficFilters(status: group).matches(unanswered), "\(group.label)")
        }
        #expect(TrafficFilters().matches(unanswered))
    }

    @Test("a status class is named the way it is read")
    func statusLabels() {
        #expect(StatusClass.success.label == "2xx")
        #expect(StatusClass.serverError.label == "5xx")
    }

    @Test("an exact status code keeps only that code")
    func exactStatusCode() {
        let filters = TrafficFilters(statusCode: 404)

        #expect(filters.matches(summary(status: 404)))
        #expect(!filters.matches(summary(status: 400)))
        #expect(!filters.matches(summary(status: nil)))
    }

    // MARK: - Host

    @Test("a host filter keeps only that host, exactly")
    func hostFilter() {
        let filters = TrafficFilters(host: "api.example.com")

        #expect(filters.matches(summary(host: "api.example.com")))
        // Exact, not a suffix: the core compares the whole column.
        #expect(!filters.matches(summary(host: "cdn.example.com")))
        #expect(!filters.matches(summary(host: "example.com")))
        #expect(!filters.matches(summary(host: "sub.api.example.com")))
    }

    @Test("a host filter does not care how the host was spelled")
    func hostCase() {
        #expect(TrafficFilters(host: "api.example.com").matches(summary(host: "API.Example.COM")))
    }

    @Test("a host filter excludes an exchange that never had a destination")
    func hostOfNoDestination() {
        #expect(!TrafficFilters(host: "api.example.com").matches(summary(host: nil)))
    }

    @Test("a host filter keeps every request to that host")
    func hostWithManyRequests() {
        let filters = TrafficFilters(host: "api.example.com")
        let rows = [
            summary(id: "a", method: "GET", url: "https://api.example.com/one"),
            summary(id: "b", method: "POST", url: "https://api.example.com/two", status: 201),
            summary(id: "c", host: "cdn.example.com", url: "https://cdn.example.com/x"),
        ]

        #expect(rows.filter(filters.matches).map(\.id) == ["a", "b"])
    }

    // MARK: - Content type, version, streams, failed

    @Test("a content-type filter matches either half")
    func contentTypeFilter() {
        let json = summary(requestContentType: "application/json")
        let html = summary(responseContentType: "text/html; charset=utf-8")

        #expect(TrafficFilters(contentType: "json").matches(json))
        #expect(!TrafficFilters(contentType: "json").matches(html))
        #expect(TrafficFilters(contentType: "html").matches(html))
        #expect(!TrafficFilters(contentType: "json").matches(summary()))
    }

    @Test("an HTTP version filter matches either half")
    func httpVersionFilter() {
        let h2 = summary(version: .http2, responseVersion: .http2)
        let mixed = summary(version: .http11, responseVersion: .http2)

        #expect(TrafficFilters(httpVersion: .http2).matches(h2))
        #expect(TrafficFilters(httpVersion: .http2).matches(mixed))
        #expect(!TrafficFilters(httpVersion: .http2).matches(summary()))
    }

    @Test("a WebSocket filter keeps switching protocols")
    func websocketFilter() {
        #expect(TrafficFilters(websocket: true).matches(summary(status: 101)))
        #expect(!TrafficFilters(websocket: true).matches(summary(status: 200)))
    }

    @Test("an SSE filter keeps event-stream responses")
    func sseFilter() {
        let sse = summary(responseContentType: "text/event-stream; charset=utf-8")

        #expect(TrafficFilters(sse: true).matches(sse))
        #expect(!TrafficFilters(sse: true).matches(summary(responseContentType: "application/json")))
        #expect(!TrafficFilters(sse: true).matches(summary()))
    }

    @Test("a failed filter keeps proxy errors")
    func failedFilter() {
        let failed = summary(
            status: nil,
            error: TrafficError(kind: .upstreamConnect, summary: "could not connect")
        )

        #expect(TrafficFilters(failed: true).matches(failed))
        #expect(!TrafficFilters(failed: true).matches(summary()))
    }

    // MARK: - Combining

    @Test("the filters combine, and all of them must match")
    func combined() {
        let filters = TrafficFilters(
            search: "users",
            method: "POST",
            status: .success,
            host: "api.example.com"
        )

        #expect(
            filters.matches(
                summary(method: "POST", url: "https://api.example.com/v1/users", status: 201)
            )
        )
        // One at a time, each failing on its own.
        #expect(!filters.matches(summary(method: "GET", status: 201)))
        #expect(!filters.matches(summary(method: "POST", status: 500)))
        #expect(!filters.matches(summary(method: "POST", host: "cdn.example.com", status: 201)))
        #expect(
            !filters.matches(
                summary(method: "POST", url: "https://api.example.com/v1/orders", status: 201)
            )
        )
    }

    // MARK: - What is sent

    @Test("the filters become the request the core reads")
    func listParameters() {
        let parameters = TrafficFilters(
            search: " login ",
            method: "POST",
            status: .clientError,
            host: "api.example.com",
            sort: .oldestFirst
        ).listParameters(limit: 50)

        #expect(parameters.limit == 50)
        #expect(parameters.search == "login")
        #expect(parameters.searchRegex == false)
        #expect(parameters.method == "POST")
        #expect(parameters.status == .class(4))
        #expect(parameters.host == "api.example.com")
        #expect(parameters.order == .oldest)
        #expect(parameters.cursor == nil)
    }

    @Test("regex and the extra filters are sent when they are set")
    func extraListParameters() {
        let parameters = TrafficFilters(
            search: "user-\\d+",
            searchRegex: true,
            searchCaseSensitive: true,
            statusCode: 404,
            contentType: "json",
            httpVersion: .http2,
            websocket: true,
            failed: true
        ).listParameters(limit: 10)

        #expect(parameters.search == "user-\\d+")
        #expect(parameters.searchRegex)
        #expect(parameters.searchCaseSensitive)
        #expect(parameters.status == .exact(404))
        #expect(parameters.contentType == "json")
        #expect(parameters.httpVersion == .http2)
        #expect(parameters.websocket)
        #expect(parameters.failed)
    }

    @Test("filters that narrow nothing send nothing to narrow with")
    func emptyParameters() {
        let parameters = TrafficFilters().listParameters(limit: 200)

        #expect(parameters.search == nil)
        #expect(parameters.searchRegex == false)
        #expect(parameters.method == nil)
        #expect(parameters.status == nil)
        #expect(parameters.host == nil)
        #expect(parameters.contentType == nil)
        #expect(parameters.httpVersion == nil)
        #expect(parameters.websocket == false)
        #expect(parameters.sse == false)
        #expect(parameters.failed == false)
        #expect(parameters.order == .newest)
    }

    @Test("a page request carries the cursor it was given")
    func cursorParameter() {
        let cursor = TrafficCursor(startedAtMs: 1_700_000_000_500, id: "a")
        let parameters = TrafficFilters().listParameters(limit: 10, cursor: cursor)

        #expect(parameters.cursor == cursor)
    }

    // MARK: - State

    @Test("only the filters count as narrowing, not the sort")
    func narrowing() {
        #expect(!TrafficFilters().isNarrowed)
        #expect(!TrafficFilters(sort: .oldestFirst).isNarrowed)
        #expect(TrafficFilters(search: "x").isNarrowed)
        #expect(TrafficFilters(method: "GET").isNarrowed)
        #expect(TrafficFilters(status: .success).isNarrowed)
        #expect(TrafficFilters(statusCode: 404).isNarrowed)
        #expect(TrafficFilters(host: "example.com").isNarrowed)
        #expect(TrafficFilters(contentType: "json").isNarrowed)
        #expect(TrafficFilters(httpVersion: .http2).isNarrowed)
        #expect(TrafficFilters(websocket: true).isNarrowed)
        #expect(TrafficFilters(sse: true).isNarrowed)
        #expect(TrafficFilters(failed: true).isNarrowed)
    }

    @Test("clearing removes every filter and keeps the order")
    func cleared() {
        let cleared = TrafficFilters(
            search: "x",
            method: "GET",
            status: .success,
            host: "example.com",
            sort: .oldestFirst
        ).cleared

        #expect(cleared == TrafficFilters(sort: .oldestFirst))
        #expect(!cleared.isNarrowed)
        #expect(cleared.sort == .oldestFirst)
    }

    @Test("how many filters are set is what the interface counts")
    func activeCount() {
        #expect(TrafficFilters().activeCount == 0)
        #expect(TrafficFilters(search: "x", method: "GET").activeCount == 2)
        #expect(
            TrafficFilters(search: "x", method: "GET", status: .success, host: "h").activeCount == 4
        )
        #expect(TrafficFilters(contentType: "json", websocket: true, failed: true).activeCount == 3)
        #expect(TrafficFilters(status: .clientError, statusCode: 404).activeCount == 1)
    }
}
