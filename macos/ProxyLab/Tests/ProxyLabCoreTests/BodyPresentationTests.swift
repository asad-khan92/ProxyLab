//  The body viewer, as a decision rather than as a window.
//
//  Pretty is a layout of the presented JSON. Raw is that presented text, or
//  Base64 when the bytes are not text. The capture is neither of those, and
//  a content coding stays on it. The tree is the same JSON, and copying a
//  subtree is that node's own JSON rather than the whole formatted document.

import Foundation
import SwiftUI
import Testing

@testable import ProxyLabCore

@Suite("Body presentation")
struct BodyPresentationTests {
    private let person = """
        {"data":{"name":"Zunaira agha","user_code":"PK-KHI-AC-28-128238","profile_picture":null,"rank":null,"class_name":"AS","section_name":"A","meta":null,"flags":[true,false]}}
        """

    private func inspect(_ text: String, type: String? = nil, encoding: String? = nil) -> BodyInspection {
        BodyInspection.of(
            .complete(Data(text.utf8)),
            contentType: type.flatMap(ContentType.parse),
            contentEncoding: encoding
        )
    }

    private func textBody(_ inspection: BodyInspection) throws -> TextBody {
        guard case let .text(text) = inspection.content else {
            Issue.record("the body should have decoded as text")
            throw PresentationRead.notText
        }
        return text
    }

    private func slices(_ layout: JSONLayout, role: JSONSpan.Role) -> [String] {
        layout.spans.filter { $0.role == role }.map { slice(layout.text, start: $0.start, end: $0.end) }
    }

    private func slice(_ text: String, start: Int, end: Int) -> String {
        let utf8 = text.utf8
        let lower = utf8.index(utf8.startIndex, offsetBy: start)
        let upper = utf8.index(lower, offsetBy: end - start)
        return String(decoding: utf8[lower..<upper], as: UTF8.self)
    }

    // MARK: - Pretty

    @Test("pretty JSON is indented and keeps the sender's tokens")
    func prettyLayout() throws {
        let layout = try #require(
            JSONPrettyPrinter.layout(Data(#"{"a":1,"b":[2,3],"ok":true,"meta":null}"#.utf8))
        )

        #expect(
            layout.text == """
                {
                  "a": 1,
                  "b": [
                    2,
                    3
                  ],
                  "ok": true,
                  "meta": null
                }
                """
        )
        #expect(slices(layout, role: .key) == [#""a""#, #""b""#, #""ok""#, #""meta""#])
        #expect(slices(layout, role: .number) == ["1", "2", "3"])
        #expect(slices(layout, role: .boolean) == ["true"])
        #expect(slices(layout, role: .null) == ["null"])
    }

    @Test("a string, a nested object, and Unicode stay inside their spans")
    func prettyValues() throws {
        let source = #"{"name":"Ada 💡","note":"a \"quote\"","child":{"n":0.10}}"#
        let layout = try #require(JSONPrettyPrinter.layout(Data(source.utf8)))

        #expect(slices(layout, role: .string) == ["\"Ada 💡\"", "\"a \\\"quote\\\"\""])
        #expect(slices(layout, role: .key) == [#""name""#, #""note""#, #""child""#, #""n""#])
        #expect(slices(layout, role: .number) == ["0.10"])
        #expect(layout.text.contains("Ada 💡"))
        #expect(layout.text.contains(#"\"quote\""#))
        #expect(!layout.text.contains("\u{0000}"))
    }

    @Test("pretty lines carry a key span after the indent, not before it")
    func prettyLines() throws {
        let layout = try #require(JSONPrettyPrinter.layout(Data(#"{"a":1}"#.utf8)))
        let lines = BodyLines.split(layout.text, spans: layout.spans)

        #expect(lines.map(\.text) == ["{", "  \"a\": 1", "}"])
        let key = try #require(lines[1].spans.first { $0.role == .key })
        #expect(slice(lines[1].text, start: key.start, end: key.end) == #""a""#)
        #expect(key.start == 2)
    }

    // MARK: - Tree

    @Test("a nested object and an array keep order, indexes, and subtree JSON")
    func tree() throws {
        let inspection = inspect(person, type: "application/json")
        let text = try textBody(inspection)
        let root = try #require(text.tree)
        let data = try #require(root.children.first { $0.key == "data" })
        let flags = try #require(data.children.first { $0.key == "flags" })

        #expect(data.children.map(\.key) == [
            "name", "user_code", "profile_picture", "rank", "class_name", "section_name", "meta",
            "flags",
        ])
        #expect(data.children.first { $0.key == "name" }?.copyableValue == "Zunaira agha")
        #expect(data.children.first { $0.key == "profile_picture" }?.raw == "null")
        #expect(data.children.first { $0.key == "rank" }?.kind == .literal)
        #expect(flags.kind == .array)
        #expect(flags.children.map(\.key) == [nil, nil])
        #expect(flags.children.map(\.id) == ["$.data.flags[0]", "$.data.flags[1]"])
        #expect(flags.children.map(\.raw) == ["true", "false"])
        #expect(flags.serialized == "[true, false]")
        #expect(data.serialized.contains(#""name": "Zunaira agha""#))
        #expect(data.serialized.contains(#""profile_picture": null"#))

        let formatted = try #require(text.formatted)
        #expect(BodyCopy.formattedJSON(inspection) == formatted)
        #expect(formatted.contains("\n"))
        #expect(formatted.contains("Zunaira agha"))
        #expect(data.serialized != formatted)
        #expect(JSONTreeIDs.containers(in: root).contains("$"))
        #expect(JSONTreeIDs.containers(in: root).contains("$.data"))
        #expect(JSONTreeIDs.containers(in: root).contains("$.data.flags"))
        #expect(!JSONTreeIDs.containers(in: root).contains("$.data.name"))
    }

    // MARK: - Raw, modes, and the three byte layers

    @Test("JSON opens in Pretty, and Raw is the presented text")
    func jsonModes() throws {
        let inspection = inspect(person, type: "application/json")

        let formatted = try textBody(inspection).formatted

        #expect(BodyPresentation.modes(for: inspection) == [.pretty, .tree, .raw])
        #expect(BodyPresentation.defaultMode(for: inspection) == .pretty)
        #expect(BodyCopy.presentedText(inspection, mode: .raw) == person)
        #expect(BodyCopy.presentedText(inspection, mode: .pretty) == formatted)
        #expect(BodyCopy.presentedText(inspection, mode: .tree) == formatted)
        #expect(inspection.capture.data == Data(person.utf8))
        #expect(inspection.presented == inspection.capture.data)
    }

    @Test("gzip JSON is pretty-printed from the presented bytes, and the capture stays gzip")
    func gzipJSONUsesPresentedBytes() throws {
        let gzip = hex(
            "1f8b08000000000002ffab56cacf56b22a292a4dd551ca53b232ac050003080c4111000000"
        )
        let inspection = BodyInspection.of(
            .complete(gzip),
            contentType: ContentType.parse("application/json"),
            contentEncoding: "gzip"
        )
        let text = try textBody(inspection)

        #expect(text.text == #"{"ok":true,"n":1}"#)
        #expect(text.formatted?.contains("\"ok\": true") == true)
        #expect(text.highlights.contains { $0.role == .key })
        #expect(BodyPresentation.defaultMode(for: inspection) == .pretty)
        #expect(BodyCopy.presentedText(inspection, mode: .raw) == #"{"ok":true,"n":1}"#)
        #expect(inspection.presented == Data(#"{"ok":true,"n":1}"#.utf8))
        #expect(inspection.capture.data == gzip)
        #expect(inspection.presented != inspection.capture.data)
    }

    @Test("HTML and XML stay the text they are, with no pretty mode")
    func markupStaysRaw() {
        let html = inspect("<html><body>Hi</body></html>", type: "text/html")
        let xml = inspect("<user><name>Ada</name></user>", type: "application/xml")

        #expect(BodyPresentation.modes(for: html).isEmpty)
        #expect(BodyPresentation.modes(for: xml).isEmpty)
        #expect(BodyPresentation.defaultMode(for: html) == .raw)
        #expect(BodyCopy.presentedText(html, mode: .raw) == "<html><body>Hi</body></html>")
        #expect(BodyCopy.formattedJSON(html) == nil)
        #expect(BodyCopy.formattedJSON(xml) == nil)
    }

    @Test("a binary body stays binary and copies as Base64")
    func binaryStaysBinary() {
        let bytes = Data([0xFF, 0xFE, 0x00, 0x01])
        let inspection = BodyInspection.of(
            .complete(bytes),
            contentType: ContentType.parse("application/octet-stream")
        )

        #expect(inspection.content == .binary)
        #expect(String(data: bytes, encoding: .utf8) == nil)
        #expect(BodyPresentation.modes(for: inspection).isEmpty)
        #expect(BodyCopy.formattedJSON(inspection) == nil)
        #expect(BodyCopy.presentedText(inspection, mode: .raw) == bytes.base64EncodedString())
        #expect(inspection.presented == bytes)
    }

    @Test("an image offers Preview and Raw, and Raw is Base64 of the presented bytes")
    func imageModes() {
        let png = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00])
        let inspection = BodyInspection.of(.complete(png))

        #expect(BodyPresentation.modes(for: inspection) == [.preview, .raw])
        #expect(BodyPresentation.defaultMode(for: inspection) == .preview)
        #expect(BodyCopy.presentedText(inspection, mode: .raw) == png.base64EncodedString())
        #expect(BodyCopy.formattedJSON(inspection) == nil)
    }

    @Test("a truncated JSON body is not laid out, and stays marked truncated")
    func truncatedJSON() throws {
        let capture = BodyCapture(
            data: Data(#"{"a":1,"#.utf8),
            size: 4_096,
            truncated: true
        )
        let inspection = BodyInspection.of(
            capture,
            contentType: ContentType.parse("application/json")
        )
        let text = try textBody(inspection)

        #expect(text.formatted == nil)
        #expect(text.tree == nil)
        #expect(text.highlights.isEmpty)
        #expect(text.shortened == false)
        #expect(BodyPresentation.modes(for: inspection).isEmpty)
        #expect(BodyCopy.formattedJSON(inspection) == nil)
        #expect(BodyCopy.presentedText(inspection, mode: .raw) == #"{"a":1,"#)
        #expect(inspection.capture.truncated)
        #expect(inspection.presented == capture.data)
    }

    @Test("a body shortened for the window is not given a pretty or tree mode")
    func shortenedJSONHasNoLayout() throws {
        let filler = String(repeating: "y", count: BodyInspection.displayedCharacterLimit)
        let inspection = inspect("{\"a\":\"\(filler)\"}", type: "application/json")
        let text = try textBody(inspection)

        #expect(text.shortened)
        #expect(text.formatted == nil)
        #expect(text.tree == nil)
        #expect(BodyPresentation.modes(for: inspection).isEmpty)
        #expect(BodyCopy.presentedText(inspection, mode: .raw).count > text.text.count)
    }

    // MARK: - Facts and find

    @Test("the fact row keeps only what is known")
    func facts() {
        let inspection = inspect(#"{"a":1}"#, type: "application/json; charset=utf-8")
        let known = BodyFacts.make(inspection: inspection, contentEncoding: "gzip", status: "200 OK")

        #expect(known.items.map(\.text) == [
            "application/json", "gzip", TrafficDisplay.size(inspection.capture.size), "200 OK",
        ])
        #expect(known.items.first { $0.id == "type" }?.help == "application/json; charset=utf-8")

        let bare = BodyFacts.make(
            inspection: inspect("hello"),
            contentEncoding: "  ",
            status: nil
        )
        #expect(bare.items.map(\.id) == ["size"])

        let truncated = BodyFacts.make(
            inspection: BodyInspection.of(
                BodyCapture(data: Data("ab".utf8), size: 80, truncated: true),
                contentType: ContentType.parse("text/plain")
            ),
            contentEncoding: nil,
            status: nil
        )
        #expect(truncated.items.map(\.id).contains("truncated"))
    }

    @Test("find is a literal substring, and offsets survive a character wider than a byte")
    func find() {
        let text = "💡 name Name"
        let hits = BodySearch.search("name", in: text)

        #expect(hits.hits.count == 2)
        #expect(hits.capped == false)
        #expect(slice(text, start: hits.hits[0].utf8Start, end: hits.hits[0].utf8End) == "name")
        #expect(slice(text, start: hits.hits[1].utf8Start, end: hits.hits[1].utf8End) == "Name")
        #expect(BodySearch.search("", in: text).hits.isEmpty)
        #expect(BodySearch.search("missing", in: text).hits.isEmpty)
    }

    @Test("a JSON body draws")
    @MainActor
    func draws() throws {
        let inspection = inspect(person, type: "application/json")
        let view = BodyView(
            inspection: inspection,
            contentEncoding: "gzip",
            status: "200 OK",
            save: {},
            copy: { _, _ in }
        )
        let renderer = ImageRenderer(content: view.frame(width: 520))
        renderer.scale = 1

        #expect((renderer.nsImage?.size.height ?? 0) > 0)
    }

    private func hex(_ text: String) -> Data {
        var data = Data()
        var index = text.startIndex
        while index < text.endIndex {
            let next = text.index(index, offsetBy: 2)
            data.append(UInt8(text[index..<next], radix: 16)!)
            index = next
        }
        return data
    }
}

private enum PresentationRead: Error {
    case notText
}
