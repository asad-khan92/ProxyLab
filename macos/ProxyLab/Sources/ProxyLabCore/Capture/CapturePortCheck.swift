//  Whether 9090 or 9091 is already taken, without touching whoever has it.
//
//  A bind that succeeds means nothing is listening there, and the socket is
//  closed before anything is launched so the core can take the port. A bind
//  that fails with "address in use" means some other process has it. That
//  process is not signaled: the check only answers the question.

import Darwin
import Foundation

/// Which of the core's loopback ports are already bound.
@MainActor
public protocol CapturePortChecking: AnyObject {
    /// The ports from the set this checker watches that a bind could not take.
    func occupiedPorts() -> [UInt16]
}

/// Binds `127.0.0.1` on the explicit proxy port and the transparent bridge
/// port, and reports whichever are already in use.
@MainActor
public final class LoopbackPortCheck: CapturePortChecking {
    public var ports: [UInt16]

    public init(
        ports: [UInt16] = [
            UInt16(CoreLocations.proxyPort),
            UInt16(CoreLocations.transparentBridgePort),
        ]
    ) {
        self.ports = ports
    }

    public func occupiedPorts() -> [UInt16] {
        ports.filter(Self.isOccupied)
    }

    /// Whether something is already bound to `127.0.0.1:port`.
    ///
    /// A successful bind is closed immediately. Only `EADDRINUSE` counts as
    /// occupied; any other failure is not reported as a foreign process.
    static func isOccupied(_ port: UInt16) -> Bool {
        let descriptor = socket(AF_INET, SOCK_STREAM, 0)
        guard descriptor >= 0 else { return false }
        defer { close(descriptor) }

        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        var raw = in_addr()
        guard inet_pton(AF_INET, "127.0.0.1", &raw) == 1 else { return false }
        address.sin_addr = raw

        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        return bound != 0 && errno == EADDRINUSE
    }
}
