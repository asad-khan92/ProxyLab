//  How a held exchange is named, before any view draws it.
//
//      ⏸ Request Paused
//      ⏸ Response Paused
//      ⏸ 3 paused
//
//  The words are the same in the traffic list, the editor, and the toolbar.
//  A held exchange is not a completed row, and it is not "pending": pending
//  is an exchange the list is still waiting to finish.

import Foundation

/// The words for one hold, and for the count of them.
enum PausedHoldLabel {
    /// "1 paused" or "3 paused".
    static func count(_ count: Int) -> String {
        count == 1 ? "1 paused" : "\(count) paused"
    }

    /// Which half is held, as the row and the editor title it.
    static func phase(_ direction: BreakpointDirection) -> String {
        switch direction {
        case .request: "Request Paused"
        case .response: "Response Paused"
        case .websocket: "WebSocket Paused"
        }
    }
}

/// One held exchange, as the traffic list draws it.
struct PausedTrafficRowContent: Equatable, Sendable {
    var title: String
    var detail: String
    var help: String
    var accessibilityLabel: String

    init(_ exchange: PausedExchange) {
        title = PausedHoldLabel.phase(exchange.direction)
        let host = TrafficDisplay.host(url: exchange.url, destinationHost: nil)
        let path = TrafficDisplay.path(exchange.url)
        let hostText = host == "—" ? "" : host
        detail = [exchange.method, hostText, path]
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        help = "Held until you continue or drop it."
        accessibilityLabel = "\(title), \(detail)"
    }
}
