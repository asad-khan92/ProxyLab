//  What the traffic list is narrowed to.
//
//      search   substring or regex of URL, host, path, method, status,
//               headers, bodies, and stream-message text (in the core)
//      method, status, host, content type, HTTP version, WebSocket, SSE, failed
//      sort     which end of the order to read from
//
//  One value, held in one place, and turned into one `list_traffic` request.
//  The filtering itself happens in the core: this is a page of an indexed
//  query, not a list the app fetched and then sieved.
//
//  # The same filters, evaluated twice
//
//  A `traffic_added` event carries a summary, and the app has to decide
//  whether the row belongs in what is currently shown — without asking the
//  core again for every request the proxy carries. So `matches(_:)` is the
//  same filter evaluated locally, and it has to agree with the SQL exactly or
//  a row will appear that a reload would drop.
//
//  Headers, bodies, and stream payloads are not on a summary. A live event
//  whose only match is in those is picked up by a debounced reload, not by
//  this function. The comparisons that *are* here match the core: exact host
//  with `COLLATE NOCASE` folding, exact method, a status range that a row
//  with no response is outside of, and ASCII-case-insensitive substring
//  search (or NSRegularExpression when regex is on).

import Foundation

/// Which end of the stored order the list is read from.
public enum TrafficSort: String, CaseIterable, Identifiable, Sendable {
    /// The most recent exchange first. What a traffic list shows by default.
    case newestFirst
    /// The earliest exchange first, for reading a session in order.
    case oldestFirst

    public var id: Self { self }

    /// What this is called in the window.
    public var label: String {
        switch self {
        case .newestFirst: "Newest first"
        case .oldestFirst: "Oldest first"
        }
    }

    /// What the core calls it.
    var order: TrafficOrder {
        switch self {
        case .newestFirst: .newest
        case .oldestFirst: .oldest
        }
    }
}

/// A class of status code, as the filter offers them.
public enum StatusClass: Int, CaseIterable, Identifiable, Sendable {
    case informational = 1
    case success = 2
    case redirection = 3
    case clientError = 4
    case serverError = 5

    public var id: Int { rawValue }

    /// What this is called in the window.
    public var label: String { "\(rawValue)xx" }

    /// What the core is asked for.
    var filter: StatusFilter { .class(UInt8(rawValue)) }

    /// Whether `status` is in this class.
    func contains(_ status: UInt16) -> Bool {
        Int(status) / 100 == rawValue
    }
}

/// The methods the filter offers.
///
/// The ones HTTP/1.1 defines and a proxy carries. The core filters on the
/// method exactly as it was sent, so these are spelled as they travel.
public enum HTTPMethodFilter {
    public static let all = [
        "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
    ]
}

/// Exact status codes the filter offers, besides the 1xx–5xx classes.
public enum HTTPStatusCodeFilter {
    public static let common: [UInt16] = [
        200, 201, 204, 301, 302, 304, 400, 401, 403, 404, 500, 502, 503,
    ]
}

/// Content types the filter offers, matched as a substring of the header.
public enum ContentTypeFilter {
    public static let presets: [(id: String, label: String)] = [
        ("json", "JSON"),
        ("html", "HTML"),
        ("xml", "XML"),
        ("javascript", "JavaScript"),
        ("x-www-form-urlencoded", "Form"),
        ("event-stream", "Event stream"),
    ]
}

/// How the traffic list is narrowed.
public struct TrafficFilters: Equatable, Sendable {
    /// What was typed into the search field.
    public var search: String
    /// Whether the search is a regular expression.
    public var searchRegex: Bool
    /// Whether the search distinguishes letter case.
    public var searchCaseSensitive: Bool
    /// One method, or `nil` for every method.
    public var method: String?
    /// One class of status, or `nil` for every status.
    public var status: StatusClass?
    /// One status code exactly, or `nil`. Wins over `status` when both are set.
    public var statusCode: UInt16?
    /// One destination host, or `nil` for every host.
    public var host: String?
    /// A substring of request or response `Content-Type`, or `nil`.
    public var contentType: String?
    /// HTTP/1.1 or HTTP/2 on either half, or `nil`.
    public var httpVersion: HTTPVersion?
    /// Only WebSocket handshakes.
    public var websocket: Bool
    /// Only Server-Sent Event responses.
    public var sse: Bool
    /// Only exchanges that ended in a proxy error.
    public var failed: Bool
    /// Which end of the order to read from.
    public var sort: TrafficSort

    public init(
        search: String = "",
        searchRegex: Bool = false,
        searchCaseSensitive: Bool = false,
        method: String? = nil,
        status: StatusClass? = nil,
        statusCode: UInt16? = nil,
        host: String? = nil,
        contentType: String? = nil,
        httpVersion: HTTPVersion? = nil,
        websocket: Bool = false,
        sse: Bool = false,
        failed: Bool = false,
        sort: TrafficSort = .newestFirst
    ) {
        self.search = search
        self.searchRegex = searchRegex
        self.searchCaseSensitive = searchCaseSensitive
        self.method = method
        self.status = status
        self.statusCode = statusCode
        self.host = host
        self.contentType = contentType
        self.httpVersion = httpVersion
        self.websocket = websocket
        self.sse = sse
        self.failed = failed
        self.sort = sort
    }

    /// Everything, newest first.
    public static let none = TrafficFilters()

    /// The search with the whitespace around it removed.
    ///
    /// A search of spaces is not a search, and sending one would ask the core
    /// for the rows containing a space — which is most of them.
    public var searchTerm: String? {
        let trimmed = search.trimmingCharacters(in: .whitespacesAndNewlines)

        return trimmed.isEmpty ? nil : trimmed
    }

    /// Whether anything is narrowed at all.
    ///
    /// The sort is not a filter: reading the same rows from the other end
    /// hides none of them, so it does not light up "clear filters".
    public var isNarrowed: Bool {
        searchTerm != nil
            || method != nil
            || status != nil
            || statusCode != nil
            || host != nil
            || contentType != nil
            || httpVersion != nil
            || websocket
            || sse
            || failed
    }

    /// Whether the host is the only thing narrowing the list.
    public var isNarrowedOnlyByHost: Bool {
        host != nil
            && searchTerm == nil
            && method == nil
            && status == nil
            && statusCode == nil
            && contentType == nil
            && httpVersion == nil
            && !websocket
            && !sse
            && !failed
    }

    /// How many of the filters are set, for the filter button to show.
    public var activeCount: Int {
        [
            searchTerm != nil,
            method != nil,
            status != nil || statusCode != nil,
            host != nil,
            contentType != nil,
            httpVersion != nil,
            websocket,
            sse,
            failed,
        ]
        .filter { $0 }
        .count
    }

    /// The same filters with nothing narrowed, keeping the sort.
    public var cleared: TrafficFilters {
        TrafficFilters(sort: sort)
    }

    /// The request these filters come to.
    public func listParameters(limit: Int, cursor: TrafficCursor? = nil) -> ListTrafficParameters {
        ListTrafficParameters(
            limit: limit,
            cursor: cursor,
            host: host,
            method: method,
            status: encodedStatus,
            search: searchTerm,
            searchRegex: searchTerm != nil && searchRegex,
            searchCaseSensitive: searchTerm != nil && searchCaseSensitive,
            contentType: contentType,
            httpVersion: httpVersion,
            websocket: websocket,
            sse: sse,
            failed: failed,
            order: sort.order
        )
    }

    /// The same narrowing, for an operation that takes no page.
    public var exportParameters: ListTrafficParameters {
        ListTrafficParameters(
            host: host,
            method: method,
            status: encodedStatus,
            search: searchTerm,
            searchRegex: searchTerm != nil && searchRegex,
            searchCaseSensitive: searchTerm != nil && searchCaseSensitive,
            contentType: contentType,
            httpVersion: httpVersion,
            websocket: websocket,
            sse: sse,
            failed: failed
        )
    }

    private var encodedStatus: StatusFilter? {
        if let statusCode { return .exact(statusCode) }
        return status?.filter
    }

    /// Whether `summary` is one of the rows these filters describe.
    public func matches(_ summary: ExchangeSummary) -> Bool {
        if let host {
            guard let destination = summary.destination?.host,
                destination.compare(host, options: .caseInsensitive) == .orderedSame
            else {
                return false
            }
        }

        if let method, summary.method != method {
            return false
        }

        if let statusCode {
            guard summary.status == statusCode else { return false }
        } else if let status {
            guard let code = summary.status, status.contains(code) else { return false }
        }

        if let contentType {
            let haystacks = [summary.requestContentType, summary.responseContentType]
                .compactMap { $0 }
            guard haystacks.contains(where: { $0.containsASCIICaseInsensitive(contentType) })
            else {
                return false
            }
        }

        if let httpVersion {
            let request = summary.version
            let response = summary.responseVersion
            guard request == httpVersion || response == httpVersion else { return false }
        }

        if websocket, summary.status != 101 {
            return false
        }

        if sse {
            let type = summary.responseContentType ?? ""
            let essence = type.split(separator: ";", maxSplits: 1).first.map(String.init) ?? type
            guard essence.trimmingCharacters(in: .whitespaces).lowercased()
                .hasPrefix("text/event-stream")
            else {
                return false
            }
        }

        if failed, summary.error == nil {
            return false
        }

        if let term = searchTerm {
            return searchableText(of: summary).contains { field in
                matchesSearch(term, in: field)
            }
        }

        return true
    }

    /// The values a live summary can search, matching the columns the core
    /// searches without opening a body.
    private func searchableText(of summary: ExchangeSummary) -> [String] {
        var fields = [summary.url, summary.method]

        if let host = summary.destination?.host { fields.append(host) }
        if let status = summary.status { fields.append(String(status)) }
        if let type = summary.requestContentType { fields.append(type) }
        if let type = summary.responseContentType { fields.append(type) }

        return fields
    }

    private func matchesSearch(_ term: String, in field: String) -> Bool {
        if searchRegex {
            let options: NSRegularExpression.Options = searchCaseSensitive ? [] : .caseInsensitive
            guard let regex = try? NSRegularExpression(pattern: term, options: options) else {
                return false
            }
            let range = NSRange(field.startIndex..<field.endIndex, in: field)
            return regex.firstMatch(in: field, range: range) != nil
        }

        if searchCaseSensitive {
            return field.contains(term)
        }

        return field.containsASCIICaseInsensitive(term)
    }
}

extension String {
    /// Whether this contains `other`, folding case the way SQLite does.
    ///
    /// SQLite's `LIKE` folds only the ASCII letters — `Ä` and `ä` are two
    /// different characters to it. Swift's own case-insensitive comparison
    /// folds Unicode, so using it here would make a row arrive that a reload
    /// would then drop. Matching the core exactly is worth more than matching
    /// more.
    func containsASCIICaseInsensitive(_ other: String) -> Bool {
        guard !other.isEmpty else { return true }

        return asciiLowercased().contains(other.asciiLowercased())
    }

    /// This string with its ASCII letters lowered, and nothing else touched.
    func asciiLowercased() -> String {
        String(
            unicodeScalars.map { scalar in
                (scalar.value >= 65 && scalar.value <= 90)
                    ? Character(Unicode.Scalar(scalar.value + 32)!)
                    : Character(scalar)
            }
        )
    }
}
