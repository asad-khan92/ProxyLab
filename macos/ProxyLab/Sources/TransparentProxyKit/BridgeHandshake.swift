//  What the provider says to the core's bridge, and what it makes of the
//  answer.
//
//      provider ── CONNECT 93.184.215.14:443 HTTP/1.1 ──▶ 127.0.0.1:9091
//                  ProxyLab-Source-Pid: 812
//               ◀─ HTTP/1.1 200 Connection Established ─
//               …then the application's own bytes, both ways
//
//  The bridge speaks `CONNECT` because the core already reads it: the target
//  goes through the same parser an explicit proxy client's does, and there is
//  no framing of ProxyLab's own to keep in step on two sides of a boundary.
//  The one header that is ProxyLab's names the process the flow came from, so
//  the core can recognise its own outbound connections coming back to it —
//  `core/proxy-engine/src/transparent/mod.rs` is the other half of this file.

import Foundation

/// The `CONNECT` a flow is announced with, and the reply it gets.
public enum BridgeHandshake {
    /// The header the source process is named in. The core's
    /// `SOURCE_PID_HEADER`, spelled as it is on the wire.
    public static let sourcePIDHeader = "ProxyLab-Source-Pid"

    /// The longest reply head that is read before the reply is given up on.
    ///
    /// The bridge's answer is a status line and one or two headers; anything
    /// longer is not the bridge.
    public static let maximumReplyHead = 8 * 1_024

    /// The request announcing a flow to `host` on `port`.
    ///
    /// `nil` when `host` could not be written into a request line — a
    /// character that would end the line or the target early. The provider
    /// then carries the flow itself rather than send the core something it
    /// did not mean.
    ///
    /// - Parameter sourcePID: the process the flow came from, when the
    ///   system said. Left out when it did not, which the core treats as a
    ///   flow that is not its own.
    public static func request(host: String, port: UInt16, sourcePID: Int32?) -> Data? {
        guard let authority = authority(host: host, port: port) else { return nil }

        var head = "CONNECT \(authority) HTTP/1.1\r\nHost: \(authority)\r\n"
        if let sourcePID {
            head += "\(sourcePIDHeader): \(sourcePID)\r\n"
        }
        head += "\r\n"

        return Data(head.utf8)
    }

    /// `host:port`, with an IPv6 literal in brackets.
    static func authority(host: String, port: UInt16) -> String? {
        let bare = host.hasPrefix("[") && host.hasSuffix("]")
            ? String(host.dropFirst().dropLast())
            : host

        guard port != 0, !bare.isEmpty,
              bare.unicodeScalars.allSatisfy(isAuthorityCharacter)
        else { return nil }

        return bare.contains(":") ? "[\(bare)]:\(port)" : "\(bare):\(port)"
    }

    /// Letters, digits, and the punctuation an address or a host name uses.
    ///
    /// Everything else is refused, and in particular the space, the `@`, and
    /// the line breaks that would make a target into something else.
    private static func isAuthorityCharacter(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar {
        case "a"..."z", "A"..."Z", "0"..."9", ".", "-", ":", "%", "_": true
        default: false
        }
    }

    /// What the bridge said.
    public enum Reply: Equatable, Sendable {
        /// `200`: the destination is connected and the flow is the core's.
        case established
        /// `508`: the flow came from the core itself. It is carried directly,
        /// and the process is not offered to the core again.
        case passBack
        /// Anything else: the destination was refused or could not be
        /// reached, and the application's connection fails.
        case refused(status: Int)
    }

    /// Where reading a reply has got to.
    public enum HeadReading: Equatable, Sendable {
        /// More bytes are needed.
        case incomplete
        /// The head ended `length` bytes in. Anything after it already
        /// belongs to the flow.
        case complete(Reply, length: Int)
        /// What arrived is not a reply the bridge would send.
        case malformed
    }

    /// Reads the reply head out of what has arrived so far.
    public static func read(_ bytes: Data) -> HeadReading {
        let terminator = Data("\r\n\r\n".utf8)
        guard let end = bytes.range(of: terminator) else {
            return bytes.count > maximumReplyHead ? .malformed : .incomplete
        }

        let length = end.upperBound - bytes.startIndex
        guard length <= maximumReplyHead + terminator.count,
              let head = String(data: bytes[bytes.startIndex..<end.lowerBound], encoding: .utf8),
              let statusLine = head.split(separator: "\r\n", omittingEmptySubsequences: false).first
        else { return .malformed }

        let fields = statusLine.split(separator: " ", maxSplits: 2)
        guard fields.count >= 2,
              fields[0] == "HTTP/1.1" || fields[0] == "HTTP/1.0",
              fields[1].count == 3,
              let status = Int(fields[1])
        else { return .malformed }

        switch status {
        case 200: return .complete(.established, length: length)
        case 508: return .complete(.passBack, length: length)
        default: return .complete(.refused(status: status), length: length)
        }
    }
}
