//  The words an empty pane uses.
//
//  Traffic, rules, breakpoints, saved requests, and domains each say the one
//  thing that is true: nothing captured yet, nothing matches, or the list
//  could not be loaded. The views draw these. They do not invent a second
//  sentence.

import Foundation

/// What the traffic list says when it has no rows.
enum TrafficEmptyCopy {
    struct Content: Equatable {
        var symbol: String
        var title: String
        var detail: String
        var action: Action?
    }

    enum Action: Equatable {
        case retryConnection
        case reload
        case clearFilters
        case showAllTraffic

        var title: String {
            switch self {
            case .retryConnection, .reload: "Try Again"
            case .clearFilters: "Clear Filters"
            case .showAllTraffic: "Show All Traffic"
            }
        }
    }

    /// `nil` when the list has rows, or is still loading and shows a spinner.
    static func content(_ placeholder: TrafficListPlaceholder) -> Content? {
        switch placeholder {
        case .rows, .loading:
            return nil
        case let .disconnected(reason):
            return Content(
                symbol: "bolt.horizontal.circle",
                title: "Not connected to the core",
                detail: UserFacingNotice(reason ?? "The ProxyLab core is not answering.").summary,
                action: .retryConnection
            )
        case let .failed(reason):
            return Content(
                symbol: "exclamationmark.triangle",
                title: "Traffic could not be loaded",
                detail: UserFacingNotice(reason).summary,
                action: .reload
            )
        case .noTraffic:
            return Content(
                symbol: "arrow.left.arrow.right",
                title: "No traffic yet",
                detail: "Requests that go through the proxy appear here as they finish.",
                action: nil
            )
        case .noMatches:
            return Content(
                symbol: "line.3.horizontal.decrease.circle",
                title: "No traffic matches these filters",
                detail: "The core has traffic, but none of it matches what you are looking for.",
                action: .clearFilters
            )
        case let .noHostTraffic(host):
            return Content(
                symbol: "pin",
                title: "No traffic for \(host)",
                detail: "Nothing has gone through the proxy for this domain yet. "
                    + "It stays pinned either way.",
                action: .showAllTraffic
            )
        }
    }
}

/// What the breakpoints list says when it has no rows.
enum BreakpointEmptyCopy {
    static let title = "No breakpoints"
    static let detail =
        "A breakpoint holds a matching request or response still so you can change it "
        + "before it goes on. Everything else is carried as usual."
}

/// What the saved-request list says when it has no rows.
enum SavedEmptyCopy {
    static let title = "No saved requests"
    static let detail = "Select a request and choose Save Request."
}

/// What the domain list says when it has nothing to draw.
enum DomainEmptyCopy {
    static let noneCaptured = "No domains captured yet"
    static let noMatches = "No domains match."
}
