//  What the provider says when the app asks how it is doing.
//
//      app ── {"type":"status"} ──▶ provider      NETunnelProviderSession
//          ◀── ProviderStatus ────                .sendProviderMessage
//
//  The one channel between the two processes Apple provides, and the only one
//  used. It carries counts and a sentence — never a host, a URL, or anything a
//  flow carried — because what the app needs to know is whether the core is
//  getting the traffic, not what the traffic was. The traffic itself is the
//  core's, and reaches the app over the core's own IPC socket.

import Foundation

/// The provider's own account of itself.
public struct ProviderStatus: Codable, Equatable, Sendable {
    /// Whether the core's bridge answered the last time it was tried.
    public enum Health: String, Codable, Sendable {
        /// No flow has been offered to it yet.
        case unknown
        case reachable
        case unreachable
    }

    /// How many flows went where, since the provider started.
    ///
    /// `bridged`, `passedThrough`, and `proxyLab` are the running account:
    /// a flow that was handed over and then given back is taken out of
    /// `bridged`. The `claimed` and `bypassed` fields only increase, so a
    /// later look can still tell a TCP :443 claim from a QUIC bypass.
    public struct Counters: Codable, Equatable, Sendable {
        /// Handed to the core and not since given back.
        public var bridged = 0
        /// Carried directly because the core was not there.
        public var passedThrough = 0
        /// The core's own connections, left to macOS.
        public var proxyLab = 0
        /// TCP port 80 flows handed to the core.
        public var claimedTcp80 = 0
        /// TCP port 443 flows handed to the core.
        public var claimedTcp443 = 0
        /// TCP to a port other than 80 or 443. The network rules do not
        /// deliver these, so this stays zero unless one arrives anyway.
        public var bypassedPort = 0
        /// UDP, including QUIC on port 443, left to macOS.
        public var bypassedUdp = 0
        /// Loopback, left to macOS.
        public var bypassedLoopback = 0
        /// The core's own process, left to macOS.
        public var bypassedProxyLab = 0
        /// A claimed flow the bridge did not keep, then carried directly,
        /// and the flows left alone while that lasted.
        public var bridgeFallback = 0
        /// A flow whose destination could not be read.
        public var bypassedUnreadable = 0

        public init(
            bridged: Int = 0,
            passedThrough: Int = 0,
            proxyLab: Int = 0,
            claimedTcp80: Int = 0,
            claimedTcp443: Int = 0,
            bypassedPort: Int = 0,
            bypassedUdp: Int = 0,
            bypassedLoopback: Int = 0,
            bypassedProxyLab: Int = 0,
            bridgeFallback: Int = 0,
            bypassedUnreadable: Int = 0
        ) {
            self.bridged = bridged
            self.passedThrough = passedThrough
            self.proxyLab = proxyLab
            self.claimedTcp80 = claimedTcp80
            self.claimedTcp443 = claimedTcp443
            self.bypassedPort = bypassedPort
            self.bypassedUdp = bypassedUdp
            self.bypassedLoopback = bypassedLoopback
            self.bypassedProxyLab = bypassedProxyLab
            self.bridgeFallback = bridgeFallback
            self.bypassedUnreadable = bypassedUnreadable
        }
    }

    public var bridge: Health
    public var counters: Counters
    /// Why the bridge could not be reached, in a sentence of the provider's
    /// own choosing.
    public var lastBridgeFailure: String?

    public init(bridge: Health, counters: Counters, lastBridgeFailure: String?) {
        self.bridge = bridge
        self.counters = counters
        self.lastBridgeFailure = lastBridgeFailure
    }
}

/// A message from the app to the provider.
public enum ProviderMessage: String, Codable, Sendable {
    /// Ask for a ``ProviderStatus``.
    case status

    private struct Envelope: Codable {
        let type: ProviderMessage
    }

    public var encoded: Data {
        // A two-field struct of known types does not fail to encode.
        (try? JSONEncoder().encode(Envelope(type: self))) ?? Data()
    }

    public static func decode(_ data: Data) -> ProviderMessage? {
        (try? JSONDecoder().decode(Envelope.self, from: data))?.type
    }
}

extension ProviderStatus {
    public var encoded: Data {
        (try? JSONEncoder().encode(self)) ?? Data()
    }

    public static func decode(_ data: Data) -> ProviderStatus? {
        try? JSONDecoder().decode(ProviderStatus.self, from: data)
    }
}
