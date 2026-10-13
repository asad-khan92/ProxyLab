//  A breakpoint started from one captured exchange.
//
//      selected row ──▶ host, path, method ──▶ one or two breakpoints
//
//  The row is a finished exchange. Nothing here changes it. The draft says
//  which later request should pause, using the matcher a breakpoint already
//  has: host, path, and method. The query, the headers, and the body are
//  left out. A query string is often the part that changes between two
//  calls, and matching it would wait for a request that never comes again.
//
//  Request and response are separate breakpoints, because a breakpoint has
//  one direction. Checking both configures two of them. Each stays until it
//  is disabled or deleted. The core has no one-shot breakpoint, so this draft
//  does not pretend to remove itself after a single pause.

import Foundation

/// The sheet opened from a traffic row, before anything is configured.
struct TrafficBreakpointDraft: Equatable {
    var host: String
    var path: String
    /// A prefix matches every path that starts with `path`. Exact is the
    /// default, so one URL does not pause its neighbours.
    var pathIsPrefix: Bool
    var method: String
    var pauseRequest: Bool
    var pauseResponse: Bool

    /// What the sheet calls this draft. Derived from the matcher, because a
    /// breakpoint has no name of its own.
    var name: String {
        let host = host.trimmingCharacters(in: .whitespaces)
        let path = path.trimmingCharacters(in: .whitespaces)
        let method = method.trimmingCharacters(in: .whitespaces)
        let target = host.isEmpty ? path : (path.isEmpty ? host : host + path)

        if method.isEmpty { return target.isEmpty ? "Breakpoint" : target }
        if target.isEmpty { return method }

        return "\(method) \(target)"
    }

    /// The criteria the fields describe.
    var matcher: RuleMatcher {
        var matcher = RuleMatcher.any
        let host = host.trimmingCharacters(in: .whitespaces)
        let path = path.trimmingCharacters(in: .whitespaces)
        let method = method.trimmingCharacters(in: .whitespaces)

        if !host.isEmpty { matcher.host = .exact(host) }
        if !path.isEmpty { matcher.path = pathIsPrefix ? .prefix(path) : .exact(path) }
        if !method.isEmpty { matcher.method = .exact(method) }

        return matcher
    }

    /// The breakpoints this draft would configure, request first.
    var directions: [BreakpointDirection] {
        var directions: [BreakpointDirection] = []
        if pauseRequest { directions.append(.request) }
        if pauseResponse { directions.append(.response) }

        return directions
    }

    /// Why the core would refuse this, or why there is nothing to configure.
    var refusal: String? {
        let host = host.trimmingCharacters(in: .whitespaces)
        let path = path.trimmingCharacters(in: .whitespaces)
        let method = method.trimmingCharacters(in: .whitespaces)

        if !host.isEmpty, host.contains("/") || host.contains(where: \.isWhitespace) {
            return "A host is a host name, not a URL."
        }
        if !path.isEmpty, !path.hasPrefix("/") {
            return "A path begins with “/”."
        }
        if !method.isEmpty, !TransformationChecks.isToken(method) {
            return "A method is one word, with no spaces."
        }
        if directions.isEmpty {
            return "Choose request, response, or both."
        }

        return nil
    }

    /// What will pause, in one line under the fields.
    var pauseNote: String {
        switch (pauseRequest, pauseResponse) {
        case (true, true):
            "The request pauses first. After it continues, the response can pause on its own."
        case (true, false):
            "The next matching request pauses before it is sent."
        case (false, true):
            "The next matching response pauses before the client receives it."
        case (false, false):
            "Choose request, response, or both."
        }
    }

    /// The selected exchange is finished. This draft is for the next one.
    static let scopeNote =
        "The selected exchange stays as it was. The next match will pause."

    /// A draft filled from `summary`.
    ///
    /// Host, path, and method only. The path does not include the query.
    /// Request and response both start checked, and each one is still the
    /// breakpoint direction the core already has.
    static func from(summary: ExchangeSummary) -> TrafficBreakpointDraft {
        TrafficBreakpointDraft(
            host: host(of: summary),
            path: path(of: summary.url),
            pathIsPrefix: false,
            method: method(of: summary.method),
            pauseRequest: true,
            pauseResponse: true
        )
    }

    private static func host(of summary: ExchangeSummary) -> String {
        if let host = URLComponents(string: summary.url)?.host, !host.isEmpty {
            return host
        }

        return summary.destination?.host ?? ""
    }

    /// The path without the query. Empty when it is not a path the matcher
    /// would accept.
    private static func path(of url: String) -> String {
        guard let components = URLComponents(string: url), components.host != nil else { return "" }

        let path = components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath

        return TransformationChecks.isPath(path) ? path : ""
    }

    private static func method(of method: String) -> String {
        let method = method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()

        return TransformationChecks.isToken(method) ? method : ""
    }
}
