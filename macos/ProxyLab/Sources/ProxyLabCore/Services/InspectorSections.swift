//  Which faces and sections the detail pane offers for one exchange.
//
//      Request | Response | Messages
//      Overview  Headers  Query  Body  Cookies  Auth  Raw
//
//  Messages exists only for a WebSocket handshake or an event stream. The
//  other sections are always there: an empty query is a fact, and hiding the
//  section would make the next exchange jump to a different place.

import CoreGraphics
import Foundation

/// The top of the inspector: one half of the exchange, or its stream.
enum InspectorFace: String, Identifiable, CaseIterable {
    case request = "Request"
    case response = "Response"
    case messages = "Messages"

    var id: Self { self }
}

/// The sections of the request half, in the order the rail shows them.
enum RequestSection: String, Identifiable, CaseIterable {
    case overview = "Overview"
    case headers = "Headers"
    case query = "Query"
    case body = "Body"
    case cookies = "Cookies"
    case auth = "Auth"
    case raw = "Raw"

    var id: Self { self }
}

/// The sections of the response half, in the order the rail shows them.
enum ResponseSection: String, Identifiable, CaseIterable {
    case overview = "Overview"
    case headers = "Headers"
    case body = "Body"
    case cookies = "Cookies"
    case raw = "Raw"

    var id: Self { self }
}

/// Which faces an exchange has, and how narrow the pane has to be before a
/// key sits above its value.
enum InspectorSections {
    /// Request and Response always. Messages when this exchange has a stream.
    static func faces(of exchange: TrafficExchange) -> [InspectorFace] {
        var faces: [InspectorFace] = [.request, .response]
        if exchange.carriesStreamMessages {
            faces.append(.messages)
        }

        return faces
    }
}

/// Widths the section rail and the key column agree on.
enum InspectorMetrics {
    /// The section names, and the padding around them.
    static let railWidth: CGFloat = 112
    /// A key label. It does not grow when the pane does.
    static let keyWidth: CGFloat = 128
    /// Below this, the content column stacks a key above its value.
    static let stackBreakpoint: CGFloat = 300

    static func contentWidth(paneWidth: CGFloat) -> CGFloat {
        max(0, paneWidth - railWidth)
    }

    static func stacksKeys(contentWidth: CGFloat) -> Bool {
        contentWidth < stackBreakpoint
    }
}
