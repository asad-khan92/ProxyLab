//  Reading what a message said its body is.
//
//  The rule these are protecting: the header is kept exactly as it was
//  spelled, the pieces compared against are lowercased, and a message that
//  declared nothing is not given a type it did not send.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Content type")
struct ContentTypeTests {
    @Test("a bare media type is its two halves")
    func bare() throws {
        let type = try #require(ContentType.parse("application/json"))

        #expect(type.type == "application")
        #expect(type.subtype == "json")
        #expect(type.essence == "application/json")
        #expect(type.parameters.isEmpty)
        #expect(type.charset == nil)
    }

    @Test("the header is kept as it was spelled")
    func rawIsKept() throws {
        let type = try #require(ContentType.parse("Application/JSON; Charset=UTF-8"))

        #expect(type.raw == "Application/JSON; Charset=UTF-8")
        // Compared in lower case, shown in the sender's case.
        #expect(type.essence == "application/json")
        #expect(type.charset == "UTF-8")
        #expect(type.isJSON)
    }

    @Test("parameters are kept in the order they were written")
    func parameters() throws {
        let type = try #require(
            ContentType.parse("multipart/form-data; boundary=----x; charset=utf-8")
        )

        #expect(type.parameters.map(\.name) == ["boundary", "charset"])
        #expect(type.parameter("boundary") == "----x")
        #expect(type.parameter("BOUNDARY") == "----x")
    }

    @Test("a quoted parameter loses its quotes and keeps its semicolons")
    func quotedParameters() throws {
        let type = try #require(ContentType.parse(#"text/plain; note="a;b"; charset=utf-8"#))

        #expect(type.parameter("note") == "a;b")
        #expect(type.charset == "utf-8")
    }

    @Test("a declared character set resolves to the encoding it names")
    func encodings() {
        #expect(ContentType.parse("text/plain; charset=utf-8")?.declaredEncoding == .utf8)
        #expect(
            ContentType.parse("text/plain; charset=windows-1252")?.declaredEncoding
                == .windowsCP1252
        )
        #expect(
            ContentType.parse("text/plain; charset=iso-8859-1")?.declaredEncoding
                == .isoLatin1
        )
    }

    @Test("a character set nothing on this machine knows is no encoding at all")
    func unknownEncoding() {
        #expect(ContentType.parse("text/plain; charset=nonsense-9")?.declaredEncoding == nil)
        #expect(ContentType.parse("text/plain")?.declaredEncoding == nil)
    }

    @Test("a structured suffix says what the syntax underneath is")
    func suffixes() throws {
        #expect(ContentType.parse("application/vnd.api+json")?.isJSON == true)
        #expect(ContentType.parse("application/problem+json")?.suffix == "json")
        #expect(ContentType.parse("application/atom+xml")?.isXML == true)
        #expect(ContentType.parse("application/xhtml+xml")?.isHTML == true)
    }

    @Test("the textual types are the ones a person reads")
    func textual() {
        #expect(ContentType.parse("text/csv")?.isTextual == true)
        #expect(ContentType.parse("application/json")?.isTextual == true)
        #expect(ContentType.parse("application/x-www-form-urlencoded")?.isTextual == true)
        #expect(ContentType.parse("application/octet-stream")?.isTextual == false)
        #expect(ContentType.parse("image/png")?.isTextual == false)
    }

    @Test("an image says it is one")
    func images() {
        #expect(ContentType.parse("image/png")?.isImage == true)
        #expect(ContentType.parse("image/svg+xml")?.isImage == true)
        #expect(ContentType.parse("application/json")?.isImage == false)
    }

    @Test("something that is not a media type is not read as one")
    func nonsense() {
        #expect(ContentType.parse("") == nil)
        #expect(ContentType.parse("json") == nil)
        #expect(ContentType.parse("/json") == nil)
        #expect(ContentType.parse("application/") == nil)
    }

    @Test("the content type of a message is the header it carried, or nothing")
    func fromHeaders() {
        let headers = Headers([
            Header(name: "Accept", value: .text("*/*")),
            Header(name: "content-type", value: .text("application/json; charset=utf-8")),
        ])

        #expect(ContentType.of(headers)?.essence == "application/json")
        #expect(ContentType.of(Headers()) == nil)
    }

    @Test("a content type that was not text has no type to read")
    func bytesHeader() {
        let headers = Headers([Header(name: "Content-Type", value: .bytes(Data([0xFF])))])

        #expect(ContentType.of(headers) == nil)
    }
}
