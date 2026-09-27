//  Reading a map-remote destination out of a URL.
//
//  The core keeps a destination in parts and never as a string, so the split
//  happens here before the rule is sent. What matters is that it makes the same
//  refusals `RemoteTarget::parse` makes: a destination this build accepts and
//  the core then refuses would be a rule that fails after a round trip, and one
//  this build refuses and the core would have taken is a rule that cannot be
//  written at all.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Map-remote destinations")
struct RemoteDestinationTests {
    @Test("a plain host becomes a scheme and a host, and nothing else")
    func plainHost() throws {
        let target = try RemoteTarget.parse("https://staging.example.com")

        #expect(target.scheme == .https)
        #expect(target.host == "staging.example.com")
        #expect(target.port == nil)
        #expect(target.path == nil)
        #expect(target.query == nil)
    }

    @Test("a bare trailing slash names no path, so the request keeps its own")
    func trailingSlash() throws {
        #expect(try RemoteTarget.parse("http://example.com/").path == nil)
    }

    @Test("a port, a path, and a query are each read out separately")
    func everyPart() throws {
        let target = try RemoteTarget.parse("http://example.com:8080/v2/api?debug=1&x=2")

        #expect(target.scheme == .http)
        #expect(target.host == "example.com")
        #expect(target.port == 8080)
        #expect(target.path == "/v2/api")
        #expect(target.query == "debug=1&x=2")
    }

    @Test("a query with no path is a query with no path")
    func queryOnly() throws {
        let target = try RemoteTarget.parse("https://example.com?only=query")

        #expect(target.path == nil)
        #expect(target.query == "only=query")
    }

    @Test("an IPv6 literal keeps its colons and loses its brackets")
    func ipv6() throws {
        let target = try RemoteTarget.parse("http://[::1]:8443/x")

        #expect(target.host == "::1")
        #expect(target.port == 8443)
        #expect(target.path == "/x")
        // Written back out, the brackets come back: that is what a URL needs.
        #expect(target.url == "http://[::1]:8443/x")
    }

    @Test("a scheme is read without regard to case")
    func schemeCase() throws {
        #expect(try RemoteTarget.parse("HTTPS://example.com").scheme == .https)
    }

    @Test("surrounding whitespace is not part of a destination")
    func trimmed() throws {
        #expect(try RemoteTarget.parse("  https://example.com  ").host == "example.com")
    }

    @Test("a destination is written back out as the URL it came from")
    func roundTrip() throws {
        for url in [
            "https://example.com",
            "http://example.com:8080",
            "https://example.com/v2",
            "https://example.com/v2?debug=1",
            "http://[::1]:9000/x",
        ] {
            #expect(try RemoteTarget.parse(url).url == url)
        }
    }

    @Test("a port that is the scheme's own is not written back out")
    func defaultPortOmitted() throws {
        #expect(try RemoteTarget.parse("https://example.com:443/x").url == "https://example.com/x")
    }

    @Test("what is not a destination is refused, and for the reason it is not")
    func refusals() {
        let cases: [(String, RemoteDestinationError)] = [
            ("", .empty),
            ("   ", .empty),
            ("example.com", .noScheme),
            ("ftp://example.com", .unsupportedScheme),
            ("file:///etc/passwd", .unsupportedScheme),
            ("https://example.com/x#frag", .fragment),
            ("https://user:secret@example.com", .userinfo),
            ("https://", .hostEmpty),
            ("https://example.com:0", .portInvalid),
            ("https://example.com:99999", .portInvalid),
            ("https://example.com:abc", .portInvalid),
            ("https://exa mple.com", .invalidURL),
        ]

        for (url, expected) in cases {
            #expect(throws: expected) {
                try RemoteTarget.parse(url)
            }
        }
    }

    @Test("parts assembled rather than typed are held to the same rule")
    func checkedParts() {
        #expect(throws: RemoteDestinationError.hostEmpty) {
            try RemoteTarget(scheme: .https, host: "").checked()
        }
        #expect(throws: RemoteDestinationError.hostInvalid) {
            try RemoteTarget(scheme: .https, host: "exa/mple.com").checked()
        }
        #expect(throws: RemoteDestinationError.portInvalid) {
            try RemoteTarget(scheme: .https, host: "example.com", port: 0).checked()
        }
        #expect(throws: RemoteDestinationError.pathInvalid) {
            try RemoteTarget(scheme: .https, host: "example.com", path: "v2").checked()
        }
        #expect(throws: RemoteDestinationError.queryInvalid) {
            try RemoteTarget(scheme: .https, host: "example.com", query: "a b").checked()
        }
    }

    @Test("every refusal has something a person can read")
    func messages() {
        for error in [
            RemoteDestinationError.empty, .noScheme, .unsupportedScheme, .invalidURL, .fragment,
            .userinfo, .hostEmpty, .hostInvalid, .portInvalid, .pathInvalid, .queryInvalid,
        ] {
            #expect(!error.message.isEmpty)
            #expect(error.message.hasSuffix("."))
        }
    }
}

@Suite("Map-local paths")
struct LocalFilePathTests {
    @Test("an empty path is refused before anything is looked at")
    func empty() {
        #expect(LocalFilePath.check("") == .empty)
        #expect(LocalFilePath.check("   ") == .empty)
    }

    @Test("a relative path is refused: the core has nothing to resolve it against")
    func relative() {
        #expect(LocalFilePath.check("mocks/users.json") == .notAbsolute)
        #expect(LocalFilePath.check("~/mocks/users.json") == .notAbsolute)
    }

    @Test("a path with nothing at it says so")
    func missing() {
        #expect(LocalFilePath.check("/this/does/not/exist/at/all.json") == .missing)
    }

    @Test("a folder chosen instead of a file says so")
    func directory() {
        #expect(LocalFilePath.check(NSTemporaryDirectory()) == .isDirectory)
    }

    @Test("a readable file is accepted, and is never opened to find out")
    func readableFile() throws {
        let path = NSTemporaryDirectory() + "proxylab-rule-fixture-\(UUID().uuidString).json"
        try Data(#"{"ok":true}"#.utf8).write(to: URL(fileURLWithPath: path))
        defer { try? FileManager.default.removeItem(atPath: path) }

        #expect(LocalFilePath.check(path) == nil)
    }

    @Test("every refusal has something a person can read")
    func messages() {
        for error in [
            LocalFileError.empty, .notAbsolute, .missing, .isDirectory, .unreadable,
        ] {
            #expect(!error.message.isEmpty)
        }
    }
}
