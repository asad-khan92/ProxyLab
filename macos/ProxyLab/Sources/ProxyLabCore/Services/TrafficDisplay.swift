//  Turning a captured exchange into the strings a row and a header show.
//
//  All of it is derived from what the core already sent — there is nothing
//  here that asks the server a second question, and nothing that invents a
//  value the exchange did not have. A URL that cannot be parsed is shown as it
//  arrived rather than replaced with a guess.

import Foundation

/// The parts of a summary a traffic row draws.
public enum TrafficDisplay {
    /// The host the request was for.
    ///
    /// From the URL, falling back to the destination the proxy connected to,
    /// which is what an exchange rejected before it had a URL still has.
    public static func host(_ summary: ExchangeSummary) -> String {
        host(url: summary.url, destinationHost: summary.destination?.host)
    }

    /// The host a URL names, or the destination the proxy connected to.
    ///
    /// The same reading as ``host(_:)``. The detail pane has the exchange
    /// rather than the list summary, and it should not grow a second parser.
    public static func host(url: String, destinationHost: String?) -> String {
        if let host = URLComponents(string: url)?.host, !host.isEmpty {
            return host
        }

        return destinationHost ?? "—"
    }

    /// The host the core's own host filter compares, for one summary.
    ///
    /// Deliberately not ``host(_:)``. That one is for *reading* and prefers
    /// the URL, which is what a developer recognises; this one is the
    /// destination the proxy actually connected to, which is the column
    /// `list_traffic` filters on. Filtering by, or pinning, anything else
    /// would narrow the list to nothing whenever the two disagree.
    ///
    /// `nil` for an exchange that was refused before a destination was worked
    /// out, which is one there is no host to filter by.
    public static func filterHost(_ summary: ExchangeSummary) -> String? {
        summary.destination?.host
    }

    /// The path and query, which is what distinguishes one row from the next.
    ///
    /// Anything that is not an absolute URL is shown exactly as it arrived:
    /// the core assembles an absolute URL for every exchange it captures, so
    /// a string that is not one is not something to reformat into a guess.
    public static func path(_ url: String) -> String {
        guard let components = URLComponents(string: url), components.host != nil else {
            return url
        }

        let path = components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath
        guard let query = components.percentEncodedQuery else { return path }

        return "\(path)?\(query)"
    }

    /// What a row shows in place of a status code.
    public static func outcome(_ summary: ExchangeSummary) -> Outcome {
        if let status = summary.status {
            return .status(status)
        }
        if summary.error != nil {
            return .failed
        }

        return .pending
    }

    /// How an exchange ended.
    public enum Outcome: Equatable, Sendable {
        /// The origin answered with this status code.
        case status(UInt16)
        /// There is no response and the exchange says why.
        case failed
        /// There is no response and no failure: it has not finished.
        case pending
    }

    /// A status code with its reason phrase, when the response carried one.
    ///
    /// The stored reason and nothing else: there is no table of codes here, so
    /// an extension code the world has not standardised shows as the code it
    /// is rather than as the nearest one this build has heard of. HTTP/2
    /// carries no reason phrase at all, and an exchange that has none shows
    /// its code alone.
    public static func status(_ status: UInt16, reason: String?) -> String {
        guard let reason, !reason.isEmpty else { return String(status) }

        return "\(status) \(reason)"
    }

    /// What class of answer a status code is, by the range it falls in.
    ///
    /// Derived from the code rather than looked up, which is how RFC 9110
    /// defines it and is why an unrecognised code in a known range still says
    /// something true.
    public static func statusClass(_ status: UInt16) -> String {
        switch status {
        case 100..<200: "Informational (1xx)"
        case 200..<300: "Success (2xx)"
        case 300..<400: "Redirection (3xx)"
        case 400..<500: "Client error (4xx)"
        case 500..<600: "Server error (5xx)"
        default: "Outside every standard class"
        }
    }

    /// A duration in the shortest form that stays honest.
    public static func duration(milliseconds: Double?) -> String? {
        guard let milliseconds else { return nil }

        if milliseconds < 1 {
            return String(format: "%.2f ms", milliseconds)
        }
        if milliseconds < 1_000 {
            return String(format: "%.0f ms", milliseconds)
        }

        return String(format: "%.2f s", milliseconds / 1_000)
    }

    /// A byte count, for a body or a header value.
    public static func size(_ bytes: UInt64) -> String {
        Int64(min(bytes, UInt64(Int64.max))).formatted(.byteCount(style: .binary))
    }

    /// The time of day an exchange started, to the millisecond.
    ///
    /// To the millisecond because two requests a browser fires together are
    /// otherwise the same row twice.
    public static func time(_ timestamp: Timestamp) -> String {
        timestamp.date.formatted(clockFormat)
    }

    /// `DateFormatter` is not `Sendable` and cannot be a shared constant;
    /// this style is a value, and is both.
    private static let clockFormat = Date.VerbatimFormatStyle(
        format: """
            \(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased)):\
            \(minute: .twoDigits):\(second: .twoDigits).\(secondFraction: .fractional(3))
            """,
        timeZone: .current,
        calendar: .current
    )
}
