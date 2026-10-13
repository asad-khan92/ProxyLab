//  How a body reading is shown, without reading it again.
//
//  Three strings are easy to mix up, and the viewer must not:
//
//      captured bytes     what the proxy stored. A content coding is still
//                         on them. Save writes these, and nothing here
//                         changes them.
//      presented bytes    the capture after a declared Content-Encoding was
//                         removed. Raw shows this: as text when it decoded,
//                         as Base64 when it did not.
//      formatted JSON     a layout of the presented JSON, for the pretty
//                         view and for Copy JSON. It is not what was captured.
//
//  The decision of which modes exist is made from the reading `BodyInspection`
//  already produced. A view asks; it does not parse.

import Foundation

/// One way of looking at a body.
enum BodyViewMode: String, Hashable, Sendable {
    case pretty
    case tree
    case raw
    case preview

    var title: String {
        switch self {
        case .pretty: "Pretty"
        case .tree: "Tree"
        case .raw: "Raw"
        case .preview: "Preview"
        }
    }
}

/// Which views a reading can honestly offer.
enum BodyPresentation {
    /// The modes a picker should list, in display order.
    ///
    /// Empty when there is only one honest way to show the body, so the
    /// picker is not a switch with one side.
    static func modes(for inspection: BodyInspection) -> [BodyViewMode] {
        switch inspection.content {
        case let .text(text) where text.formatted != nil && text.tree != nil:
            [.pretty, .tree, .raw]
        case let .text(text) where text.formatted != nil:
            [.pretty, .raw]
        case .image:
            [.preview, .raw]
        case .text, .empty, .binary, .undecodable:
            []
        }
    }

    /// Pretty for JSON, the preview for an image, and the presented text
    /// otherwise. There is no stored preference to consult.
    static func defaultMode(for inspection: BodyInspection) -> BodyViewMode {
        modes(for: inspection).first ?? .raw
    }
}

/// What the body actions put on the clipboard.
enum BodyCopy {
    /// The text of the view `mode` is showing.
    ///
    /// Pretty and Tree copy the formatted JSON, because that is the document
    /// on screen. Raw and Preview copy the presented body: the decoded text,
    /// or Base64 when the presented bytes are not text. Neither is the
    /// capture. A shortened window still copies the whole presented text;
    /// the window is the only thing that was shortened.
    static func presentedText(_ inspection: BodyInspection, mode: BodyViewMode) -> String {
        switch mode {
        case .pretty, .tree:
            if let formatted = formattedJSON(inspection) { return formatted }
            return ExchangeText.body(inspection)
        case .raw, .preview:
            return ExchangeText.body(inspection)
        }
    }

    /// The complete formatted JSON, when this reading has one.
    static func formattedJSON(_ inspection: BodyInspection) -> String? {
        guard case let .text(text) = inspection.content else { return nil }

        return text.formatted
    }
}

/// The compact facts drawn above a body. Absent facts are absent, not filled in.
struct BodyFacts: Equatable, Sendable {
    struct Item: Equatable, Sendable, Identifiable {
        let id: String
        let text: String
        let help: String
    }

    var items: [Item]

    static func make(
        inspection: BodyInspection,
        contentEncoding: String?,
        status: String?
    ) -> BodyFacts {
        var items: [Item] = []

        if let type = inspection.contentType {
            items.append(
                Item(
                    id: "type",
                    text: type.essence,
                    help: type.raw == type.essence ? "Content-Type" : type.raw
                )
            )
        }

        switch inspection.content {
        case let .text(text) where text.encoding != .utf8:
            items.append(
                Item(id: "encoding", text: text.encodingName, help: "Character set it decoded as")
            )
        case let .image(image) where inspection.contentType == nil:
            items.append(Item(id: "format", text: image.format, help: "Image format"))
        default:
            break
        }

        if let coding = contentEncoding?.trimmingCharacters(in: .whitespacesAndNewlines),
            !coding.isEmpty
        {
            items.append(Item(id: "coding", text: coding, help: "Content-Encoding"))
        }

        if !inspection.isEmpty {
            items.append(
                Item(
                    id: "size",
                    text: TrafficDisplay.size(inspection.capture.size),
                    help: "Captured body size"
                )
            )
        }

        if inspection.capture.truncated {
            items.append(
                Item(
                    id: "truncated",
                    text: "truncated",
                    help: "The capture kept less than the full body"
                )
            )
        }

        if let status, !status.isEmpty {
            items.append(Item(id: "status", text: status, help: "Response status"))
        }

        return BodyFacts(items: items)
    }
}

/// One literal match in a body, as UTF-8 offsets into the text that was searched.
struct BodyMatch: Equatable, Sendable {
    let utf8Start: Int
    let utf8End: Int
}

/// A literal find over the text a mode is showing. Not a query language.
enum BodySearch {
    struct Result: Equatable, Sendable {
        var hits: [BodyMatch]
        var capped: Bool
    }

    /// Case-insensitive substring matches, in order, capped so a query of one
    /// common character cannot build a list as long as the body.
    static func search(_ query: String, in text: String, limit: Int = 400) -> Result {
        guard !query.isEmpty else { return Result(hits: [], capped: false) }

        var hits: [BodyMatch] = []
        var rest = text.startIndex..<text.endIndex

        while hits.count < limit {
            guard let found = text.range(of: query, options: [.caseInsensitive], range: rest) else {
                break
            }

            hits.append(
                BodyMatch(
                    utf8Start: utf8Offset(of: found.lowerBound, in: text),
                    utf8End: utf8Offset(of: found.upperBound, in: text)
                )
            )
            guard found.upperBound < text.endIndex else { break }
            rest = found.upperBound..<text.endIndex
        }

        let capped =
            hits.count == limit && text.range(of: query, options: [.caseInsensitive], range: rest) != nil

        return Result(hits: hits, capped: capped)
    }

    private static func utf8Offset(of index: String.Index, in text: String) -> Int {
        guard let utf8Index = index.samePosition(in: text.utf8) else { return 0 }

        return text.utf8.distance(from: text.utf8.startIndex, to: utf8Index)
    }
}

/// One logical line of a body, with token spans rewritten to that line.
struct BodyLine: Equatable, Identifiable, Sendable {
    let number: Int
    /// UTF-8 offset of this line in the whole text.
    let utf8Start: Int
    let text: String
    /// Spans whose offsets are local to `text`.
    let spans: [JSONSpan]

    var id: Int { number }
}

/// Splits a body into lines without building an array of characters.
enum BodyLines {
    static func split(_ text: String, spans: [JSONSpan] = []) -> [BodyLine] {
        var lines: [BodyLine] = []
        let utf8 = text.utf8
        var lineStart = utf8.startIndex
        var lineOffset = 0
        var number = 1
        var cursor = utf8.startIndex

        while cursor < utf8.endIndex {
            if utf8[cursor] == 0x0A {
                let byteCount = utf8.distance(from: lineStart, to: cursor)
                lines.append(
                    line(
                        number: number,
                        utf8Start: lineOffset,
                        bytes: utf8[lineStart..<cursor],
                        byteCount: byteCount,
                        spans: spans
                    )
                )
                lineOffset += byteCount + 1
                number += 1
                cursor = utf8.index(after: cursor)
                lineStart = cursor
            } else {
                cursor = utf8.index(after: cursor)
            }
        }

        if lineStart < utf8.endIndex || lines.isEmpty {
            let byteCount = utf8.distance(from: lineStart, to: utf8.endIndex)
            lines.append(
                line(
                    number: number,
                    utf8Start: lineOffset,
                    bytes: utf8[lineStart..<utf8.endIndex],
                    byteCount: byteCount,
                    spans: spans
                )
            )
        }

        return lines
    }

    private static func line(
        number: Int,
        utf8Start: Int,
        bytes: String.UTF8View.SubSequence,
        byteCount: Int,
        spans: [JSONSpan]
    ) -> BodyLine {
        let lineEnd = utf8Start + byteCount
        let local = spans.compactMap { span -> JSONSpan? in
            guard span.end > utf8Start, span.start < lineEnd else { return nil }

            let start = max(span.start, utf8Start) - utf8Start
            let end = min(span.end, lineEnd) - utf8Start
            guard start < end else { return nil }

            return JSONSpan(start: start, end: end, role: span.role)
        }

        return BodyLine(
            number: number,
            utf8Start: utf8Start,
            text: String(decoding: bytes, as: UTF8.self),
            spans: local
        )
    }
}

/// The ids of nodes that can be expanded. Leaves are not included, so a
/// collapsed row never has to ask its children to exist as views.
enum JSONTreeIDs {
    static func containers(in node: JSONNode) -> Set<String> {
        var ids: Set<String> = []
        walk(node, into: &ids)
        return ids
    }

    private static func walk(_ node: JSONNode, into ids: inout Set<String>) {
        guard !node.children.isEmpty else { return }

        ids.insert(node.id)
        for child in node.children {
            walk(child, into: &ids)
        }
    }
}
