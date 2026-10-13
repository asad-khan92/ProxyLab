//  One message's headers, ready to draw and to copy.
//
//  Order is the order the message carried them in. The same name twice stays
//  twice. A search narrows that list and does not sort it. Values are the
//  values that were stored: this is not the place that masks a credential.
//  The authorization section is.

import Foundation

/// The headers of one message, in the order they arrived.
struct HeaderPresentation: Equatable, Sendable {
    /// One header, already turned into the strings the row draws and copies.
    struct Row: Equatable, Sendable, Identifiable {
        /// Where it sat in the message. Stable across a search.
        let index: Int
        let name: String
        /// The value as text, or a length when the value was not text.
        let value: String

        var id: Int { index }

        var copyName: String { name }
        var copyValue: String { value }
        var copyHeader: String { "\(name): \(value)" }
    }

    let rows: [Row]
    /// Every header, in order, in the form Copy All puts on the clipboard.
    let copyAll: String

    init(headers: Headers) {
        rows = headers.entries.enumerated().map { index, header in
            Row(index: index, name: header.name, value: Self.text(header.value))
        }
        copyAll = ExchangeText.headers(headers)
    }

    /// The rows whose name or value contains `query`, in their original order.
    ///
    /// An empty query is the whole list. Matching is case-insensitive and
    /// does not rebuild the rows.
    func matching(_ query: String) -> [Row] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return rows }

        return rows.filter {
            $0.name.localizedStandardContains(needle) || $0.value.localizedStandardContains(needle)
        }
    }

    private static func text(_ value: HeaderValue) -> String {
        switch value {
        case let .text(text): text
        case let .bytes(bytes): "<\(bytes.count) bytes, not text>"
        }
    }
}
