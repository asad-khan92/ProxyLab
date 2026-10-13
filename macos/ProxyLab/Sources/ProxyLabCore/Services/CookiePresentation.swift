//  Cookie and Set-Cookie, read as a list.
//
//      Cookie: session_id=abc; locale=en
//      Set-Cookie: session_id=abc; Path=/; Secure
//
//  Presentation only. Nothing is stored, nothing is sent back, and a value
//  is the characters the header carried. A cookie this parser cannot read
//  stays in the header list, which is the unparsed message.

import Foundation

/// The cookies on one message.
struct CookiePresentation: Equatable, Sendable {
    /// One attribute of a `Set-Cookie`, or nothing on a request cookie.
    struct Attribute: Equatable, Sendable, Identifiable {
        let id: Int
        let name: String
        let value: String
    }

    /// One cookie, in the order the headers carried them.
    struct Cookie: Equatable, Sendable, Identifiable {
        let id: Int
        let name: String
        let value: String
        let attributes: [Attribute]

        /// `name=value`, which is the cookie and not its attributes.
        var parameterText: String { "\(name)=\(value)" }
    }

    let cookies: [Cookie]

    var isEmpty: Bool { cookies.isEmpty }

    /// Every `Cookie` header, split into pairs. Repeated names stay repeated.
    static func request(headers: Headers) -> CookiePresentation {
        var cookies: [Cookie] = []

        for header in headers.entries where header.name.caseInsensitiveCompare("Cookie") == .orderedSame {
            guard let text = header.value.text else { continue }

            for piece in split(text) {
                guard let pair = pair(piece) else { continue }

                cookies.append(
                    Cookie(id: cookies.count, name: pair.name, value: pair.value, attributes: [])
                )
            }
        }

        return CookiePresentation(cookies: cookies)
    }

    /// Every `Set-Cookie` header. The first pair is the cookie. The rest are
    /// attributes. A flag with no `=` is present, which this shows as `true`.
    static func response(headers: Headers) -> CookiePresentation {
        var cookies: [Cookie] = []

        for header in headers.entries
        where header.name.caseInsensitiveCompare("Set-Cookie") == .orderedSame {
            guard let text = header.value.text else { continue }

            let pieces = split(text)
            guard let first = pieces.first, let cookie = pair(first) else { continue }

            var attributes: [Attribute] = []
            for piece in pieces.dropFirst() {
                let part = attribute(piece)
                attributes.append(Attribute(id: attributes.count, name: part.name, value: part.value))
            }

            cookies.append(
                Cookie(
                    id: cookies.count,
                    name: cookie.name,
                    value: cookie.value,
                    attributes: attributes
                )
            )
        }

        return CookiePresentation(cookies: cookies)
    }

    /// Semicolons separate pairs. A semicolon inside a cookie value is not
    /// something this header syntax allows, so the split is the syntax.
    private static func split(_ text: String) -> [String] {
        text.split(separator: ";", omittingEmptySubsequences: true).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }.filter { !$0.isEmpty }
    }

    private static func pair(_ piece: String) -> (name: String, value: String)? {
        guard let equals = piece.firstIndex(of: "=") else { return nil }

        let name = piece[..<equals].trimmingCharacters(in: .whitespacesAndNewlines)
        let value = piece[piece.index(after: equals)...].trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return nil }

        return (name, value)
    }

    private static func attribute(_ piece: String) -> (name: String, value: String) {
        guard let equals = piece.firstIndex(of: "=") else {
            return (piece, "true")
        }

        let name = piece[..<equals].trimmingCharacters(in: .whitespacesAndNewlines)
        let value = piece[piece.index(after: equals)...].trimmingCharacters(in: .whitespacesAndNewlines)

        return (name.isEmpty ? piece : name, value)
    }
}
