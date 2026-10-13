//  What kind of authorization a request carried, without the credential.
//
//      Authorization    Bearer ••••••••
//      Scheme           Bearer
//
//  The header list still shows the value the message carried. This section
//  does not copy it out a second time. The token is read only long enough
//  to decide that there was one, and it is not kept on the value this draws.

import Foundation

/// The authorization headers of one request, as scheme and a mask.
struct AuthorizationPresentation: Equatable, Sendable {
    struct Entry: Equatable, Sendable, Identifiable {
        let id: Int
        /// The header name, spelled as the message spelled it.
        let header: String
        /// `Bearer`, `Basic`, or `Unspecified` when the value named no scheme.
        let scheme: String
        /// The scheme and a mask. Never the credential.
        let summary: String
    }

    let entries: [Entry]

    var isEmpty: Bool { entries.isEmpty }

    /// `Authorization` and `Proxy-Authorization`, in the order they arrived.
    init(headers: Headers) {
        var entries: [Entry] = []

        for header in headers.entries where Self.isAuthorization(header.name) {
            guard let text = header.value.text else { continue }

            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { continue }

            let parsed = Self.parse(trimmed)
            entries.append(
                Entry(
                    id: entries.count,
                    header: header.name,
                    scheme: parsed.scheme,
                    summary: parsed.summary
                )
            )
        }

        self.entries = entries
    }

    private static func isAuthorization(_ name: String) -> Bool {
        name.caseInsensitiveCompare("Authorization") == .orderedSame
            || name.caseInsensitiveCompare("Proxy-Authorization") == .orderedSame
    }

    /// A single token is the credential: showing it as the scheme would
    /// publish it. A scheme with nothing after it has no credential to mask.
    private static func parse(_ value: String) -> (scheme: String, summary: String) {
        let parts = value.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
        guard parts.count > 1 else {
            return ("Unspecified", mask)
        }

        let scheme = String(parts[0])
        let credential = parts[1].trimmingCharacters(in: .whitespacesAndNewlines)
        guard !credential.isEmpty else {
            return (scheme, scheme)
        }

        return (scheme, "\(scheme) \(mask)")
    }

    private static let mask = "••••••••"
}
