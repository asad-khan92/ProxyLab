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
//  are the newlines and spaces between them.
//
//  It does not validate: `BodyInspection` has Foundation confirm the bytes
//  are JSON before this is asked to lay them out.

import Foundation

/// Lays JSON out over several lines, changing nothing else.
enum JSONPrettyPrinter {
    /// How far one level of nesting is indented.
    private static let indent = "  "

    /// `json` laid out for reading, or `nil` if it is not text.
    static func format(_ json: Data) -> String? {
        guard let source = String(data: json, encoding: .utf8) else { return nil }

        var output = ""
        output.reserveCapacity(source.count + source.count / 4)

        var depth = 0
        let characters = Array(source)
        var index = 0

        while index < characters.count {
            let character = characters[index]

            switch character {
            case "\"":
                let end = endOfString(in: characters, from: index)
                output.append(contentsOf: characters[index...end])
                index = end + 1

            case "{", "[":
                output.append(character)
                let next = nextSignificant(in: characters, after: index)

                // An empty object or array stays on one line: `{}` read over
                // three lines is harder to read, not easier.
                if let next, characters[next] == (character == "{" ? "}" : "]") {
                    output.append(characters[next])
                    index = next + 1
                } else {
                    depth += 1
                    appendNewline(to: &output, depth: depth)
                    index += 1
                }

            case "}", "]":
                depth = max(0, depth - 1)
                appendNewline(to: &output, depth: depth)
                output.append(character)
                index += 1

            case ",":
                output.append(character)
                appendNewline(to: &output, depth: depth)
                index += 1

            case ":":
                output.append(": ")
                index += 1

            case " ", "\t", "\n", "\r":
                // Whitespace between values is the server's formatting, and
                // this is replacing it.
                index += 1

            default:
                output.append(character)
                index += 1
            }
        }

        return output
    }

    /// The index of the quote that closes the string starting at `start`.
    private static func endOfString(in characters: [Character], from start: Int) -> Int {
        var index = start + 1

        while index < characters.count {
            if characters[index] == "\\" {
                index += 2
                continue
            }
            if characters[index] == "\"" {
                return index
            }

            index += 1
        }

        // Unterminated, which the validation before this ruled out. Copying
        // the rest is the reading that loses nothing.
        return characters.count - 1
    }

    /// The index of the next character that is not whitespace.
    private static func nextSignificant(in characters: [Character], after start: Int) -> Int? {
        var index = start + 1

        while index < characters.count {
            if !characters[index].isWhitespace { return index }
            index += 1
        }

        return nil
    }

    private static func appendNewline(to output: inout String, depth: Int) {
        output.append("\n")
        output.append(String(repeating: indent, count: depth))
    }
}
