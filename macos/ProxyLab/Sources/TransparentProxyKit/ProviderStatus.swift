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
    public struct Counters: Codable, Equatable, Sendable {
        /// Handed to the core.
        public var bridged = 0
        /// Carried directly because the core was not there.
        public var passedThrough = 0
        /// The core's own connections, left to macOS.
        public var proxyLab = 0

        public init(bridged: Int = 0, passedThrough: Int = 0, proxyLab: Int = 0) {
            self.bridged = bridged
            self.passedThrough = passedThrough
            self.proxyLab = proxyLab
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
