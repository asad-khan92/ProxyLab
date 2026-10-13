//  A failed exchange, as a person reads it.
//
//      TLS handshake failed
//      The client did not accept the proxy's certificate.
//
//      Stage         TLS
//      Destination   api.example.com:443
//      Duration      112 ms
//
//  The core's summary is a sentence written to be shown. A summary that
//  looks like a stack trace or a source path is not that sentence, and this
//  substitutes the kind's own description. The kind's wire name
//  (`client_tls`) is not a label.

import Foundation

/// Why an exchange failed, without the core's internal names.
struct FailurePresentation: Equatable, Sendable {
    struct Fact: Equatable, Sendable, Identifiable {
        let name: String
        let value: String
        var id: String { name }
    }

    /// A short title: "TLS handshake failed".
    let title: String
    /// The curated sentence, or this kind's own description.
    let detail: String
    let stage: String
    let facts: [Fact]

    init(exchange: TrafficExchange) {
        let error = exchange.error
        let kind = error?.kind
        title = Self.title(for: kind)
        stage = Self.stage(for: kind)
        detail = Self.detail(summary: error?.summary, kind: kind)

        var facts = [Fact(name: "Stage", value: stage)]
        if let destination = exchange.connection.destination {
            facts.append(
                Fact(name: "Destination", value: "\(destination.host):\(destination.port)")
            )
        }
        if let duration = TrafficDisplay.duration(milliseconds: exchange.timing.durationMilliseconds) {
            facts.append(Fact(name: "Duration", value: duration))
        }
        self.facts = facts
    }

    private static func title(for kind: TrafficErrorKind?) -> String {
        guard let kind else { return "Request failed" }

        if kind == .clientTLS || kind == .upstreamTLS { return "TLS handshake failed" }
        if kind == .upstreamConnect { return "Connection failed" }
        if kind == .requestRejected { return "Request rejected" }
        if kind == .proxyFailure { return "Proxy failed" }
        if kind == .upstreamHTTP { return "Response unreadable" }
        if kind == .responseForwarding { return "Response not forwarded" }
        if kind == .clientDisconnected { return "Client disconnected" }
        if kind == .incomplete { return "Exchange did not finish" }

        return "Request failed"
    }

    private static func stage(for kind: TrafficErrorKind?) -> String {
        guard let kind else { return "Failed" }

        if kind == .clientTLS || kind == .upstreamTLS { return "TLS" }
        if kind == .upstreamConnect { return "Connect" }
        if kind == .requestRejected { return "Request" }
        if kind == .proxyFailure { return "Proxy" }
        if kind == .upstreamHTTP { return "HTTP" }
        if kind == .responseForwarding { return "Forward" }
        if kind == .clientDisconnected { return "Client" }
        if kind == .incomplete { return "Incomplete" }

        return "Failed"
    }

    /// The summary, as a sentence, when it is one. Otherwise the description
    /// this kind already has, which does not name a file or a panic.
    private static func detail(summary: String?, kind: TrafficErrorKind?) -> String {
        let fallback = Self.fallback(for: kind)
        guard let summary else { return fallback }

        let trimmed = summary.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !looksInternal(trimmed, kind: kind) else { return fallback }

        return sentence(trimmed)
    }

    private static func fallback(for kind: TrafficErrorKind?) -> String {
        guard let kind else { return "The exchange failed." }

        if kind == .clientTLS { return "The client's TLS handshake failed." }
        if kind == .upstreamTLS { return "The origin's TLS handshake failed." }
        if kind == .upstreamConnect { return "The origin could not be reached." }
        if kind == .requestRejected { return "The proxy rejected the request." }
        if kind == .proxyFailure { return "The proxy could not carry out this request." }
        if kind == .upstreamHTTP { return "The origin's response could not be read." }
        if kind == .responseForwarding { return "The response could not be forwarded." }
        if kind == .clientDisconnected { return "The client disconnected." }
        if kind == .incomplete { return "The exchange did not finish." }

        return "The exchange failed."
    }

    /// A path, a panic, a stack, or the kind's own wire name.
    private static func looksInternal(_ text: String, kind: TrafficErrorKind?) -> Bool {
        if text.contains("\n") || text.contains("\t") { return true }
        if text.contains(".rs") { return true }
        if kind?.rawValue == text { return true }

        let lowered = text.lowercased()
        if lowered.contains("panicked") || lowered.contains("backtrace") { return true }
        if lowered.contains("stack trace") { return true }

        return text.count > 280
    }

    private static func sentence(_ text: String) -> String {
        var sentence = text
        if let first = sentence.first, first.isLowercase {
            sentence.replaceSubrange(
                sentence.startIndex...sentence.startIndex,
                with: String(first).uppercased()
            )
        }
        if let last = sentence.last, last.isLetter || last.isNumber {
            sentence.append(".")
        }

        return sentence
    }
}
