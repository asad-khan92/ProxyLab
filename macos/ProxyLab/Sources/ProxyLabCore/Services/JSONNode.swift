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
        // Reject a body that is not UTF-8, then walk the bytes. An array of
        // characters would be many times the document.
        guard String(data: data, encoding: .utf8) != nil else { return nil }

        var parser = Parser(bytes: [UInt8](data))
        guard let node = try? parser.parseValue(path: "$", key: nil) else { return nil }
        parser.skipWhitespace()
        guard parser.index >= parser.bytes.count else { return nil }

        return node
    }
}

private struct Parser {
    let bytes: [UInt8]
    var index = 0

    mutating func parseValue(path: String, key: String?) throws -> JSONNode {
        skipWhitespace()
        guard let byte = peek() else { throw ParseError.truncated }

        switch byte {
        case UInt8(ascii: "{"):
            return try parseObject(path: path, key: key)
        case UInt8(ascii: "["):
            return try parseArray(path: path, key: key)
        case UInt8(ascii: "\""):
            let raw = takeString()
            return JSONNode(id: path, key: key, kind: .string, raw: raw, children: [])
        case UInt8(ascii: "-"), UInt8(ascii: "0")...UInt8(ascii: "9"):
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

        if peek() == UInt8(ascii: "}") {
            index += 1
            return JSONNode(id: path, key: key, kind: .object, raw: "{}", children: [])
        }

        while index < bytes.count {
            skipWhitespace()
            let name = takeString()
            skipWhitespace()
            if peek() == UInt8(ascii: ":") { index += 1 }
            let decodedKey = String(name.dropFirst().dropLast())
            let child = try parseValue(path: "\(path).\(decodedKey)", key: decodedKey)
            children.append(child)
            member += 1
            skipWhitespace()
            guard let byte = peek() else { break }
            if byte == UInt8(ascii: ",") {
                index += 1
                continue
            }
            if byte == UInt8(ascii: "}") {
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

        if peek() == UInt8(ascii: "]") {
            index += 1
            return JSONNode(id: path, key: key, kind: .array, raw: "[]", children: [])
        }

        var member = 0
        while index < bytes.count {
            let child = try parseValue(path: "\(path)[\(member)]", key: nil)
            children.append(child)
            member += 1
            skipWhitespace()
            guard let byte = peek() else { break }
            if byte == UInt8(ascii: ",") {
                index += 1
                continue
            }
            if byte == UInt8(ascii: "]") {
                index += 1
                break
            }
            break
        }

        return JSONNode(id: path, key: key, kind: .array, raw: "[\(member)]", children: children)
    }

    mutating func takeString() -> String {
        let start = index
        guard peek() == UInt8(ascii: "\"") else { return "" }
        index += 1
        while index < bytes.count {
            if bytes[index] == UInt8(ascii: "\\") {
                index += 2
                continue
            }
            if bytes[index] == UInt8(ascii: "\"") {
                index += 1
                break
            }
            index += 1
        }
        return text(from: start, to: min(index, bytes.count))
    }

    mutating func takeNumber() -> String {
        let start = index
        while index < bytes.count {
            switch bytes[index] {
            case UInt8(ascii: "-"), UInt8(ascii: "+"), UInt8(ascii: "0")...UInt8(ascii: "9"),
                UInt8(ascii: "."), UInt8(ascii: "e"), UInt8(ascii: "E"):
                index += 1
            default:
                return text(from: start, to: index)
            }
        }
        return text(from: start, to: index)
    }

    mutating func takeLiteral() -> String {
        let start = index
        while index < bytes.count, isLetter(bytes[index]) {
            index += 1
        }
        return text(from: start, to: index)
    }

    mutating func skipWhitespace() {
        while let byte = peek(), isJSONWhitespace(byte) {
            index += 1
        }
    }

    func peek() -> UInt8? {
        index < bytes.count ? bytes[index] : nil
    }

    func text(from start: Int, to end: Int) -> String {
        String(decoding: bytes[start..<end], as: UTF8.self)
    }
}

private func isJSONWhitespace(_ byte: UInt8) -> Bool {
    byte == 0x20 || byte == 0x09 || byte == 0x0A || byte == 0x0D
}

private func isLetter(_ byte: UInt8) -> Bool {
    (byte >= UInt8(ascii: "a") && byte <= UInt8(ascii: "z"))
        || (byte >= UInt8(ascii: "A") && byte <= UInt8(ascii: "Z"))
}

private enum ParseError: Error {
    case truncated
}
