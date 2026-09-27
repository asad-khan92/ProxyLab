//  A JSON value as a tree, preserving the order the sender wrote.
//
//  Foundation can parse JSON, but an object becomes a dictionary and the keys
//  come out in hash order. This walks the bytes the same way `JSONPrettyPrinter`
//  does: strings, numbers, and literals are copied through, and objects and
//  arrays keep their members in order so a collapsed node still names what
//  was sent.

import Foundation

/// One node of a JSON document, for a collapsible viewer.
public struct JSONNode: Identifiable, Equatable, Sendable {
    public enum Kind: String, Equatable, Sendable {
        case object
        case array
        case string
        case number
        case literal
    }

    /// Stable within one tree: the path from the root.
    public let id: String
    /// The object key, when this node is a member of an object.
    public let key: String?
    /// What kind of JSON value this is.
    public let kind: Kind
    /// The value as it was written, including quotes around a string.
    public let raw: String
    /// Members, in the order they were written.
    public let children: [JSONNode]

    public init(id: String, key: String?, kind: Kind, raw: String, children: [JSONNode]) {
        self.id = id
        self.key = key
        self.kind = kind
        self.raw = raw
        self.children = children
    }

    /// A short label for a collapsed row.
    public var preview: String {
        switch kind {
        case .object: "{\(children.count)}"
        case .array: "[\(children.count)]"
        case .string, .number, .literal: raw
        }
    }

    /// Compact JSON for this node, for copying a subtree.
    public var serialized: String {
        switch kind {
        case .object:
            let members = children.map { child in
                "\"\(child.key ?? "")\": \(child.serialized)"
            }
            return "{\(members.joined(separator: ", "))}"
        case .array:
            return "[\(children.map(\.serialized).joined(separator: ", "))]"
        case .string, .number, .literal:
            return raw
        }
    }

    /// What "Copy Value" puts on the clipboard.
    ///
    /// A string is copied without its surrounding quotes; everything else is
    /// the JSON token as it was sent.
    public var copyableValue: String {
        guard kind == .string, raw.count >= 2, raw.first == "\"", raw.last == "\"" else {
            return serialized
        }

        return String(raw.dropFirst().dropLast())
    }

    /// `data` as a tree, or `nil` if it is not UTF-8 JSON this walker can read.
    public static func parse(_ data: Data) -> JSONNode? {
        guard let source = String(data: data, encoding: .utf8) else { return nil }

        var parser = Parser(characters: Array(source))
        guard let node = try? parser.parseValue(path: "$", key: nil) else { return nil }
        parser.skipWhitespace()
        guard parser.index >= parser.characters.count else { return nil }

        return node
    }
}

private struct Parser {
    let characters: [Character]
    var index = 0

    mutating func parseValue(path: String, key: String?) throws -> JSONNode {
        skipWhitespace()
        guard index < characters.count else { throw ParseError.truncated }

        switch characters[index] {
        case "{":
            return try parseObject(path: path, key: key)
        case "[":
            return try parseArray(path: path, key: key)
        case "\"":
            let raw = takeString()
            return JSONNode(id: path, key: key, kind: .string, raw: raw, children: [])
        case "-", "0"..."9":
            let raw = takeNumber()
            return JSONNode(id: path, key: key, kind: .number, raw: raw, children: [])
        default:
            let raw = takeLiteral()
            guard raw == "true" || raw == "false" || raw == "null" else {
                throw ParseError.truncated
            }
            return JSONNode(id: path, key: key, kind: .literal, raw: raw, children: [])
        }
    }

    mutating func parseObject(path: String, key: String?) throws -> JSONNode {
        index += 1
        skipWhitespace()
        var children: [JSONNode] = []
        var member = 0

        if index < characters.count, characters[index] == "}" {
            index += 1
            return JSONNode(id: path, key: key, kind: .object, raw: "{}", children: [])
        }

        while index < characters.count {
            skipWhitespace()
            let name = takeString()
            skipWhitespace()
            if index < characters.count, characters[index] == ":" { index += 1 }
            let decodedKey = String(name.dropFirst().dropLast())
            let child = try parseValue(path: "\(path).\(decodedKey)", key: decodedKey)
            children.append(child)
            member += 1
            skipWhitespace()
            guard index < characters.count else { break }
            if characters[index] == "," {
                index += 1
                continue
            }
            if characters[index] == "}" {
                index += 1
                break
            }
            break
        }

        return JSONNode(id: path, key: key, kind: .object, raw: "{\(member)}", children: children)
    }

    mutating func parseArray(path: String, key: String?) throws -> JSONNode {
        index += 1
        skipWhitespace()
        var children: [JSONNode] = []

        if index < characters.count, characters[index] == "]" {
            index += 1
            return JSONNode(id: path, key: key, kind: .array, raw: "[]", children: [])
        }

        var member = 0
        while index < characters.count {
            let child = try parseValue(path: "\(path)[\(member)]", key: nil)
            children.append(child)
            member += 1
            skipWhitespace()
            guard index < characters.count else { break }
            if characters[index] == "," {
                index += 1
                continue
            }
            if characters[index] == "]" {
                index += 1
                break
            }
            break
        }

        return JSONNode(id: path, key: key, kind: .array, raw: "[\(member)]", children: children)
    }

    mutating func takeString() -> String {
        let start = index
        guard index < characters.count, characters[index] == "\"" else {
            return ""
        }
        index += 1
        while index < characters.count {
            if characters[index] == "\\" {
                index += 2
                continue
            }
            if characters[index] == "\"" {
                index += 1
                break
            }
            index += 1
        }
        return String(characters[start..<min(index, characters.count)])
    }

    mutating func takeNumber() -> String {
        let start = index
        while index < characters.count {
            switch characters[index] {
            case "-", "+", "0"..."9", ".", "e", "E":
                index += 1
            default:
                return String(characters[start..<index])
            }
        }
        return String(characters[start..<index])
    }

    mutating func takeLiteral() -> String {
        let start = index
        while index < characters.count, characters[index].isLetter {
            index += 1
        }
        return String(characters[start..<index])
    }

    mutating func skipWhitespace() {
        while index < characters.count, characters[index].isWhitespace {
            index += 1
        }
    }
}

private enum ParseError: Error {
    case truncated
}
