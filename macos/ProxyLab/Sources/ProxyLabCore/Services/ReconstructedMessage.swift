//  A request or a response, reassembled so it can be read as a message.
//
//      GET /v1/users?page=2 HTTP/1.1
//      Accept: application/json
//
//  ProxyLab does not keep the bytes that were on the wire. HTTP/2 has no
//  start line, header names keep whatever case the sender used, and a body
//  is the capture rather than a chunked frame. This says so. The body itself
//  stays in the body section: copying it into this string would be a second
//  copy of a payload the viewer already holds.

import Foundation

/// A message assembled from the stored exchange. Not the original wire bytes.
struct ReconstructedMessage: Equatable, Sendable {
    /// What to call it. Never "raw wire".
    static let label = "Reconstructed"

    let note: String
    let text: String

    /// The request: start line, headers, and a note where the body is.
    static func request(_ exchange: TrafficExchange, inspection: BodyInspection) -> ReconstructedMessage {
        let version = ExchangeVersions.name(exchange.request.version)
        let target = TrafficDisplay.path(exchange.request.url)
        let start = "\(exchange.request.method) \(target) \(version)"

        return ReconstructedMessage(
            note: note,
            text: message(start: start, headers: exchange.request.headers, body: inspection)
        )
    }

    /// The response, or nil when the origin never sent one.
    static func response(
        _ response: CapturedResponse,
        inspection: BodyInspection?
    ) -> ReconstructedMessage {
        let version = ExchangeVersions.name(response.version)
        let status = TrafficDisplay.status(response.status, reason: response.reason)
        let start = "\(version) \(status)"

        return ReconstructedMessage(
            note: note,
            text: message(start: start, headers: response.headers, body: inspection)
        )
    }

    private static let note =
        "Reconstructed from the stored method, target, version, and headers. Not the original wire bytes."

    private static func message(start: String, headers: Headers, body: BodyInspection?) -> String {
        var lines = [start]
        let block = ExchangeText.headers(headers)
        if !block.isEmpty {
            lines.append(block)
        }
        if let body, !body.isEmpty {
            lines.append("")
            var line = "[Body, \(TrafficDisplay.size(body.capture.size)) — shown in Body]"
            if body.capture.truncated {
                line += " Truncated."
            }
            lines.append(line)
        }

        return lines.joined(separator: "\n")
    }
}
