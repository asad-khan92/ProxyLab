//  Indenting JSON without rewriting it.
//
//  Foundation can pretty-print JSON, but only by parsing it into dictionaries
//  first, and a dictionary has no order: the keys come back out in whatever
//  order the hash gave them. For a debugging proxy that is the wrong trade.
//  What a developer is looking at is the body the server actually sent, and
//  the order of its fields is part of that.
//
//  So this reformats the bytes rather than the values. Strings, numbers, and
//  literals are copied through exactly as they arrived — every digit, every
//  escape, every key in the order it was written — and the only things added
//  are the newlines and spaces between them. The walk is over UTF-8 bytes.
//  An array of characters would be many times the document, and a body this
//  size is already the largest the pane will draw.
//
//  It does not validate: `BodyInspection` has Foundation confirm the bytes
//  are JSON before this is asked to lay them out. The spans it returns point
//  into the formatted text, so a view can color that text without scanning
//  it again.

import Foundation

/// One colored range of formatted JSON.
///
/// `start` and `end` are UTF-8 offsets into the formatted text, not into the
/// captured bytes and not into the presented text the raw view shows.
struct JSONSpan: Equatable, Sendable {
    enum Role: String, Equatable, Sendable {
        case key
        case string
        case number
        case boolean
        case null
    }

    let start: Int
    let end: Int
    let role: Role
}

/// Formatted JSON and the token ranges inside it.
struct JSONLayout: Equatable, Sendable {
    let text: String
    let spans: [JSONSpan]
}

/// Lays JSON out over several lines, changing nothing else.
enum JSONPrettyPrinter {
    /// How far one level of nesting is indented.
    fileprivate static let indentWidth = 2

    /// `json` laid out for reading, or `nil` if it is not text.
    static func format(_ json: Data) -> String? {
        layout(json)?.text
    }

    /// `json` laid out, with a span for every key, string, number, and literal.
    static func layout(_ json: Data) -> JSONLayout? {
        guard String(data: json, encoding: .utf8) != nil else { return nil }

        var printer = Printer(bytes: [UInt8](json))
        return printer.run()
    }
}

private struct Printer {
    let bytes: [UInt8]
    var index = 0
    var output: [UInt8] = []
    var spans: [JSONSpan] = []
    var depth = 0

    mutating func run() -> JSONLayout {
        output.reserveCapacity(bytes.count + bytes.count / 4)

        while index < bytes.count {
            let byte = bytes[index]

            switch byte {
            case quote:
                writeString()
            case openBrace, openBracket:
                writeOpen(byte)
            case closeBrace, closeBracket:
                writeClose(byte)
            case comma:
                output.append(comma)
                newline()
                index += 1
            case colon:
                output.append(colon)
                output.append(space)
                index += 1
            case space, tab, lineFeed, carriageReturn:
                // Whitespace between values is the server's formatting, and
                // this is replacing it.
                index += 1
            default:
                writeAtom()
            }
        }

        return JSONLayout(
            text: String(decoding: output, as: UTF8.self),
            spans: spans
        )
    }

    mutating func writeString() {
        let start = index
        index += 1

        while index < bytes.count {
            if bytes[index] == backslash {
                index += 1
                if index < bytes.count { index += 1 }
                continue
            }
            if bytes[index] == quote {
                index += 1
                break
            }
            index += 1
        }

        let end = min(index, bytes.count)
        let role: JSONSpan.Role = nextIsColon() ? .key : .string
        emit(bytes[start..<end], role: role)
    }

    mutating func writeOpen(_ byte: UInt8) {
        output.append(byte)

        // An empty object or array stays on one line: `{}` read over
        // three lines is harder to read, not easier.
        if let next = nextSignificant(after: index), next.byte == closing(byte) {
            output.append(next.byte)
            index = next.index + 1
        } else {
            depth += 1
            newline()
            index += 1
        }
    }

    mutating func writeClose(_ byte: UInt8) {
        depth = max(0, depth - 1)
        newline()
        output.append(byte)
        index += 1
    }

    mutating func writeAtom() {
        let start = index
        let byte = bytes[index]

        if isNumberStart(byte) {
            index += 1
            while index < bytes.count, isNumberByte(bytes[index]) { index += 1 }
            emit(bytes[start..<index], role: .number)
            return
        }

        if isLetter(byte) {
            index += 1
            while index < bytes.count, isLetter(bytes[index]) { index += 1 }
            let word = String(decoding: bytes[start..<index], as: UTF8.self)
            let role: JSONSpan.Role? =
                switch word {
                case "true", "false": .boolean
                case "null": .null
                default: nil
                }
            emit(bytes[start..<index], role: role)
            return
        }

        output.append(byte)
        index += 1
    }

    mutating func emit(_ bytes: ArraySlice<UInt8>, role: JSONSpan.Role?) {
        let start = output.count
        output.append(contentsOf: bytes)
        if let role {
            spans.append(JSONSpan(start: start, end: output.count, role: role))
        }
    }

    mutating func newline() {
        output.append(lineFeed)
        output.append(contentsOf: repeatElement(space, count: depth * JSONPrettyPrinter.indentWidth))
    }

    func nextIsColon() -> Bool {
        var cursor = index
        while cursor < bytes.count {
            let byte = bytes[cursor]
            if byte == space || byte == tab || byte == lineFeed || byte == carriageReturn {
                cursor += 1
                continue
            }
            return byte == colon
        }
        return false
    }

    func nextSignificant(after start: Int) -> (index: Int, byte: UInt8)? {
        var cursor = start + 1
        while cursor < bytes.count {
            let byte = bytes[cursor]
            if byte == space || byte == tab || byte == lineFeed || byte == carriageReturn {
                cursor += 1
                continue
            }
            return (cursor, byte)
        }
        return nil
    }

    func closing(_ open: UInt8) -> UInt8 {
        open == openBrace ? closeBrace : closeBracket
    }
}

private let quote = UInt8(ascii: "\"")
private let backslash = UInt8(ascii: "\\")
private let openBrace = UInt8(ascii: "{")
private let closeBrace = UInt8(ascii: "}")
private let openBracket = UInt8(ascii: "[")
private let closeBracket = UInt8(ascii: "]")
private let comma = UInt8(ascii: ",")
private let colon = UInt8(ascii: ":")
private let space = UInt8(ascii: " ")
private let tab = UInt8(ascii: "\t")
private let lineFeed = UInt8(ascii: "\n")
private let carriageReturn = UInt8(ascii: "\r")

private func isNumberStart(_ byte: UInt8) -> Bool {
    byte == UInt8(ascii: "-") || (byte >= UInt8(ascii: "0") && byte <= UInt8(ascii: "9"))
}

private func isNumberByte(_ byte: UInt8) -> Bool {
    switch byte {
    case UInt8(ascii: "-"), UInt8(ascii: "+"), UInt8(ascii: "."), UInt8(ascii: "e"),
        UInt8(ascii: "E"), UInt8(ascii: "0")...UInt8(ascii: "9"):
        true
    default:
        false
    }
}

private func isLetter(_ byte: UInt8) -> Bool {
    (byte >= UInt8(ascii: "a") && byte <= UInt8(ascii: "z"))
        || (byte >= UInt8(ascii: "A") && byte <= UInt8(ascii: "Z"))
}
