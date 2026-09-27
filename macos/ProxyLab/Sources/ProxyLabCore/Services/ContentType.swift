//  What a message said its body is.
//
//      Content-Type: application/json; charset=utf-8
//                    │          │       │
//                    type       subtype parameters
//
//  Parsed for one purpose: deciding how to *show* a body. Nothing here changes
//  a body, and nothing here invents a type — a message that declared none has
//  none, and the viewer sniffs the bytes instead of being told a guess.
//
//  The header is kept exactly as it was spelled, because a debugging proxy is
//  a place where the difference between `application/json` and
//  `Application/JSON;charset=UTF8` is occasionally the bug. The parsed pieces
//  are lowercased, because a comparison is not a display.

import Foundation

/// One message's declared media type.
public struct ContentType: Equatable, Sendable {
    /// The header value exactly as the message carried it.
    public let raw: String
    /// The type, lowercased: `application` in `application/json`.
    public let type: String
    /// The subtype, lowercased: `json` in `application/json`.
    public let subtype: String
    /// The parameters after the type, in the order they were written.
    public let parameters: [Parameter]

    /// One `name=value` after the media type.
    public struct Parameter: Equatable, Sendable {
        public let name: String
        public let value: String

        public init(name: String, value: String) {
            self.name = name
            self.value = value
        }
    }

    public init(raw: String, type: String, subtype: String, parameters: [Parameter] = []) {
        self.raw = raw
        self.type = type
        self.subtype = subtype
        self.parameters = parameters
    }

    /// The type and subtype without the parameters: `application/json`.
    public var essence: String { "\(type)/\(subtype)" }

    /// The value of `name`, matched without regard to case.
    public func parameter(_ name: String) -> String? {
        parameters.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value
    }

    /// The declared character set, if the message declared one.
    public var charset: String? { parameter("charset") }

    /// The structured-syntax suffix: `json` in `application/vnd.api+json`.
    ///
    /// Registered media types name their underlying syntax this way, and a
    /// body that is `application/problem+json` is JSON however unfamiliar the
    /// first half of its name is.
    public var suffix: String? {
        guard let plus = subtype.lastIndex(of: "+") else { return nil }

        return String(subtype[subtype.index(after: plus)...])
    }

    /// Whether the declared type is JSON.
    public var isJSON: Bool { essence == "application/json" || suffix == "json" }

    /// Whether the declared type is XML.
    public var isXML: Bool {
        essence == "application/xml" || essence == "text/xml" || suffix == "xml"
    }

    /// Whether the declared type is HTML.
    public var isHTML: Bool { essence == "text/html" || essence == "application/xhtml+xml" }

    /// Whether the declared type is one a person reads as text.
    ///
    /// `text/*` by definition, and the several `application/*` types that are
    /// text in everything but their registration.
    public var isTextual: Bool {
        if type == "text" { return true }
        if isJSON || isXML || isHTML { return true }

        switch essence {
        case "application/javascript", "application/ecmascript",
            "application/x-www-form-urlencoded", "application/graphql",
            "application/x-ndjson", "application/yaml", "application/x-yaml":
            return true
        default:
            return false
        }
    }

    /// Whether the declared type is an image.
    public var isImage: Bool { type == "image" }

    /// The character encoding to decode this body with, when one was declared
    /// and this system knows it.
    ///
    /// Resolved through Core Foundation's IANA table rather than a hand-written
    /// list of names, so `utf-8`, `iso-8859-1`, `windows-1252`, and
    /// `shift_jis` all arrive at the encoding they name without this file
    /// having to know any of them.
    public var declaredEncoding: String.Encoding? {
        guard let charset, !charset.isEmpty else { return nil }

        let cf = CFStringConvertIANACharSetNameToEncoding(charset as CFString)
        guard cf != kCFStringEncodingInvalidId else { return nil }

        return String.Encoding(rawValue: CFStringConvertEncodingToNSStringEncoding(cf))
    }
}

extension ContentType {
    /// The `Content-Type` of a message, or `nil` if it declared none.
    ///
    /// A header whose value was not text has no media type to read: the value
    /// is given back as it is by ``Headers``, and this does not decode it.
    public static func of(_ headers: Headers) -> ContentType? {
        guard let value = headers.text("Content-Type") else { return nil }

        return parse(value)
    }

    /// `value` as a media type, or `nil` if there is no type in it.
    public static func parse(_ value: String) -> ContentType? {
        let raw = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return nil }

        let pieces = split(raw)
        let media = pieces[0].trimmingCharacters(in: .whitespaces).lowercased()
        guard let slash = media.firstIndex(of: "/") else { return nil }

        let type = String(media[media.startIndex..<slash])
        let subtype = String(media[media.index(after: slash)...])
        guard !type.isEmpty, !subtype.isEmpty else { return nil }

        return ContentType(
            raw: raw,
            type: type,
            subtype: subtype,
            parameters: pieces.dropFirst().compactMap(parameter)
        )
    }

    /// `raw` split on the semicolons that separate its parameters, ignoring
    /// the ones inside a quoted value.
    private static func split(_ raw: String) -> [String] {
        var pieces: [String] = []
        var current = ""
        var quoted = false
        var escaped = false

        for character in raw {
            if escaped {
                current.append(character)
                escaped = false
                continue
            }

            switch character {
            case "\\" where quoted:
                current.append(character)
                escaped = true
            case "\"":
                quoted.toggle()
                current.append(character)
            case ";" where !quoted:
                pieces.append(current)
                current = ""
            default:
                current.append(character)
            }
        }
        pieces.append(current)

        return pieces
    }

    /// One `name=value` piece, unquoted.
    private static func parameter(_ piece: String) -> Parameter? {
        guard let equals = piece.firstIndex(of: "=") else { return nil }

        let name = piece[piece.startIndex..<equals]
            .trimmingCharacters(in: .whitespaces)
            .lowercased()
        guard !name.isEmpty else { return nil }

        var value = piece[piece.index(after: equals)...].trimmingCharacters(in: .whitespaces)
        if value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\"") {
            value = String(value.dropFirst().dropLast())
                .replacingOccurrences(of: "\\\"", with: "\"")
        }

        return Parameter(name: name, value: value)
    }
}
