//  What the provider says to the core's bridge, byte for byte, and how it
//  reads the answer.
//
//  The other half of this is `core/proxy-engine/tests/transparent_capture.rs`,
//  which sends the same request to a real bridge. What is pinned here is that
//  the request is one the core's `CONNECT` parser reads, and that nothing a
//  host name could contain turns it into a different request.

import Foundation
import Testing

@testable import TransparentProxyKit

@Suite("The bridge handshake")
struct BridgeHandshakeTests {
    private func text(_ data: Data?) -> String? {
        data.flatMap { String(data: $0, encoding: .utf8) }
    }

    @Test("an IPv4 flow is announced as its address and port, with its source")
    func ipv4() {
        let request = text(BridgeHandshake.request(host: "93.184.215.14", port: 443, sourcePID: 812))

        #expect(request == "CONNECT 93.184.215.14:443 HTTP/1.1\r\n"
            + "Host: 93.184.215.14:443\r\n"
            + "ProxyLab-Source-Pid: 812\r\n\r\n")
    }

    @Test("an IPv6 address is bracketed, once")
    func ipv6() {
        let bare = text(BridgeHandshake.request(host: "2001:db8::1", port: 443, sourcePID: nil))
        let bracketed = text(BridgeHandshake.request(host: "[2001:db8::1]", port: 443, sourcePID: nil))

        #expect(bare?.hasPrefix("CONNECT [2001:db8::1]:443 HTTP/1.1\r\n") == true)
        #expect(bare == bracketed)
    }

    @Test("a flow whose source is not known names none")
    func noSource() {
        let request = text(BridgeHandshake.request(host: "example.com", port: 80, sourcePID: nil))

        #expect(request == "CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\n\r\n")
    }

    @Test("a host that could break the request line is refused rather than sent")
    func injection() {
        for host in [
            "example.com\r\nX-Evil: 1", "exa mple.com", "user@example.com",
            "example.com/", "", "[]", "exa\u{0}mple",
        ] {
            #expect(BridgeHandshake.request(host: host, port: 443, sourcePID: 1) == nil, "\(host)")
        }
    }

    @Test("port zero is not a destination")
    func portZero() {
        #expect(BridgeHandshake.request(host: "example.com", port: 0, sourcePID: 1) == nil)
    }

    @Test("200 is an established flow, and the head's length is where the flow begins")
    func established() {
        let reply = Data("HTTP/1.1 200 Connection Established\r\n\r\nABC".utf8)

        #expect(BridgeHandshake.read(reply) == .complete(.established, length: reply.count - 3))
    }

    @Test("508 hands the flow back to be carried directly")
    func passBack() {
        let reply = Data(
            "HTTP/1.1 508 Loop Detected\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\n".utf8
        )

        #expect(BridgeHandshake.read(reply) == .complete(.passBack, length: reply.count))
    }

    @Test("any other status is a refusal the application sees")
    func refused() {
        for status in [400, 403, 405, 500, 502] {
            let reply = Data("HTTP/1.1 \(status) No\r\n\r\n".utf8)

            #expect(BridgeHandshake.read(reply) == .complete(.refused(status: status), length: reply.count))
        }
    }

    @Test("half a head is waited for")
    func incomplete() {
        #expect(BridgeHandshake.read(Data()) == .incomplete)
        #expect(BridgeHandshake.read(Data("HTTP/1.1 200 Connection Est".utf8)) == .incomplete)
        #expect(BridgeHandshake.read(Data("HTTP/1.1 200 OK\r\n".utf8)) == .incomplete)
    }

    @Test("something that is not the bridge talking is malformed")
    func malformed() {
        for reply in ["SSH-2.0-OpenSSH\r\n\r\n", "HTTP/1.1 2000 Huh\r\n\r\n", "HTTP/2 200\r\n\r\n", "\r\n\r\n"] {
            #expect(BridgeHandshake.read(Data(reply.utf8)) == .malformed, "\(reply)")
        }
    }

    @Test("a head that never ends is given up on rather than read for ever")
    func endless() {
        let flood = Data(repeating: UInt8(ascii: "a"), count: BridgeHandshake.maximumReplyHead + 1)

        #expect(BridgeHandshake.read(flood) == .malformed)
    }
}
