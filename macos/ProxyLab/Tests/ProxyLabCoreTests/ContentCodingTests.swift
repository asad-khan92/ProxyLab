//  A response body, from the bytes the proxy stored to the text the pane reads.
//
//  The capture is the entity body. Content-Encoding is still on it: the proxy
//  does not inflate what it forwards. IPC carries those bytes as base64, once,
//  and the model turns that base64 back into the same bytes before this
//  reading starts. gzip's magic is `1f 8b 08`, and the base64 of that magic is
//  `H4sI`. That string is what the undecodable view prints. It is not the body
//  the server sent, and nothing here treats a string that starts that way as
//  a coding.
//
//  The gzip, zlib, and raw-deflate fixtures were produced by Python's zlib,
//  not by the decoder under test. Brotli has no separate encoder in this
//  tree; that case is a round trip through the platform codec the reading uses.

import Compression
import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Content coding")
struct ContentCodingTests {
    private let jsonType = ContentType.parse("application/json")

    /// `{"ok":true,"n":1}` as gzip, from Python, with a fixed modification time.
    private let okGzip = ContentCodingTests.hex(
        "1f8b08000000000002ffab56cacf56b22a292a4dd551ca53b232ac050003080c4111000000"
    )
    /// The same member with a filename in the header, which a client may set.
    private let namedGzip = ContentCodingTests.hex(
        "1f8b08080000000002ff626f64792e6a736f6e00ab56cacf56b22a292a4dd551ca53b232ac050003080c4111000000"
    )
    /// The same JSON as a zlib wrapper, which is what HTTP `deflate` names.
    private let okZlib = ContentCodingTests.hex(
        "789cab56cacf56b22a292a4dd551ca53b232ac05003219055a"
    )
    /// The same JSON as raw DEFLATE, which is the other thing servers send.
    private let okRawDeflate = ContentCodingTests.hex(
        "ab56cacf56b22a292a4dd551ca53b232ac0500"
    )
    /// Gzip of an empty payload.
    private let emptyGzip = ContentCodingTests.hex(
        "1f8b08000000000002ff03000000000000000000"
    )
    private let okJSON = #"{"ok":true,"n":1}"#

    private func text(
        _ inspection: BodyInspection,
        _ comment: Comment = "the body should have decoded as text",
        sourceLocation: SourceLocation = #_sourceLocation
    ) throws -> TextBody {
        guard case let .text(text) = inspection.content else {
            Issue.record(comment, sourceLocation: sourceLocation)
            throw InspectionRead.notText
        }

        return text
    }

    // MARK: - Plain text, and the symptom

    @Test("plain UTF-8 JSON is JSON, and the reading is the capture")
    func plainJSON() throws {
        let bytes = Data(#"{"a":1,"b":[2]}"#.utf8)
        let inspection = BodyInspection.of(.complete(bytes), contentType: jsonType)
        let body = try text(inspection)

        #expect(body.language == .json)
        #expect(body.formatted?.contains("\"a\": 1") == true)
        #expect(body.text == #"{"a":1,"b":[2]}"#)
        #expect(inspection.presented == bytes)
        #expect(inspection.capture.data == bytes)
    }

    @Test("gzip bytes labelled JSON are not text, and their base64 starts H4sI")
    func gzipWithoutACodingIsNotSniffed() {
        // No Content-Encoding. The bytes are gzip, the type says JSON, and
        // the reading must not notice the magic. This is the view that
        // printed H4sI: base64 of the captured bytes, after IPC had already
        // turned its own base64 back into those bytes.
        let inspection = BodyInspection.of(.complete(okGzip), contentType: jsonType)

        #expect(inspection.content == .undecodable)
        #expect(inspection.presented == okGzip)
        #expect(okGzip.base64EncodedString().hasPrefix("H4sI"))
        #expect(String(data: okGzip, encoding: .utf8) == nil)
    }

    @Test("a gzip file with no Content-Encoding stays the file it is")
    func gzipFileStaysBinary() {
        let inspection = BodyInspection.of(
            .complete(okGzip),
            contentType: ContentType.parse("application/gzip")
        )

        #expect(inspection.content == .binary)
        #expect(inspection.presented == okGzip)
    }

    // MARK: - gzip JSON

    @Test("application/json with Content-Encoding gzip is the JSON")
    func gzipJSON() throws {
        let inspection = BodyInspection.of(
            .complete(okGzip),
            contentType: jsonType,
            contentEncoding: "gzip"
        )
        let body = try text(inspection)

        #expect(body.language == .json)
        #expect(body.text == okJSON)
        #expect(body.formatted?.contains("\"ok\": true") == true)
        #expect(body.tree?.children.map(\.key) == ["ok", "n"])
        #expect(inspection.presented == Data(okJSON.utf8))
        #expect(inspection.capture.data == okGzip)
        #expect(inspection.capture.size == UInt64(okGzip.count))
        #expect(ExchangeText.body(inspection) == okJSON)
    }

    @Test("x-gzip and a capitalised coding name are gzip")
    func gzipAliases() throws {
        for coding in ["x-gzip", "GZip", " gzip "] {
            let inspection = BodyInspection.of(
                .complete(okGzip),
                contentType: jsonType,
                contentEncoding: coding
            )

            #expect(try text(inspection).text == okJSON)
            #expect(inspection.capture.data == okGzip)
        }
    }

    @Test("a gzip member that carries a filename still inflates")
    func gzipFilename() throws {
        let inspection = BodyInspection.of(
            .complete(namedGzip),
            contentType: jsonType,
            contentEncoding: "gzip"
        )

        #expect(try text(inspection).text == okJSON)
    }

    @Test("gzip of an empty payload is an empty body, and the capture is kept")
    func gzipOfNothing() {
        let inspection = BodyInspection.of(
            .complete(emptyGzip),
            contentType: jsonType,
            contentEncoding: "gzip"
        )

        #expect(inspection.content == .empty)
        #expect(inspection.presented.isEmpty)
        #expect(inspection.capture.data == emptyGzip)
    }

    @Test("a character set is applied after the content coding")
    func charsetAfterGzip() throws {
        let payload = Data([0x43, 0x6F, 0x70, 0x79, 0xA9])
        let inspection = BodyInspection.of(
            .complete(try Packed.gzip(payload)),
            contentType: ContentType.parse("text/plain; charset=windows-1252"),
            contentEncoding: "gzip"
        )
        let body = try text(inspection)

        #expect(body.text == "Copy©")
        #expect(body.encoding == .windowsCP1252)
    }

    @Test("JSON that is not JSON is the text it is, after it is inflated")
    func invalidJSON() throws {
        let source = "<html>not json at all</html>"
        let packed = try Packed.gzip(Data(source.utf8))
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: jsonType,
            contentEncoding: "gzip"
        )
        let body = try text(inspection)

        #expect(body.language == .json)
        #expect(body.formatted == nil)
        #expect(body.text == source)
        #expect(inspection.capture.data == packed)
    }

    @Test("a coding that does not inflate leaves a readable body readable")
    func codingThatDoesNotMatch() throws {
        let source = #"{"a":1}"#
        let bytes = Data(source.utf8)
        for coding in ["gzip", "zstd", "deflate"] {
            let inspection = BodyInspection.of(
                .complete(bytes),
                contentType: jsonType,
                contentEncoding: coding
            )

            #expect(try text(inspection).text == source)
            #expect(inspection.presented == bytes)
        }
    }

    @Test("identity changes nothing, including a capture that was cut")
    func identity() throws {
        let bytes = Data(#"{"a":1,"#.utf8)
        let capture = BodyCapture(data: bytes, size: 4_000, truncated: true)
        let inspection = BodyInspection.of(
            capture,
            contentType: jsonType,
            contentEncoding: "identity"
        )

        #expect(try text(inspection).text == #"{"a":1,"#)
        #expect(inspection.capture.truncated)
        #expect(inspection.presented == bytes)
    }

    @Test("a truncated gzip capture is not inflated from its prefix")
    func truncatedGzip() {
        let capture = BodyCapture(data: okGzip, size: 10_000, truncated: true)
        let inspection = BodyInspection.of(capture, contentType: jsonType, contentEncoding: "gzip")

        #expect(inspection.content == .undecodable)
        #expect(inspection.presented == okGzip)
        #expect(inspection.capture.truncated)
    }

    @Test("a corrupt gzip stream is left as the bytes that arrived")
    func corruptGzip() {
        let bytes = Data([0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x01, 0x02])
        let inspection = BodyInspection.of(
            .complete(bytes),
            contentType: jsonType,
            contentEncoding: "gzip"
        )

        #expect(inspection.content == .undecodable)
        #expect(inspection.presented == bytes)
        #expect(inspection.capture.data == bytes)
    }

    @Test("an inflated body that is still too long to draw copies whole")
    func longGzipTextCopiesWhole() throws {
        let source = String(repeating: "x", count: BodyInspection.displayedCharacterLimit + 1_000)
        let packed = try Packed.gzip(Data(source.utf8))
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: ContentType.parse("text/plain"),
            contentEncoding: "gzip"
        )
        let body = try text(inspection)

        #expect(body.shortened)
        #expect(body.text.count == BodyInspection.displayedCharacterLimit)
        #expect(ExchangeText.body(inspection) == source)
        #expect(inspection.capture.data == packed)
    }

    // MARK: - deflate and brotli

    @Test("deflate as a zlib wrapper inflates to the JSON")
    func deflateZlib() throws {
        let inspection = BodyInspection.of(
            .complete(okZlib),
            contentType: jsonType,
            contentEncoding: "deflate"
        )

        #expect(try text(inspection).text == okJSON)
        #expect(inspection.capture.data == okZlib)
        #expect(inspection.presented == Data(okJSON.utf8))
    }

    @Test("deflate as a raw DEFLATE stream inflates to the JSON")
    func deflateRaw() throws {
        let inspection = BodyInspection.of(
            .complete(okRawDeflate),
            contentType: jsonType,
            contentEncoding: "deflate"
        )

        #expect(try text(inspection).text == okJSON)
        #expect(inspection.presented == Data(okJSON.utf8))
    }

    @Test("brotli JSON is JSON")
    func brotliJSON() throws {
        let source = #"{"br":true}"#
        let packed = try Packed.brotli(Data(source.utf8))
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: jsonType,
            contentEncoding: "br"
        )
        let body = try text(inspection)

        #expect(body.language == .json)
        #expect(body.text == source)
        #expect(body.formatted?.contains("\"br\": true") == true)
        #expect(inspection.capture.data == packed)
        #expect(inspection.presented == Data(source.utf8))
    }

    @Test("codings are removed in the reverse of the order they were applied")
    func chainedCoding() throws {
        let source = #"{"a":1}"#
        let deflated = try Packed.zlib(Data(source.utf8))
        let packed = try Packed.gzip(deflated)
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: jsonType,
            contentEncoding: "deflate, gzip"
        )

        #expect(try text(inspection).text == source)
        #expect(inspection.capture.data == packed)
    }

    @Test("repeated Content-Encoding headers are one list")
    func repeatedHeaders() throws {
        let source = #"{"a":1}"#
        let deflated = try Packed.zlib(Data(source.utf8))
        let packed = try Packed.gzip(deflated)
        let exchange = TrafficExchange(
            id: "chain",
            connection: ConnectionInfo(client: "127.0.0.1:1", scheme: .https, destination: nil),
            request: CapturedRequest(
                method: "GET",
                url: "https://api.example.com/v1/item",
                version: .http11
            ),
            response: CapturedResponse(
                status: 200,
                version: .http11,
                headers: Headers([
                    Header(name: "Content-Type", value: .text("application/json")),
                    Header(name: "Content-Encoding", value: .text("deflate")),
                    Header(name: "Content-Encoding", value: .text("gzip")),
                ]),
                body: .complete(packed)
            ),
            timing: Timing(startedAt: Timestamp(seconds: 1, nanos: 0))
        )
        let response = try #require(BodyInspection.of(exchange).response)

        #expect(try text(response).text == source)
        #expect(response.capture.data == packed)
    }

    @Test("an expansion past the reading cap is left compressed")
    func expansionCap() throws {
        let payload = Data(repeating: UInt8(ascii: "a"), count: 400)
        let packed = try Packed.gzip(payload)

        #expect(ContentCoding.remove("gzip", from: packed, truncated: false, limit: 64) == .failed)

        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: ContentType.parse("text/plain"),
            contentEncoding: "gzip"
        )

        #expect(try text(inspection).text == String(repeating: "a", count: 400))
    }

    // MARK: - Binary stays binary

    @Test("an empty body stays empty when a coding is declared and there are no bytes")
    func empty() {
        let inspection = BodyInspection.of(.empty, contentType: jsonType, contentEncoding: "gzip")

        #expect(inspection.content == .empty)
        #expect(inspection.presented.isEmpty)
        #expect(ExchangeText.body(inspection).isEmpty)
    }

    @Test("a binary payload stays binary after it is inflated")
    func binaryStaysBinary() throws {
        let payload = Data([0x0A, 0x03, 0xFF, 0xFE, 0xFD])
        let packed = try Packed.gzip(payload)
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: ContentType.parse("application/octet-stream"),
            contentEncoding: "gzip"
        )

        #expect(inspection.content == .binary)
        #expect(inspection.presented == payload)
        #expect(inspection.capture.data == packed)
        #expect(ExchangeText.body(inspection) == payload.base64EncodedString())
    }

    @Test("a gzipped image is the image, and the capture stays gzip")
    func gzippedImage() throws {
        let png = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
            + Data(repeating: 0, count: 8)
        let packed = try Packed.gzip(png)
        let inspection = BodyInspection.of(
            .complete(packed),
            contentType: ContentType.parse("image/png"),
            contentEncoding: "gzip"
        )

        #expect(inspection.content == .image(ImageBody(format: "PNG")))
        #expect(inspection.presented == png)
        #expect(inspection.capture.data == packed)
    }

    // MARK: - The socket

    @Test("gzip JSON arrives as gzip bytes and is read as JSON")
    func gzipAcrossIPC() async throws {
        let base64 = "H4sIAAAAAAAC/6tWys9WsiopKk3VUcpTsjKsBQADCAxBEQAAAA=="
        #expect(okGzip.base64EncodedString() == base64)
        #expect(base64.hasPrefix("H4sI"))

        let exchange = """
            {"id":"0199c0de-0000-7000-8000-0000000000aa",\
            "connection":{"client":"127.0.0.1:9","scheme":"https",\
            "destination":{"host":"api.example.com","port":443}},\
            "request":{"method":"GET","url":"https://api.example.com/v1/status",\
            "version":"HTTP/1.1","headers":[],\
            "body":{"data":"","size":0,"truncated":false}},\
            "response":{"status":200,"reason":"OK","version":"HTTP/1.1",\
            "headers":[{"name":"Content-Type","value":{"text":"application/json"}},\
            {"name":"Content-Encoding","value":{"text":"gzip"}},\
            {"name":"Content-Length","value":{"text":"\(okGzip.count)"}}],\
            "body":{"data":"\(base64)","size":\(okGzip.count),"truncated":false}},\
            "error":null,\
            "timing":{"started_at":{"seconds":1700000000,"nanos":0},\
            "upstream_connect_micros":null,"upstream_response_micros":1000,\
            "completed_micros":2000}}
            """

        let server = try ScriptedSocketServer()
        try server.start { line in
            .reply([Fixtures.getResponse(requestID: requestID(in: line) ?? "?", exchange: exchange)])
        }
        defer { server.stop() }

        let client = IPCClient.overSocket(at: server.path)
        try await client.connect()
        defer { Task { await client.disconnect() } }

        let fetched = try await client.traffic(id: "0199c0de-0000-7000-8000-0000000000aa")
        let stored = try #require(fetched.response)

        // One base64 decode. The stored body is the gzip bytes, not the
        // characters of the transport string, and not those bytes decoded again.
        #expect(stored.body.data == okGzip)
        #expect(stored.body.data.count == 37)
        #expect(stored.body.size == 37)
        #expect(String(data: stored.body.data, encoding: .utf8) == nil)
        #expect(stored.headers.text("Content-Type") == "application/json")
        #expect(stored.headers.text("Content-Encoding") == "gzip")
        #expect(stored.headers.text("Content-Length") == "37")
        #expect(stored.headers.text("Transfer-Encoding") == nil)

        let reading = try #require(BodyInspection.of(fetched).response)
        let body = try text(reading)

        #expect(reading.capture.data == okGzip)
        #expect(reading.presented == Data(okJSON.utf8))
        #expect(body.language == .json)
        #expect(body.text == okJSON)
        #expect(body.formatted?.contains("\n") == true)
        #expect(ExchangeText.body(reading) == okJSON)
    }
}

/// Why a reading was not text, so a test stops instead of continuing.
private enum InspectionRead: Error {
    case notText
}

/// Gzip, zlib, and brotli fixtures built with the platform codec.
///
/// The interoperability fixtures in the suite are not built here. These exist
/// so a test can pack a payload the Python fixtures do not carry — an image,
/// a long body, a chain of two codings — and still have a trailer the reader
/// will check.
private enum Packed {
    static func gzip(_ payload: Data) throws -> Data {
        let deflate = try raw(payload, algorithm: COMPRESSION_ZLIB)
        var archive = Data([0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF])
        archive.append(deflate)
        archive.append(contentsOf: littleEndian(crc32(payload)))
        let size = UInt32(truncatingIfNeeded: payload.count)
        archive.append(contentsOf: littleEndian(size))

        return archive
    }

    static func zlib(_ payload: Data) throws -> Data {
        let deflate = try raw(payload, algorithm: COMPRESSION_ZLIB)
        var archive = Data([0x78, 0x9C])
        archive.append(deflate)
        let sum = adler32(payload)
        archive.append(UInt8((sum >> 24) & 0xFF))
        archive.append(UInt8((sum >> 16) & 0xFF))
        archive.append(UInt8((sum >> 8) & 0xFF))
        archive.append(UInt8(sum & 0xFF))

        return archive
    }

    static func brotli(_ payload: Data) throws -> Data {
        try raw(payload, algorithm: COMPRESSION_BROTLI)
    }

    private static func raw(_ payload: Data, algorithm: compression_algorithm) throws -> Data {
        let bound = payload.count + 64 + max(payload.count / 8, 64)
        var destination = [UInt8](repeating: 0, count: bound)
        let written: Int = payload.withUnsafeBytes { src in
            guard let srcBase = src.bindMemory(to: UInt8.self).baseAddress, !payload.isEmpty else {
                return 0
            }

            return destination.withUnsafeMutableBytes { dst in
                guard let dstBase = dst.bindMemory(to: UInt8.self).baseAddress else { return 0 }

                return Int(
                    compression_encode_buffer(
                        dstBase,
                        dst.count,
                        srcBase,
                        payload.count,
                        nil,
                        algorithm
                    )
                )
            }
        }
        try #require(written > 0, "the platform codec did not pack the fixture")

        return Data(destination.prefix(written))
    }

    private static func littleEndian(_ value: UInt32) -> [UInt8] {
        [
            UInt8(value & 0xFF),
            UInt8((value >> 8) & 0xFF),
            UInt8((value >> 16) & 0xFF),
            UInt8((value >> 24) & 0xFF),
        ]
    }

    private static func crc32(_ data: Data) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in data {
            crc ^= UInt32(byte)
            for _ in 0..<8 {
                crc = (crc & 1) == 1 ? (0xEDB8_8320 ^ (crc >> 1)) : (crc >> 1)
            }
        }

        return ~crc
    }

    private static func adler32(_ data: Data) -> UInt32 {
        var a: UInt32 = 1
        var b: UInt32 = 0
        for byte in data {
            a = (a + UInt32(byte)) % 65521
            b = (b + a) % 65521
        }

        return (b << 16) | a
    }
}

extension ContentCodingTests {
    fileprivate static func hex(_ string: String) -> Data {
        precondition(string.count.isMultiple(of: 2))

        var data = Data()
        data.reserveCapacity(string.count / 2)
        var index = string.startIndex
        while index < string.endIndex {
            let next = string.index(index, offsetBy: 2)
            guard let byte = UInt8(string[index..<next], radix: 16) else {
                preconditionFailure("a fixture hex digit was not hex")
            }
            data.append(byte)
            index = next
        }

        return data
    }
}

/// The `request_id` of a line the client sent.
private func requestID(in line: String) -> String? {
    guard
        let object = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
    else {
        return nil
    }

    return object["request_id"] as? String
}
