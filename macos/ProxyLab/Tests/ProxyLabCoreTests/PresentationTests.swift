//  Turning an exchange into the strings a row and a headline show.
//
//  The rule these are protecting: what the interface shows is what the core
//  captured. A URL that cannot be parsed is shown as it arrived; a status code
//  the world has not standardised is shown as the code it is; a duration is
//  shown at a scale a person reads without being rounded into a different
//  number.
//
//  How a *body* is shown is in `BodyInspectionTests`.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Traffic display")
struct TrafficDisplayTests {
    @Test("the host comes from the URL")
    func host() {
        let summary = Fixtures.summary(id: "a", url: "https://api.example.com/v1/users")

        #expect(TrafficDisplay.host(summary) == "api.example.com")
    }

    @Test("an HTTPS exchange names its host and path, not its CONNECT target")
    func httpsHostAndPath() {
        let summary = Fixtures.summary(
            id: "a",
            url: "https://api.example.com/v1/users?active=true"
        )

        #expect(TrafficDisplay.host(summary) == "api.example.com")
        #expect(TrafficDisplay.path(summary.url) == "/v1/users?active=true")
    }

    @Test("a URL with no path shows the root")
    func rootPath() {
        #expect(TrafficDisplay.path("https://example.com") == "/")
    }

    @Test("a URL that cannot be parsed is shown as it arrived")
    func unparsableURL() {
        #expect(TrafficDisplay.path("not a url at all") == "not a url at all")
    }

    @Test("an exchange with no URL host falls back to where the proxy connected")
    func destinationFallback() {
        let summary = ExchangeSummary(
            id: "a",
            startedAt: Timestamp(seconds: 1, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .http,
            destination: Destination(host: "example.com", port: 80),
            method: "GET",
            url: "/relative",
            version: .http11,
            completed: false
        )

        #expect(TrafficDisplay.host(summary) == "example.com")
    }

    @Test("how an exchange ended is a status, a failure, or neither yet")
    func outcome() {
        #expect(TrafficDisplay.outcome(Fixtures.summary(id: "a", status: 200)) == .status(200))

        let failed = ExchangeSummary(
            id: "b",
            startedAt: Timestamp(seconds: 1, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .http,
            method: "GET",
            url: "http://example.com/",
            version: .http11,
            completed: false,
            error: TrafficError(kind: .upstreamConnect, summary: "could not connect")
        )
        #expect(TrafficDisplay.outcome(failed) == .failed)

        #expect(TrafficDisplay.outcome(Fixtures.summary(id: "c", status: nil)) == .pending)
    }

    @Test("a status is its code and the reason the response carried")
    func status() {
        #expect(TrafficDisplay.status(200, reason: "OK") == "200 OK")
        #expect(TrafficDisplay.status(404, reason: "Not Found") == "404 Not Found")
        #expect(TrafficDisplay.status(500, reason: "Internal Server Error")
            == "500 Internal Server Error")
    }

    @Test("a response with no reason phrase shows its code alone")
    func statusWithoutReason() {
        // HTTP/2 carries no reason phrase at all, so this is every h2
        // response and not an edge case.
        #expect(TrafficDisplay.status(204, reason: nil) == "204")
        #expect(TrafficDisplay.status(204, reason: "") == "204")
    }

    @Test("a status code nobody standardised is still shown as itself")
    func unknownStatus() {
        #expect(TrafficDisplay.status(599, reason: nil) == "599")
        #expect(TrafficDisplay.status(799, reason: "Nonsense") == "799 Nonsense")
        #expect(TrafficDisplay.statusClass(599) == "Server error (5xx)")
        #expect(TrafficDisplay.statusClass(799) == "Outside every standard class")
    }

    @Test("a status class is read off the range the code is in")
    func statusClasses() {
        #expect(TrafficDisplay.statusClass(100) == "Informational (1xx)")
        #expect(TrafficDisplay.statusClass(201) == "Success (2xx)")
        #expect(TrafficDisplay.statusClass(301) == "Redirection (3xx)")
        #expect(TrafficDisplay.statusClass(418) == "Client error (4xx)")
        #expect(TrafficDisplay.statusClass(503) == "Server error (5xx)")
    }

    @Test("a byte count is shown at a scale a person reads, however large it is")
    func sizes() {
        #expect(TrafficDisplay.size(0) == Int64(0).formatted(.byteCount(style: .binary)))
        // Not the raw digits: twelve thousand bytes reads as kilobytes, and
        // two sizes an order of magnitude apart do not read the same.
        #expect(!TrafficDisplay.size(12_700).contains("12,700"))
        #expect(TrafficDisplay.size(12_700) != TrafficDisplay.size(1_910_000))
        // A body longer than the signed count Foundation formats is clamped
        // rather than trapped on.
        #expect(!TrafficDisplay.size(UInt64.max).isEmpty)
        // One formatter, the platform's byte count: bytes, then kilobytes,
        // then megabytes. The row does not keep a second one.
        #expect(TrafficDisplay.size(512) == Int64(512).formatted(.byteCount(style: .binary)))
        #expect(TrafficDisplay.size(2_048) == Int64(2_048).formatted(.byteCount(style: .binary)))
        #expect(
            TrafficDisplay.size(2_097_152) == Int64(2_097_152).formatted(.byteCount(style: .binary))
        )
        #expect(TrafficDisplay.size(512).localizedCaseInsensitiveContains("byte"))
        #expect(TrafficDisplay.size(2_048).localizedCaseInsensitiveContains("kb"))
        #expect(TrafficDisplay.size(2_097_152).localizedCaseInsensitiveContains("mb"))
    }

    @Test("a duration is shown at a scale a person reads")
    func durations() {
        #expect(TrafficDisplay.duration(milliseconds: nil) == nil)
        #expect(TrafficDisplay.duration(milliseconds: 0) == "0.00 ms")
        #expect(TrafficDisplay.duration(milliseconds: 0.25) == "0.25 ms")
        #expect(TrafficDisplay.duration(milliseconds: 84) == "84 ms")
        #expect(TrafficDisplay.duration(milliseconds: 999) == "999 ms")
        #expect(TrafficDisplay.duration(milliseconds: 1_000) == "1.00 s")
        #expect(TrafficDisplay.duration(milliseconds: 1_500) == "1.50 s")
    }

    @Test("a time is shown to the millisecond")
    func time() {
        // Two requests a browser fires together are otherwise the same row.
        let earlier = TrafficDisplay.time(Timestamp(seconds: 1_700_000_000, nanos: 120_000_000))
        let later = TrafficDisplay.time(Timestamp(seconds: 1_700_000_000, nanos: 123_000_000))

        #expect(earlier != later)
        #expect(earlier.hasSuffix(".120"))
        #expect(later.hasSuffix(".123"))
    }
}

@Suite("Socket location")
struct IPCSocketLocationTests {
    @Test("the socket is the one the core listens on by default")
    func defaultLocation() throws {
        let path = try IPCSocketLocation.resolve(environment: ["HOME": "/Users/dev"])

        #expect(path == "/Users/dev/.proxylab/run/proxylab.sock")
    }

    @Test("the environment variable the core reads overrides it")
    func environmentOverride() throws {
        let path = try IPCSocketLocation.resolve(
            environment: [
                "HOME": "/Users/dev",
                "PROXYLAB_IPC_SOCKET": "/tmp/other.sock",
            ]
        )

        #expect(path == "/tmp/other.sock")
        #expect(IPCSocketLocation.environmentVariable == "PROXYLAB_IPC_SOCKET")
    }

    @Test("an empty override is not an override")
    func emptyOverride() throws {
        let path = try IPCSocketLocation.resolve(
            environment: ["HOME": "/Users/dev", "PROXYLAB_IPC_SOCKET": ""]
        )

        #expect(path == "/Users/dev/.proxylab/run/proxylab.sock")
    }

    @Test("no home directory is a reason the app can explain")
    func noHome() {
        #expect(throws: IPCError.noHomeDirectory) {
            _ = try IPCSocketLocation.resolve(environment: [:])
        }
    }
}
