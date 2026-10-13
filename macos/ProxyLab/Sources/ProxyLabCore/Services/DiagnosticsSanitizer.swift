//  Text that is safe to show and safe to paste into an issue.
//
//  A diagnostics report travels further than the machine it was written on.
//  What it may carry is the shape of a failure — which part, what it was
//  doing, what the operating system said — and not the traffic that failure
//  was about. The same rules are applied to a line the core already scrubbed,
//  because applying them twice changes nothing and applying them once is what
//  keeps a path or a header that this process added from leaving it.

import Foundation

/// Removes secrets, query strings, and personal paths from diagnostic text.
public enum DiagnosticsSanitizer {
    /// The longest message kept. A longer one is a body, not a diagnosis.
    public static let maxMessageChars = 240

    private static let redacted = "[redacted]"
    private static let pathMarker = "[path]"
    private static let headerNames = [
        "authorization:",
        "proxy-authorization:",
        "cookie:",
        "set-cookie:",
        "x-api-key:",
        "api-key:",
    ]
    private static let assignmentKeys = [
        "password=",
        "passwd=",
        "token=",
        "secret=",
        "api_key=",
        "apikey=",
        "access_token=",
    ]

    /// `text` with secrets, query strings, and personal paths removed.
    public static func sanitize(_ text: String) -> String {
        let withoutKeys = redactPEM(text)
        let withoutHeaders = redactHeaderLines(withoutKeys)
        let withoutSchemes = redactPrefixed(
            redactPrefixed(withoutHeaders, prefix: "bearer "),
            prefix: "basic "
        )
        var out = ""
        out.reserveCapacity(withoutSchemes.count)

        for token in splitKeepingWhitespace(withoutSchemes) {
            if token.allSatisfy(\.isWhitespace) {
                out.append(token)
            } else {
                out.append(sanitizeToken(token))
            }
        }

        return truncate(out)
    }

    // MARK: - Tokens

    private static func sanitizeToken(_ token: String) -> String {
        if token.contains("ca-key") { return redacted }
        if looksLikePath(token) { return pathMarker }

        var token = redactUserinfo(token)
        if let query = token.firstIndex(of: "?") {
            token = String(token[..<query])
        }
        redactAssignments(&token)
        return token
    }

    private static func looksLikePath(_ token: String) -> Bool {
        token.hasPrefix("/Users/")
            || token.hasPrefix("/home/")
            || token.hasPrefix("/private/var/folders/")
            || token.hasPrefix("/var/folders/")
    }

    private static func redactUserinfo(_ token: String) -> String {
        if let scheme = token.range(of: "://") {
            let authorityStart = scheme.upperBound
            let authorityEnd = token[authorityStart...].firstIndex(where: { character in
                character == "/" || character == "?" || character == "#"
            }) ?? token.endIndex
            let authority = token[authorityStart..<authorityEnd]
            guard let at = authority.lastIndex(of: "@") else { return token }

            var out = String(token[..<authorityStart])
            out.append(redacted)
            out.append("@")
            out.append(contentsOf: authority[authority.index(after: at)...])
            out.append(contentsOf: token[authorityEnd...])
            return out
        }

        guard let at = token.lastIndex(of: "@") else { return token }
        guard token[..<at].contains(":") else { return token }

        return redacted + "@" + token[token.index(after: at)...]
    }

    private static func redactAssignments(_ token: inout String) {
        for key in assignmentKeys {
            guard let start = firstRange(of: key, in: token) else { continue }

            let valueStart = start.upperBound
            let end = token[valueStart...].firstIndex(where: { character in
                character == "&" || character == " " || character == "\t"
            }) ?? token.endIndex
            token.replaceSubrange(valueStart..<end, with: redacted)
            return
        }
    }

    // MARK: - Whole lines

    private static func redactHeaderLines(_ text: String) -> String {
        text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline)
            .map { redactHeaderLine(String($0)) }
            .joined(separator: "\n")
    }

    private static func redactHeaderLine(_ line: String) -> String {
        var found: Range<String.Index>?
        for name in headerNames {
            guard let range = firstRange(of: name, in: line) else { continue }
            if found == nil || range.lowerBound < found!.lowerBound {
                found = range
            }
        }

        guard let found else { return line }

        return String(line[..<found.upperBound]) + " " + redacted
    }

    private static func redactPrefixed(_ text: String, prefix: String) -> String {
        var out = ""
        var rest = text

        while let start = firstRange(of: prefix, in: rest) {
            let after = start.upperBound
            out.append(contentsOf: rest[..<after])
            let tail = rest[after...]
            if let space = tail.firstIndex(where: \.isWhitespace) {
                out.append(redacted)
                rest = String(tail[space...])
            } else {
                out.append(redacted)
                rest = ""
            }
        }

        out.append(rest)
        return out
    }

    /// The first case-insensitive match of `needle`, as indexes of `text`.
    private static func firstRange(of needle: String, in text: String) -> Range<String.Index>? {
        let needle = needle.lowercased()
        guard !needle.isEmpty else { return nil }

        var index = text.startIndex
        while index < text.endIndex {
            guard let end = text.index(index, offsetBy: needle.count, limitedBy: text.endIndex) else {
                return nil
            }
            if text[index..<end].lowercased() == needle {
                return index..<end
            }
            index = text.index(after: index)
        }

        return nil
    }

    private static func redactPEM(_ text: String) -> String {
        var out = ""
        var rest = Substring(text)

        while let start = rest.range(of: "-----BEGIN") {
            out.append(contentsOf: rest[..<start.lowerBound])
            let after = rest[start.lowerBound...]
            guard let end = after.range(of: "-----END") else {
                out.append(redacted)
                return out
            }

            let tail = after[end.lowerBound...]
            if let newline = tail.firstIndex(of: "\n") {
                out.append(redacted)
                rest = after[after.index(after: newline)...]
            } else {
                out.append(redacted)
                return out
            }
        }

        out.append(contentsOf: rest)
        return out
    }

    private static func truncate(_ text: String) -> String {
        guard text.count > maxMessageChars else { return text }

        return String(text.prefix(maxMessageChars)) + "…"
    }

    private static func splitKeepingWhitespace(_ text: String) -> [String] {
        guard let first = text.first else { return [] }

        var parts: [String] = []
        var start = text.startIndex
        var inSpace = first.isWhitespace
        var index = text.startIndex

        while index < text.endIndex {
            let space = text[index].isWhitespace
            if space != inSpace {
                parts.append(String(text[start..<index]))
                start = index
                inSpace = space
            }
            index = text.index(after: index)
        }

        if start < text.endIndex {
            parts.append(String(text[start...]))
        }

        return parts
    }
}
