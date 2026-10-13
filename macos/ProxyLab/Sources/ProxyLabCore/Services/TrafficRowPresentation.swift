//  What one traffic row shows, decided before any view draws it.
//
//  Narrow, the column squeezed toward its 280-point minimum:
//
//      GET   200   api.example.com                         84 ms
//      /v1/customer/categories
//
//  Regular, around the column's ideal width:
//
//      GET   200   api.example.com                  JSON  HTTP/2
//            /v1/customer/categories                        84 ms
//
//  Wide, the column pushed toward its 520-point maximum:
//
//      GET   200   api.example.com       JSON  HTTP/2  12:04:31.882
//            /v1/customer/categories                        84 ms
//
//  HTTP/1.1 is the ordinary case, so it is drawn only when there is room.
//  HTTP/2, a mix of the two, a WebSocket handshake, and an event stream
//  are drawn as soon as they fit. A selected domain quiets the host — every
//  visible row is already that host — and the path takes the emphasis. The
//  host stays on the row.
//
//  A failure is a short word (TLS, Blocked, Connect), never the core's
//  error sentence. Status colour is the text of that word or code. Nothing
//  here paints a background, so the list's own selection stays the thing
//  that marks the selected row.
//
//  Body size is not here. A summary has no body length — that lives on the
//  captured message, which a row does not fetch. `TrafficDisplay.size` is
//  the one formatter, and the detail pane is what uses it.

import Foundation

/// How much of a traffic row fits in the list column.
enum TrafficRowWidth: Equatable, Sendable {
    /// Method, status, host, path, and duration. A WebSocket, SSE, or
    /// failure mark stays. Content kind, protocol, and the clock do not.
    case narrow
    /// Adds the content kind, and the protocol when it is not HTTP/1.1.
    case regular
    /// Adds HTTP/1.1 and the time of day.
    case wide

    /// The list column runs from 280 to 520. Below 320 it is the squeezed
    /// column; at 460 and above it has room for the clock.
    static func fitting(_ width: CGFloat) -> TrafficRowWidth {
        if width < 320 { return .narrow }
        if width < 460 { return .regular }
        return .wide
    }
}

/// The strings and marks one traffic row draws.
struct TrafficRowPresentation: Equatable, Sendable {
    /// How an exchange ended, as the status column reads it.
    struct Status: Equatable, Sendable {
        enum Tone: Equatable, Sendable {
            case success
            case redirect
            case clientError
            case serverError
            case informational
            case failed
            case pending
            case other
        }

        /// The code, or a short failure word when there is no code.
        var text: String
        var tone: Tone
        /// What a pointer hover can say. Not the core's error sentence.
        var help: String
        /// The same fact, as one clause of the row's accessibility label.
        var spoken: String
    }

    /// One trailing mark: a content kind, a stream, a protocol, or a failure.
    struct Indicator: Equatable, Sendable, Identifiable {
        enum Role: Hashable, Sendable {
            case content
            case websocket
            case sse
            case version
            case failure
        }

        var role: Role
        var label: String
        var help: String
        var spoken: String

        var id: Role { role }
    }

    var width: TrafficRowWidth
    /// The method as it was captured. Empty becomes an em dash.
    var method: String
    var status: Status
    var host: String
    /// A selected domain: the host is still shown, in a quieter face.
    var hostIsQuiet: Bool
    var path: String
    var indicators: [Indicator]
    /// `TrafficDisplay.duration`, or nil when the exchange has not finished.
    var duration: String?
    /// The clock, and only at ``TrafficRowWidth/wide``. No date.
    var time: String?
    var accessibilityLabel: String

    /// On a narrow row the duration stays with the method and status, so the
    /// path underneath can use the whole width. Wider rows trail it after
    /// the path.
    var durationFollowsPath: Bool { width != .narrow }

    /// Wider rows line the path up under the host. A narrow row does not
    /// spend that indent.
    var pathAlignsWithHost: Bool { width != .narrow }

    init(summary: ExchangeSummary, width: TrafficRowWidth, domainSelected: Bool) {
        self.width = width
        method = Self.method(summary.method)
        host = TrafficDisplay.host(summary)
        hostIsQuiet = domainSelected
        path = TrafficDisplay.path(summary.url)
        status = Self.status(of: summary)
        indicators = Self.indicators(of: summary, width: width)
        duration = TrafficDisplay.duration(milliseconds: summary.durationMilliseconds)
        time = width == .wide ? TrafficDisplay.time(summary.startedAt) : nil
        accessibilityLabel = Self.accessibility(
            method: method,
            status: status,
            host: host,
            path: path,
            indicators: indicators,
            duration: duration,
            time: time
        )
    }

    private static func method(_ method: String) -> String {
        let trimmed = method.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? "—" : trimmed
    }

    private static func status(of summary: ExchangeSummary) -> Status {
        if let code = summary.status {
            let failed = summary.error.map { failure($0.kind) }
            return Status(
                text: String(code),
                tone: failed == nil ? tone(of: code) : .failed,
                help: failed?.help ?? "\(code) · \(TrafficDisplay.statusClass(code))",
                spoken: failed.map { "status \(code), \($0.text)" } ?? "status \(code)"
            )
        }

        if let error = summary.error {
            let failed = failure(error.kind)
            return Status(
                text: failed.text,
                tone: .failed,
                help: failed.help,
                spoken: "failed, \(failed.text)"
            )
        }

        return Status(
            text: "—",
            tone: .pending,
            help: "No response yet",
            spoken: "no response yet"
        )
    }

    private static func tone(of status: UInt16) -> Status.Tone {
        switch status {
        case 200..<300: .success
        case 300..<400: .redirect
        case 400..<500: .clientError
        case 500..<600: .serverError
        case 100..<200: .informational
        default: .other
        }
    }

    /// A short word for a failure, and a sentence that is not the core's
    /// own error text.
    private static func failure(_ kind: TrafficErrorKind) -> (text: String, help: String) {
        if kind == .requestRejected {
            return ("Blocked", "The proxy rejected the request")
        }
        if kind == .upstreamConnect {
            return ("Connect", "The origin could not be reached")
        }
        if kind == .upstreamTLS {
            return ("TLS", "The origin's TLS handshake failed")
        }
        if kind == .clientTLS {
            return ("TLS", "The client's TLS handshake failed")
        }
        if kind == .upstreamHTTP {
            return ("HTTP", "The origin's response could not be read")
        }
        if kind == .responseForwarding {
            return ("Forward", "The response could not be forwarded")
        }
        if kind == .clientDisconnected {
            return ("Closed", "The client disconnected")
        }
        if kind == .incomplete {
            return ("Partial", "The exchange did not finish")
        }
        if kind == .proxyFailure {
            return ("Failed", "The proxy failed")
        }
        return ("Failed", "The exchange failed")
    }

    private static func indicators(
        of summary: ExchangeSummary,
        width: TrafficRowWidth
    ) -> [Indicator] {
        var indicators: [Indicator] = []

        // The status column already says the word when there is no code.
        // Beside a code, the word is the only place that says which failure.
        if summary.status != nil, let error = summary.error {
            let failed = failure(error.kind)
            indicators.append(
                Indicator(
                    role: .failure,
                    label: failed.text,
                    help: failed.help,
                    spoken: failed.text
                )
            )
        }

        if isWebSocket(summary) {
            indicators.append(
                Indicator(
                    role: .websocket,
                    label: "WS",
                    help: "WebSocket handshake",
                    spoken: "WebSocket"
                )
            )
        } else if isServerSentEvents(summary) {
            indicators.append(
                Indicator(
                    role: .sse,
                    label: "SSE",
                    help: "Server-sent events",
                    spoken: "Server-sent events"
                )
            )
        } else if width != .narrow,
            let label = contentLabel(summary.responseContentType ?? summary.requestContentType)
        {
            let raw = summary.responseContentType ?? summary.requestContentType ?? label
            indicators.append(Indicator(role: .content, label: label, help: raw, spoken: label))
        }

        if let version = protocolIndicator(of: summary, width: width) {
            indicators.append(version)
        }

        return indicators
    }

    /// Status 101, which is the signal the WebSocket filter already uses.
    ///
    /// The summary has no `Upgrade` header. A handshake the filter would
    /// keep is a handshake the row marks, and the other way around.
    private static func isWebSocket(_ summary: ExchangeSummary) -> Bool {
        summary.status == 101
    }

    /// The same `text/event-stream` prefix the SSE filter matches.
    private static func isServerSentEvents(_ summary: ExchangeSummary) -> Bool {
        guard let type = summary.responseContentType else { return false }

        let essence = type.split(separator: ";", maxSplits: 1).first.map(String.init) ?? type
        return essence.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            .hasPrefix("text/event-stream")
    }

    /// A short content kind, or nil when the type would only add noise.
    ///
    /// Response type wins. JSON, HTML, and images are named; a handful of
    /// other common types get an equally short word; a vendor type is left
    /// off the row.
    private static func contentLabel(_ raw: String?) -> String? {
        guard let raw, let type = ContentType.parse(raw) else { return nil }
        if type.isJSON { return "JSON" }
        if type.isHTML { return "HTML" }
        if type.isImage { return "Image" }
        if type.isXML { return "XML" }

        switch type.essence {
        case "text/event-stream":
            return nil
        case "text/plain":
            return "Text"
        case "text/css":
            return "CSS"
        case "application/javascript", "text/javascript", "application/ecmascript":
            return "JS"
        case "application/x-www-form-urlencoded", "multipart/form-data":
            return "Form"
        case "application/pdf":
            return "PDF"
        case "application/octet-stream":
            return "Data"
        case "application/wasm":
            return "Wasm"
        case "font/woff", "font/woff2", "application/font-woff", "font/ttf", "font/otf":
            return "Font"
        default:
            return nil
        }
    }

    private static func protocolIndicator(
        of summary: ExchangeSummary,
        width: TrafficRowWidth
    ) -> Indicator? {
        let label = protocolLabel(request: summary.version, response: summary.responseVersion)
        switch width {
        case .narrow:
            return nil
        case .regular where label == "HTTP/1.1":
            return nil
        case .regular, .wide:
            return Indicator(
                role: .version,
                label: label,
                help: protocolHelp(request: summary.version, response: summary.responseVersion),
                spoken: label
            )
        }
    }

    private static func protocolLabel(request: HTTPVersion, response: HTTPVersion?) -> String {
        guard let response, response != request else { return name(request) }

        return "\(abbreviation(request))→\(abbreviation(response))"
    }

    private static func protocolHelp(request: HTTPVersion, response: HTTPVersion?) -> String {
        guard let response, response != request else { return name(request) }

        return "\(name(request)) request, \(name(response)) response"
    }

    /// The core spells HTTP/2 as `HTTP/2.0`. A row says `HTTP/2`.
    private static func name(_ version: HTTPVersion) -> String {
        switch version {
        case .http09: "HTTP/0.9"
        case .http10: "HTTP/1.0"
        case .http11: "HTTP/1.1"
        case .http2: "HTTP/2"
        case .http3: "HTTP/3"
        case .unknown: "HTTP"
        }
    }

    private static func abbreviation(_ version: HTTPVersion) -> String {
        switch version {
        case .http09: "0.9"
        case .http10: "1.0"
        case .http11: "1.1"
        case .http2: "2"
        case .http3: "3"
        case .unknown: "?"
        }
    }

    private static func accessibility(
        method: String,
        status: Status,
        host: String,
        path: String,
        indicators: [Indicator],
        duration: String?,
        time: String?
    ) -> String {
        var words = [method, host, path, status.spoken]
        words.append(contentsOf: indicators.filter { $0.role != .failure }.map(\.spoken))
        if let duration { words.append(duration) }
        if let time { words.append(time) }
        return words.joined(separator: ", ")
    }
}
