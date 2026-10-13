//  What the inspector's action menu can do with the exchange it is showing.
//
//  The same decisions as the traffic row's menu. Replay, Copy as cURL, and
//  Save Request are that menu's. Apply Rule is the same list of actions the
//  row offers. This does not grow a second replay or a second clipboard.

import Foundation

/// Whether the inspector offers each action for one exchange.
struct InspectorActions: Equatable, Sendable {
    let canReplay: Bool
    let canCopyCurl: Bool
    let canSave: Bool
    let canCopyURL: Bool
    /// Breakpoint… is the row menu's, for the next match.
    let canBreakpoint: Bool
    /// Apply Rule is offered for every exchange the row menu offers it for.
    let canApplyRule: Bool

    init(summary: ExchangeSummary) {
        let menu = TrafficRowMenu(summary: summary, isPinned: false)
        canReplay = menu.canReplay
        canCopyCurl = menu.canCopyCurl
        canSave = menu.canSave
        canCopyURL = !summary.url.isEmpty
        canBreakpoint = menu.canBreakpoint
        canApplyRule = true
    }
}

extension ExchangeSummary {
    /// The list-row shape of a stored exchange, for the action menu.
    ///
    /// The row is preferred when it is on the current page. This exists for
    /// the moment it is not: the menu reads a summary, and the detail pane
    /// has the exchange.
    init(exchange: TrafficExchange) {
        self.init(
            id: exchange.id,
            startedAt: exchange.timing.startedAt,
            durationMicros: exchange.timing.completedMicros,
            client: exchange.connection.client,
            scheme: exchange.connection.scheme,
            destination: exchange.connection.destination,
            method: exchange.request.method,
            url: exchange.request.url,
            version: exchange.request.version,
            status: exchange.response?.status,
            reason: exchange.response?.reason,
            responseVersion: exchange.response?.version,
            requestContentType: exchange.request.headers.text("Content-Type"),
            responseContentType: exchange.response?.headers.text("Content-Type"),
            completed: exchange.timing.completedMicros != nil,
            error: exchange.error
        )
    }
}
