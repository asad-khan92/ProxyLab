//  The query string of a captured URL, read as the parameters it is.
//
//      ?page=2&tag=a&tag=b&flag&empty=&q=iPhone%2017
//       │      │           │     │      │
//       one    repeated    no =  empty  percent-encoded
//
//  Presentation only. The captured URL is never rewritten and never
//  re-encoded: what this produces is a second, read-only reading of it, and
//  every parameter keeps the spelling it arrived with beside the decoded form.
//
//  # Why `+` is a space
//
//  A query string is decoded by the server that receives it, and the query
//  parsers in the standard libraries a developer is debugging against — Go's
//  `url.ParseQuery`, Python's `parse_qsl`, Rack, `qs` — all read `+` as a
//  space. Showing `iPhone+17` where the server will read `iPhone 17` would be
//  showing the wire and calling it the value. The wire is still there, in
//  ``QueryParameter/rawValue``.

import Foundation

/// One parameter of a request's query string.
public struct QueryParameter: Equatable, Sendable, Identifiable {
    /// Where in the query string it was, which is what tells two parameters
    /// of the same name apart.
    public let position: Int
    /// The name, decoded.
    public let name: String
    /// The value, decoded — or `nil` when the parameter had no `=` at all,
    /// which is not the same thing as having an empty value.
    public let value: String?
    /// The name exactly as the URL spelled it.
    public let rawName: String
    /// The value exactly as the URL spelled it, when there was one.
    public let rawValue: String?

    public init(
        position: Int,
        name: String,
        value: String?,
        rawName: String,
        rawValue: String?
    ) {
        self.position = position
        self.name = name
        self.value = value
        self.rawName = rawName
        self.rawValue = rawValue
    }

    public var id: Int { position }

    /// Whether the decoded form differs from what was on the wire.
    ///
    /// What the viewer uses to decide whether showing both is worth the room.
    public var wasEncoded: Bool { name != rawName || value != rawValue }

    /// The parameter as `name=value`, in the decoded form this list shows.
    ///
    /// A parameter that had no `=` is the name alone, which is not the same
    /// thing as a parameter whose value was empty (`name=`).
    public var parameterText: String {
        guard let value else { return name }

        return "\(name)=\(value)"
    }
}

/// Reading a captured URL's query string.
public enum QueryParameters {
    /// The parameters of `url`, in the order it carried them.
    ///
    /// Empty for a URL with no query string, and empty — rather than a guess —
    /// for a string that has no query string to find.
    public static func of(url: String) -> [QueryParameter] {
        guard let query = queryString(of: url), !query.isEmpty else { return [] }

        return parse(query)
    }

    /// The raw query string of `url`, without the `?` and without the
    /// fragment, or `nil` if there is none.
    ///
    /// Scanned rather than taken from `URLComponents`, which normalises what
    /// it gives back — a lone `%` in a value nobody encoded properly comes out
    /// of it as `%25`, which is not what the client sent and not what the
    /// server received. It also parses only absolute URLs, and the core does
    /// capture request targets that are not.
    static func queryString(of url: String) -> String? {
        // A `?` inside a fragment is part of the fragment, so the fragment
        // goes first.
        let addressed = url.firstIndex(of: "#").map { String(url[url.startIndex..<$0]) } ?? url
        guard let mark = addressed.firstIndex(of: "?") else { return nil }

        return String(addressed[addressed.index(after: mark)...])
    }

    /// `query` split into its parameters.
    private static func parse(_ query: String) -> [QueryParameter] {
        var parameters: [QueryParameter] = []

        for piece in query.split(separator: "&", omittingEmptySubsequences: true) {
            let piece = String(piece)
            // Only the first `=` separates: a value may contain one, and an
            // unencoded `=` inside a base64 value is ordinary.
            let rawName: String
            let rawValue: String?

            if let equals = piece.firstIndex(of: "=") {
                rawName = String(piece[piece.startIndex..<equals])
                rawValue = String(piece[piece.index(after: equals)...])
            } else {
                rawName = piece
                rawValue = nil
            }

            parameters.append(
                QueryParameter(
                    position: parameters.count,
                    name: decode(rawName),
                    value: rawValue.map(decode),
                    rawName: rawName,
                    rawValue: rawValue
                )
            )
        }

        return parameters
    }

    /// One name or value, decoded.
    ///
    /// A percent escape that is not one — a stray `%` in a value nobody
    /// encoded, or bytes that are not UTF-8 — leaves the piece exactly as it
    /// arrived. Showing the wire is honest; showing nothing is not.
    static func decode(_ piece: String) -> String {
        let spaced = piece.replacingOccurrences(of: "+", with: " ")

        return spaced.removingPercentEncoding ?? spaced
    }
}
