//  The summary at the top of the inspector, and the two overview sections.
//
//      GET   api.example.com
//            /api/v1/customer/category
//      200 OK   HTTP/2   84 ms   application/json
//
//  Durations, sizes, and the clock go through TrafficDisplay. The path in
//  the summary leaves the query off, because the query section is the list
//  of parameters. The URL in the overview is the stored URL, unchanged.

import Foundation

/// One labeled value in an overview.
struct OverviewFact: Equatable, Sendable, Identifiable {
    let name: String
    let value: String
    var note: String?
    var id: String { name }
}

/// How a status, or the lack of one, is drawn in the summary.
enum OverviewTone: Equatable, Sendable {
    case success
    case redirect
    case clientError
    case serverError
    case informational
    case failed
    case pending
    case other
}

/// The compact summary above Request and Response.
struct InspectorHeadline: Equatable, Sendable {
    let method: String
    let host: String
    /// The path, without the query string.
    let path: String
    let statusText: String
    let statusTone: OverviewTone
    let version: String
    let versionHelp: String
    let duration: String?
    /// The response media type, without parameters, when one was declared.
    let contentType: String?
    let time: String
    let requestSize: String?
    let responseSize: String?
    /// Intercepted TLS, as opposed to a plaintext request.
    let secure: Bool
}

/// The request overview, the response overview, and the summary above them.
enum ExchangeOverviewPresentation {
    static func headline(
        exchange: TrafficExchange,
        inspection: ExchangeInspection
    ) -> InspectorHeadline {
        let versions = ExchangeVersions.of(exchange)
        let failure = exchange.error != nil

        return InspectorHeadline(
            method: method(exchange.request.method),
            host: TrafficDisplay.host(
                url: exchange.request.url,
                destinationHost: exchange.connection.destination?.host
            ),
            path: path(exchange.request.url),
            statusText: statusText(exchange),
            statusTone: failure ? .failed : tone(of: exchange),
            version: versions.label,
            versionHelp: versions.help,
            duration: TrafficDisplay.duration(milliseconds: exchange.timing.durationMilliseconds),
            contentType: contentType(inspection.response),
            time: TrafficDisplay.time(exchange.timing.startedAt),
            requestSize: size(inspection.request),
            responseSize: inspection.response.flatMap(size),
            secure: exchange.connection.scheme == .https
        )
    }

    static func requestFacts(exchange: TrafficExchange) -> [OverviewFact] {
        var facts = [
            OverviewFact(name: "Method", value: exchange.request.method),
            OverviewFact(name: "URL", value: exchange.request.url),
            OverviewFact(
                name: "Host",
                value: TrafficDisplay.host(
                    url: exchange.request.url,
                    destinationHost: exchange.connection.destination?.host
                )
            ),
            OverviewFact(name: "Path", value: path(exchange.request.url)),
            OverviewFact(
                name: "HTTP Version",
                value: ExchangeVersions.name(exchange.request.version)
            ),
            OverviewFact(name: "Time", value: TrafficDisplay.time(exchange.timing.startedAt)),
        ]

        if let destination = exchange.connection.destination {
            facts.append(
                OverviewFact(
                    name: "Destination",
                    value: "\(destination.host):\(destination.port)"
                )
            )
        }

        return facts
    }

    /// The response overview. Nil when there is no response: the caller shows
    /// the failure, or that the exchange has not finished.
    static func responseFacts(
        exchange: TrafficExchange,
        inspection: ExchangeInspection
    ) -> [OverviewFact]? {
        guard let response = exchange.response else { return nil }

        var facts = [
            OverviewFact(
                name: "Status",
                value: TrafficDisplay.status(response.status, reason: response.reason),
                note: TrafficDisplay.statusClass(response.status)
            ),
            OverviewFact(
                name: "HTTP Version",
                value: ExchangeVersions.name(response.version)
            ),
            OverviewFact(
                name: "Content Type",
                value: inspection.response?.contentType?.raw ?? "None declared"
            ),
        ]

        if let encoding = contentEncodings(response.headers) {
            facts.append(OverviewFact(name: "Content Encoding", value: encoding))
        }

        facts.append(
            OverviewFact(
                name: "Duration",
                value: TrafficDisplay.duration(milliseconds: exchange.timing.durationMilliseconds)
                    ?? "Not finished"
            )
        )
        facts.append(bodyFact(inspection.response))

        return facts
    }

    /// Every `Content-Encoding` value, in the order the message carried them.
    static func contentEncodings(_ headers: Headers) -> String? {
        let values = headers.compactMap { header -> String? in
            guard header.name.caseInsensitiveCompare("Content-Encoding") == .orderedSame,
                let text = header.value.text?.trimmingCharacters(in: .whitespacesAndNewlines),
                !text.isEmpty
            else { return nil }

            return text
        }
        guard !values.isEmpty else { return nil }

        return values.joined(separator: ", ")
    }

    /// The path of `url`, without the query. A string that is not an absolute
    /// URL is returned as `TrafficDisplay` would show it.
    static func path(_ url: String) -> String {
        guard let components = URLComponents(string: url), components.host != nil else {
            return TrafficDisplay.path(url)
        }

        let path = components.percentEncodedPath

        return path.isEmpty ? "/" : path
    }

    private static func method(_ method: String) -> String {
        let trimmed = method.trimmingCharacters(in: .whitespacesAndNewlines)

        return trimmed.isEmpty ? "—" : trimmed
    }

    private static func statusText(_ exchange: TrafficExchange) -> String {
        if let response = exchange.response {
            return TrafficDisplay.status(response.status, reason: response.reason)
        }
        if exchange.error != nil {
            return FailurePresentation(exchange: exchange).title
        }

        return "Waiting"
    }

    private static func tone(of exchange: TrafficExchange) -> OverviewTone {
        guard let status = exchange.response?.status else { return .pending }

        switch status {
        case 200..<300: return .success
        case 300..<400: return .redirect
        case 400..<500: return .clientError
        case 500..<600: return .serverError
        case 100..<200: return .informational
        default: return .other
        }
    }

    /// The media type without its parameters, for the summary line.
    private static func contentType(_ body: BodyInspection?) -> String? {
        guard let body else { return nil }
        if let type = body.contentType { return type.essence }

        return nil
    }

    private static func size(_ body: BodyInspection) -> String? {
        guard !body.isEmpty else { return nil }

        return TrafficDisplay.size(body.capture.size)
    }

    private static func bodyFact(_ body: BodyInspection?) -> OverviewFact {
        guard let body, !body.isEmpty else {
            return OverviewFact(name: "Captured Size", value: "None")
        }

        let note: String? = body.capture.truncated
            ? "Captured \(TrafficDisplay.size(UInt64(body.capture.capturedByteCount))) — truncated"
            : nil

        return OverviewFact(
            name: "Captured Size",
            value: TrafficDisplay.size(body.capture.size),
            note: note
        )
    }
}
