//  Showing a body without changing it.
//
//  The rule these are protecting: what the interface shows is what the core
//  captured. JSON is laid out, never re-serialized; a body that is not text is
//  said to be binary rather than decoded lossily; a body the sender called text
//  and that will not decode says so rather than becoming mojibake; a truncated
//  capture is not quietly presented as a whole one.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Body inspection")
struct BodyInspectionTests {
    private func capture(_ text: String) -> BodyCapture {
        BodyCapture.complete(Data(text.utf8))
    }

    private func inspect(_ text: String, type: String? = nil) -> BodyInspection {
        BodyInspection.of(capture(text), contentType: type.flatMap(ContentType.parse))
    }

    /// The text of a body that decoded, or a recorded failure.
    private func textBody(
        _ inspection: BodyInspection,
        _ comment: Comment = "the body should have decoded as text",
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> TextBody? {
        guard case let .text(text) = inspection.content else {
            Issue.record(comment, sourceLocation: sourceLocation)
            return nil
        }

        return text
    }

    // MARK: - Nothing, and text

    @Test("a body that was never there is empty")
    func empty() {
        #expect(BodyInspection.of(.empty).content == .empty)
        #expect(BodyInspection.of(.empty).isEmpty)
    }

    @Test("text is shown as it arrived, whitespace included")
    func text() {
        let source = "line one\n  line two\t\n"

        #expect(textBody(inspect(source))?.text == source)
        #expect(textBody(inspect(source))?.language == .plain)
        #expect(textBody(inspect(source))?.formatted == nil)
    }

    @Test("a body with no declared type keeps that absence")
    func noContentType() {
        #expect(inspect("hello").contentType == nil)
    }

    // MARK: - JSON

    @Test("JSON is laid out for reading")
    func json() throws {
        let text = try #require(textBody(inspect(#"{"a":1,"b":[2,3]}"#)))
        let formatted = try #require(text.formatted)

        #expect(text.language == .json)
        #expect(formatted.contains("\n"))
        #expect(formatted.contains("\"a\": 1"))

        let tree = try #require(text.tree)
        #expect(tree.kind == .object)
        #expect(tree.children.map(\.key) == ["a", "b"])
        #expect(tree.children[0].copyableValue == "1")
        #expect(tree.serialized.contains("\"a\":"))
    }

    @Test("a JSON body keeps the order its fields were sent in")
    func jsonOrder() throws {
        // The order is part of what is being debugged, and a parsed
        // dictionary has lost it.
        let source = #"{"zebra":1,"apple":2,"mango":3}"#
        let formatted = try #require(textBody(inspect(source))?.formatted)

        // The positions of the three keys, in the order they appear in the
        // output, which must be the order they were sent in.
        let positions = ["zebra", "apple", "mango"].compactMap {
            formatted.range(of: $0)?.lowerBound
        }

        #expect(positions.count == 3)
        #expect(positions == positions.sorted())
    }

    @Test("a JSON body keeps its values exactly")
    func jsonValues() throws {
        // A number re-serialized through a parser can come back as `1.0`, or
        // lose digits it had. The bytes are copied instead.
        let source = #"{"big":10000000000000000001,"exact":0.10,"escaped":"a\"b","slash":"a/b"}"#
        let formatted = try #require(textBody(inspect(source))?.formatted)

        #expect(formatted.contains("10000000000000000001"))
        #expect(formatted.contains("0.10"))
        #expect(formatted.contains(#""a\"b""#))
        #expect(formatted.contains("a/b"))
    }

    @Test("the raw body is still the bytes that arrived")
    func rawIsUnchanged() throws {
        let source = #"{"a":1}"#
        let inspection = inspect(source)

        #expect(textBody(inspection)?.text == source)
        #expect(inspection.capture.data == Data(source.utf8))
    }

    @Test("an empty object or array stays on one line")
    func emptyContainers() throws {
        let formatted = try #require(textBody(inspect(#"{"a":{},"b":[]}"#))?.formatted)

        #expect(formatted.contains("\"a\": {}"))
        #expect(formatted.contains("\"b\": []"))
    }

    @Test("a brace inside a string is not a nesting level")
    func bracesInStrings() throws {
        let formatted = try #require(textBody(inspect(#"{"a":"{not a brace}"}"#))?.formatted)

        #expect(formatted.contains(#""{not a brace}""#))
    }

    @Test("nested objects and arrays are laid out to their depth")
    func nested() throws {
        let source = #"{"a":{"b":{"c":[1,[2,{"d":3}]]}}}"#
        let formatted = try #require(textBody(inspect(source))?.formatted)

        #expect(formatted.contains("\"d\": 3"))
        // The deepest key is indented further than the shallowest one.
        let outer = try #require(formatted.range(of: "  \"a\""))
        let inner = try #require(formatted.range(of: "\"d\""))
        let outerIndent = formatted.distance(
            from: formatted.lineStart(of: outer.lowerBound),
            to: outer.lowerBound
        )
        let innerIndent = formatted.distance(
            from: formatted.lineStart(of: inner.lowerBound),
            to: inner.lowerBound
        )
        #expect(innerIndent > outerIndent)
    }

    @Test("a top-level array is laid out like an object")
    func topLevelArray() throws {
        let formatted = try #require(textBody(inspect(#"[1,2,{"a":3}]"#))?.formatted)

        #expect(formatted.contains("\n"))
        #expect(formatted.contains("\"a\": 3"))
    }

    @Test("Unicode in a JSON body survives being laid out")
    func jsonUnicode() throws {
        let source = #"{"name":"Ada 💡","greek":"λόγος","escaped":"é"}"#
        let formatted = try #require(textBody(inspect(source))?.formatted)

        #expect(formatted.contains("Ada 💡"))
        #expect(formatted.contains("λόγος"))
        // An escape the sender wrote is the sender's; it is not unescaped.
        #expect(formatted.contains(#"é"#))
    }

    @Test("a bare JSON value is text, not a layout problem")
    func jsonFragment() {
        // Valid JSON, and nothing is gained by spreading it over lines.
        #expect(textBody(inspect("42"))?.formatted == nil)
        #expect(textBody(inspect("42"))?.text == "42")
        #expect(textBody(inspect("\"hello\""))?.text == "\"hello\"")
    }

    @Test("text that is nearly JSON is text")
    func almostJSON() throws {
        let source = #"{"a":1"#
        let text = try #require(textBody(inspect(source)))

        #expect(text.text == source)
        #expect(text.formatted == nil)
        #expect(text.language == .plain)
    }

    @Test("a body the sender called JSON and that is not falls back to its text")
    func invalidJSONDeclared() throws {
        let source = "<html>not json at all</html>"
        let text = try #require(textBody(inspect(source, type: "application/json")))

        // Still called JSON, because that is what the message said it was —
        // which is the bug — and shown as the text it actually is.
        #expect(text.language == .json)
        #expect(text.formatted == nil)
        #expect(text.text == source)
    }

    @Test("JSON arriving as text/plain is still laid out")
    func jsonAsPlainText() throws {
        let text = try #require(textBody(inspect(#"{"a":1}"#, type: "text/plain")))

        #expect(text.language == .json)
        #expect(text.formatted?.contains("\"a\": 1") == true)
    }

    @Test("a registered type that ends in +json is JSON")
    func suffixedJSON() throws {
        let text = try #require(
            textBody(inspect(#"{"title":"nope"}"#, type: "application/problem+json"))
        )

        #expect(text.language == .json)
        #expect(text.formatted != nil)
    }

    // MARK: - The other text languages

    @Test("XML is named as XML and shown as it was written")
    func xml() throws {
        let source = "<?xml version=\"1.0\"?>\n<user><name>Ada</name></user>"
        let text = try #require(textBody(inspect(source, type: "application/xml")))

        #expect(text.language == .xml)
        // Not re-indented: an XML document's whitespace can be its content.
        #expect(text.formatted == nil)
        #expect(text.text == source)
    }

    @Test("text/xml is XML too")
    func textXML() {
        #expect(textBody(inspect("<a/>", type: "text/xml"))?.language == .xml)
        #expect(textBody(inspect("<a/>", type: "application/atom+xml"))?.language == .xml)
    }

    @Test("HTML is named as HTML")
    func html() throws {
        let source = "<!doctype html><html><body>hello</body></html>"
        let text = try #require(textBody(inspect(source, type: "text/html; charset=utf-8")))

        #expect(text.language == .html)
        #expect(text.text == source)
    }

    // MARK: - Encodings

    @Test("the character set the sender declared is what decodes the body")
    func declaredCharset() throws {
        // `0xA9` is `©` in Windows-1252 and is not valid UTF-8 on its own.
        let bytes = BodyCapture.complete(Data([0x43, 0x6F, 0x70, 0x79, 0xA9]))
        let inspection = BodyInspection.of(
            bytes,
            contentType: ContentType.parse("text/plain; charset=windows-1252")
        )
        let text = try #require(textBody(inspection))

        #expect(text.text == "Copy©")
        #expect(text.encoding == .windowsCP1252)
    }

    @Test("a body with no declared character set is read as UTF-8")
    func defaultsToUTF8() {
        #expect(textBody(inspect("é far away"))?.encoding == .utf8)
    }

    @Test("a body the sender called text and that no encoding reads says so")
    func undecodable() {
        let bytes = BodyCapture.complete(Data([0xFF, 0xFE, 0x00, 0x01]))
        let inspection = BodyInspection.of(
            bytes,
            contentType: ContentType.parse("text/plain")
        )

        #expect(inspection.content == .undecodable)
    }

    @Test("bytes that are not text are said to be binary, never decoded")
    func binary() {
        let capture = BodyCapture(data: Data([0xFF, 0xFE, 0x00, 0x01]), size: 4, truncated: false)

        #expect(BodyInspection.of(capture).content == .binary)
        #expect(BodyInspection.of(capture).capture.capturedByteCount == 4)
    }

    @Test("an octet-stream that is really text is shown as the text it is")
    func octetStreamText() {
        let inspection = inspect("plain words", type: "application/octet-stream")

        #expect(textBody(inspection)?.text == "plain words")
    }

    // MARK: - Truncation

    @Test("a truncated capture is shown as whatever it is, and still marked truncated")
    func truncated() throws {
        let capture = BodyCapture(
            data: Data(#"{"a":1,"#.utf8),
            size: 4_096,
            truncated: true
        )
        let inspection = BodyInspection.of(capture)

        // Cut mid-object, so it is not JSON any more — and it is not repaired
        // into something the server never sent.
        #expect(textBody(inspection)?.text == #"{"a":1,"#)
        #expect(textBody(inspection)?.formatted == nil)
        #expect(inspection.capture.truncated)
        #expect(inspection.capture.size == 4_096)
        #expect(inspection.capture.capturedByteCount == 7)
    }

    @Test("a truncated capture cut through a character is still the text it is")
    func truncatedMidCharacter() throws {
        // "naïve" with the last byte of the `ï` missing, which is what a
        // capture limit landing mid-character produces.
        var bytes = Data("naïve".utf8)
        bytes = bytes.dropLast(bytes.count - 3)
        let capture = BodyCapture(data: bytes, size: 6_000, truncated: true)
        let text = try #require(textBody(BodyInspection.of(capture)))

        #expect(text.text == "na")
    }

    @Test("bytes that are not text are not rescued by the truncation allowance")
    func truncatedBinaryStaysBinary() {
        let capture = BodyCapture(
            data: Data([0xFF, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA]),
            size: 9_000,
            truncated: true
        )

        #expect(BodyInspection.of(capture).content == .binary)
    }

    // MARK: - Large bodies

    @Test("a body too large to draw is shortened for the window and says so")
    func large() throws {
        let source = String(repeating: "x", count: BodyInspection.displayedCharacterLimit + 5_000)
        let inspection = inspect(source)
        let text = try #require(textBody(inspection))

        #expect(text.shortened)
        #expect(text.text.count == BodyInspection.displayedCharacterLimit)
        // The whole of it is still there to copy and to save.
        #expect(inspection.capture.capturedByteCount == source.utf8.count)
    }

    @Test("a large JSON body is not laid out from half of itself")
    func largeJSONIsNotFormatted() throws {
        let filler = String(repeating: "y", count: BodyInspection.displayedCharacterLimit)
        let inspection = inspect("{\"a\":\"\(filler)\"}")
        let text = try #require(textBody(inspection))

        #expect(text.shortened)
        #expect(text.formatted == nil)
    }

    // MARK: - Images

    @Test("an image is recognised by its own first bytes")
    func images() {
        let png = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
            + Data(repeating: 0, count: 8)
        let jpeg = Data([0xFF, 0xD8, 0xFF, 0xE0]) + Data(repeating: 0, count: 8)
        let gif = Data("GIF89a".utf8) + Data(repeating: 0, count: 8)
        let webp = Data("RIFF".utf8) + Data([0, 0, 0, 0]) + Data("WEBPVP8 ".utf8)

        #expect(BodyInspection.of(.complete(png)).content == .image(ImageBody(format: "PNG")))
        #expect(BodyInspection.of(.complete(jpeg)).content == .image(ImageBody(format: "JPEG")))
        #expect(BodyInspection.of(.complete(gif)).content == .image(ImageBody(format: "GIF")))
        #expect(BodyInspection.of(.complete(webp)).content == .image(ImageBody(format: "WebP")))
    }

    @Test("a message that says image and sends something else is taken at its word")
    func declaredImage() {
        // Shown as the image it claims to be, and the viewer is what discovers
        // it cannot be drawn. Silently calling it binary would hide the bug.
        let inspection = BodyInspection.of(
            .complete(Data([0x00, 0x01, 0x02, 0x03])),
            contentType: ContentType.parse("image/png")
        )

        #expect(inspection.content == .image(ImageBody(format: "PNG")))
    }

    @Test("an SVG is the one image that is read as text")
    func svg() {
        let source = "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"
        let inspection = inspect(source, type: "image/svg+xml")

        #expect(textBody(inspection)?.text == source)
    }

    // MARK: - A whole exchange

    @Test("an exchange's two bodies are read with their own content types")
    func wholeExchange() throws {
        let exchange = TrafficExchange(
            id: "x",
            connection: ConnectionInfo(client: "127.0.0.1:1", scheme: .https, destination: nil),
            request: CapturedRequest(
                method: "POST",
                url: "https://api.example.com/v1/users",
                version: .http11,
                headers: Headers([
                    Header(name: "Content-Type", value: .text("application/json"))
                ]),
                body: capture(#"{"name":"Ada"}"#)
            ),
            response: CapturedResponse(
                status: 200,
                version: .http2,
                headers: Headers([Header(name: "Content-Type", value: .text("text/html"))]),
                body: capture("<html></html>")
            ),
            timing: Timing(startedAt: Timestamp(seconds: 1, nanos: 0))
        )
        let inspection = BodyInspection.of(exchange)

        #expect(inspection.exchangeID == "x")
        #expect(inspection.request.contentType?.essence == "application/json")
        #expect(textBody(inspection.request)?.language == .json)
        #expect(textBody(try #require(inspection.response))?.language == .html)
    }

    @Test("an exchange with no response has no response body to read")
    func exchangeWithoutResponse() {
        let inspection = BodyInspection.of(Fixtures.failedExchange(id: "y"))

        #expect(inspection.response == nil)
        #expect(inspection.request.content == .empty)
    }

    @Test("a text WebSocket payload that is JSON is laid out")
    func streamJSON() throws {
        let inspection = BodyInspection.ofStream(
            capture(#"{"id":1}"#),
            kind: .websocketText
        )
        let text = try #require(textBody(inspection))

        #expect(text.language == .json)
        #expect(text.formatted?.contains("\n") == true)
        #expect(text.text == #"{"id":1}"#)
    }

    @Test("a binary WebSocket payload is bytes even when those bytes are UTF-8")
    func streamBinaryStaysBinary() {
        let inspection = BodyInspection.ofStream(
            capture("hello"),
            kind: .websocketBinary
        )

        #expect(inspection.content == .binary)
    }

    @Test("SSE data is read as text")
    func streamSSE() throws {
        let inspection = BodyInspection.ofStream(capture("tick"), kind: .sse)

        #expect(try #require(textBody(inspection)).text == "tick")
        #expect(try #require(textBody(inspection)).formatted == nil)
    }
}

extension String {
    /// The start of the line `index` is on, for asserting about indentation.
    fileprivate func lineStart(of index: Index) -> Index {
        self[startIndex..<index].lastIndex(of: "\n").map { self.index(after: $0) } ?? startIndex
    }
}
