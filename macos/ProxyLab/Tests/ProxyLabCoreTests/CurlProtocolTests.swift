//  The cURL and replay operations on the wire.
//
//  What a request must carry, and what a response must be read as. Every JSON
//  string here is written by hand in the shape `core/ipc` sends, rather than
//  produced by the encoder under test: a test that builds its input with the
//  code it is testing agrees with itself no matter what the core sends.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("cURL and replay requests")
struct CurlRequestEncodingTests {
    private let encoder = JSONEncoder()

    /// The request as a dictionary, which is how the core reads it.
    private func encoded(_ operation: IPCOperation, id: String = "r1") throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: id, operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("the operation names are the ones the core implements")
    func names() {
        #expect(IPCOperation.parseCurl("curl x").name == "parse_curl")
        #expect(IPCOperation.curlFromTraffic(exchangeID: "x").name == "curl_from_traffic")
        #expect(
            IPCOperation.replayRequest(ReplayRequest(method: "GET", url: "x")).name
                == "replay_request"
        )
    }

    @Test("a parse carries the command and nothing else")
    func parse() throws {
        let json = try encoded(.parseCurl("curl 'https://example.com/'"))

        #expect(json["version"] as? Int == 1)
        #expect(json["type"] as? String == "parse_curl")
        #expect(json["curl"] as? String == "curl 'https://example.com/'")
        #expect(json["url"] == nil)
        #expect(json["method"] == nil)
    }

    @Test("a multiline command survives being encoded")
    func multiline() throws {
        let command = "curl 'https://example.com/api' \\\n  -H 'Accept: */*'"
        let json = try encoded(.parseCurl(command))

        #expect(json["curl"] as? String == command)
    }

    @Test("a command from traffic names the exchange")
    func fromTraffic() throws {
        let json = try encoded(.curlFromTraffic(exchangeID: "0199c0de-0000-7000-8000-00000000002a"))

        #expect(json["type"] as? String == "curl_from_traffic")
        #expect(json["exchange_id"] as? String == "0199c0de-0000-7000-8000-00000000002a")
    }

    @Test("a replay carries the request flattened, as the core parses it")
    func replay() throws {
        let request = ReplayRequest(
            method: "POST",
            url: "https://example.com/api",
            headers: Headers([
                Header(name: "Content-Type", value: .text("application/json")),
                Header(name: "Authorization", value: .text("Bearer TEST_TOKEN")),
            ]),
            body: Data(#"{"name":"Ada"}"#.utf8)
        )

        let json = try encoded(.replayRequest(request))

        #expect(json["type"] as? String == "replay_request")
        #expect(json["method"] as? String == "POST")
        #expect(json["url"] as? String == "https://example.com/api")
        // The body is base64, which is what the core reads it as.
        #expect(json["body"] as? String == Data(#"{"name":"Ada"}"#.utf8).base64EncodedString())

        let headers = try #require(json["headers"] as? [[String: Any]])
        #expect(headers.count == 2)
        #expect(headers[0]["name"] as? String == "Content-Type")
        #expect((headers[0]["value"] as? [String: Any])?["text"] as? String == "application/json")
        #expect(headers[1]["name"] as? String == "Authorization")
    }

    @Test("a replay with no headers and no body still carries both, empty")
    func emptyReplay() throws {
        let json = try encoded(.replayRequest(ReplayRequest(method: "GET", url: "http://x.test/")))

        #expect((json["headers"] as? [[String: Any]])?.isEmpty == true)
        #expect(json["body"] as? String == "")
    }

    @Test("duplicate headers are sent as duplicates, in order")
    func duplicates() throws {
        let request = ReplayRequest(
            method: "GET",
            url: "http://x.test/",
            headers: Headers([
                Header(name: "X-Trace", value: .text("one")),
                Header(name: "X-Trace", value: .text("two")),
            ])
        )

        let headers = try #require(try encoded(.replayRequest(request))["headers"] as? [[String: Any]])

        #expect(headers.count == 2)
        #expect((headers[0]["value"] as? [String: Any])?["text"] as? String == "one")
        #expect((headers[1]["value"] as? [String: Any])?["text"] as? String == "two")
    }

    @Test("a body that is not text survives as the bytes it is")
    func binaryBody() throws {
        let bytes = Data((0...255).map(UInt8.init))
        let request = ReplayRequest(method: "PUT", url: "http://x.test/", body: bytes)

        let json = try encoded(.replayRequest(request))
        let encoded = try #require(json["body"] as? String)

        #expect(Data(base64Encoded: encoded) == bytes)
    }
}

@Suite("cURL and replay responses")
struct CurlResponseDecodingTests {
    private let decoder = JSONDecoder()

    private func message(_ json: String) throws -> IPCMessage {
        try decoder.decode(IPCMessage.self, from: Data(json.utf8))
    }

    @Test("a parsed command comes back as the request it describes")
    func parsed() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"parse_curl","result":{\
            "request":{"method":"POST","url":"https://example.com/api",\
            "headers":[{"name":"Authorization","value":{"text":"Bearer TEST_TOKEN"}}],\
            "body":"eyJhIjoxfQ=="},\
            "notes":["compressed","insecure"]}}
            """

        guard case let .response(response) = try message(json),
            case let .parseCurl(parsed) = try #require(response.result)
        else {
            Issue.record("not a parse_curl response")
            return
        }

        let request = try #require(parsed.request)
        #expect(request.method == "POST")
        #expect(request.url == "https://example.com/api")
        #expect(request.headers.text("authorization") == "Bearer TEST_TOKEN")
        #expect(request.body == Data(#"{"a":1}"#.utf8))
        #expect(parsed.notes == [.compressed, .insecure])
        #expect(parsed.problem == nil)
    }

    @Test("a command that does not parse comes back as a problem, not a failure")
    func problem() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"parse_curl","result":{\
            "problem":{"code":"unsupported_option",\
            "message":"ProxyLab does not implement this cURL option (--foo)",\
            "option":"--foo"}}}
            """

        guard case let .response(response) = try message(json),
            case let .parseCurl(parsed) = try #require(response.result)
        else {
            Issue.record("not a parse_curl response")
            return
        }

        #expect(response.error == nil)
        #expect(parsed.request == nil)
        #expect(parsed.notes.isEmpty)
        #expect(parsed.problem?.code == .unsupportedOption)
        #expect(parsed.problem?.option == "--foo")
    }

    @Test("a note this build has not heard of is still decoded")
    func unknownNote() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"parse_curl","result":{\
            "request":{"method":"GET","url":"https://example.com/","headers":[],"body":""},\
            "notes":["something_new"]}}
            """

        guard case let .response(response) = try message(json),
            case let .parseCurl(parsed) = try #require(response.result)
        else {
            Issue.record("not a parse_curl response")
            return
        }

        #expect(parsed.notes == [CurlNote(rawValue: "something_new")])
        // Nothing is shown for a note the app has no words for.
        #expect(parsed.notes[0].message == nil)
    }

    @Test("a generated command comes back with whether it says everything")
    func generated() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"curl_from_traffic","result":{\
            "curl":"curl 'https://example.com/api' \\\\\\n  -X POST","truncated":true}}
            """

        guard case let .response(response) = try message(json),
            case let .curlFromTraffic(generated) = try #require(response.result)
        else {
            Issue.record("not a curl_from_traffic response")
            return
        }

        #expect(generated.curl == "curl 'https://example.com/api' \\\n  -X POST")
        #expect(generated.truncated)
    }

    @Test("a replay that was answered comes back with its status")
    func replayed() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"replay_request","result":{\
            "exchange_id":"0199c0de-0000-7000-8000-00000000002a","status":201,"error":null}}
            """

        guard case let .response(response) = try message(json),
            case let .replayRequest(result) = try #require(response.result)
        else {
            Issue.record("not a replay_request response")
            return
        }

        #expect(result.exchangeID == "0199c0de-0000-7000-8000-00000000002a")
        #expect(result.status == 201)
        #expect(result.error == nil)
        #expect(result.succeeded)
    }

    @Test("a replay that failed comes back with why, and is still an exchange")
    func replayFailed() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":true,"type":"replay_request","result":{\
            "exchange_id":"0199c0de-0000-7000-8000-00000000002a","status":null,\
            "error":{"kind":"upstream_connect",\
            "summary":"could not connect to the destination server"}}}
            """

        guard case let .response(response) = try message(json),
            case let .replayRequest(result) = try #require(response.result)
        else {
            Issue.record("not a replay_request response")
            return
        }

        #expect(!result.succeeded)
        #expect(result.status == nil)
        #expect(result.error?.kind == .upstreamConnect)
        #expect(result.exchangeID == "0199c0de-0000-7000-8000-00000000002a")
    }

    @Test("a refusal of a replay is an error response like any other")
    func refused() throws {
        let json = """
            {"version":1,"request_id":"r1","ok":false,\
            "error":{"code":"invalid_request",\
            "message":"ProxyLab sends http:// and https:// requests"}}
            """

        guard case let .response(response) = try message(json) else {
            Issue.record("not a response")
            return
        }

        #expect(response.result == nil)
        #expect(response.error?.code == .invalidRequest)
        #expect(response.error?.message == "ProxyLab sends http:// and https:// requests")
    }

    @Test("a request round-trips through the shape the core reads and writes")
    func roundTrip() throws {
        let original = ReplayRequest(
            method: "PATCH",
            url: "https://example.com/a?b=c",
            headers: Headers([
                Header(name: "Cookie", value: .text("session=TEST_SESSION")),
                Header(name: "X-Raw", value: .bytes(Data([0xff, 0x00]))),
            ]),
            body: Data([0x00, 0x01, 0xfe, 0xff])
        )

        let data = try JSONEncoder().encode(original)
        let back = try JSONDecoder().decode(ReplayRequest.self, from: data)

        #expect(back == original)
    }
}
