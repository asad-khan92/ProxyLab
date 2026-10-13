//  Which flows the provider hands to the core, and which it leaves alone.
//
//      new flow
//           │
//           ├─ UDP, including QUIC on 443 ─────────────▶ not claimed (counted)
//           ├─ loopback, or TCP not on port 80/443 ────▶ not claimed
//           ├─ from a process that is ProxyLab's core ─▶ not claimed
//           ├─ the core was unreachable a moment ago ──▶ not claimed
//           └─ TCP 80 or 443 ──────────────────────────▶ bridged to the core
//
//  A flow that is not claimed goes out exactly as it would have with no
//  provider installed: macOS carries it, and ProxyLab never sees its bytes.
//  UDP is not in the rules the provider installs. A QUIC flow therefore never
//  arrives here and cannot be counted; `decide` still refuses one if it does.
//
//  # Loops
//
//  The core's own connections to origin servers are TCP flows to port 443
//  like any other, and the provider sees them. The first one from a core
//  process is bridged like any other flow, and the core recognises its own
//  process in the handshake and answers `508` — at which point this policy
//  learns that process and never offers it again. So a loop costs one refused
//  handshake per core process, and cannot recurse: the `508` is answered
//  before the core connects to anything.
//
//  The process is remembered by its audit token's pid *and* pid version, so a
//  pid the system later reuses for some other program is not mistaken for
//  the core.
//
//  # When the core is not there
//
//  A bridge that refuses the connection means the core is not running. Every
//  flow the provider claimed would then fail, and the Mac would lose its web
//  traffic to a debugging tool that is not even open. So a failed bridge
//  connection is remembered for a few seconds, during which flows are not
//  claimed at all; the flow that found out is carried directly by the
//  provider. Nothing is captured meanwhile, and the app says so.

import Foundation
import os

/// A process, as a flow's audit token names it.
public struct SourceProcess: Hashable, Sendable {
    public let pid: Int32
    /// Incremented by the kernel each time a pid is reused, so the pair names
    /// one process for its whole life and never another.
    public let pidVersion: Int32

    public init(pid: Int32, pidVersion: Int32) {
        self.pid = pid
        self.pidVersion = pidVersion
    }

    /// The process an `audit_token_t` names.
    ///
    /// An audit token is eight 32-bit words, and `libbsm` reads the pid from
    /// the sixth and the pid version from the eighth — which is all
    /// `audit_token_to_pid` and `audit_token_to_pidversion` do. Read here
    /// directly so that this type needs no system library and can be tested
    /// with a token built in the test.
    ///
    /// `nil` for anything that is not the size of a token.
    public init?(auditToken: Data) {
        guard auditToken.count == 8 * MemoryLayout<UInt32>.size else { return nil }

        let words = auditToken.withUnsafeBytes { raw in
            (0..<8).map { raw.loadUnaligned(fromByteOffset: $0 * 4, as: UInt32.self) }
        }
        self.init(pid: Int32(bitPattern: words[5]), pidVersion: Int32(bitPattern: words[7]))
    }
}

/// TCP or UDP, as the system classified the flow.
public enum FlowTransport: Equatable, Sendable {
    case tcp
    case udp
}

/// One flow the provider asks macOS to deliver.
public struct TransparentNetworkRule: Equatable, Sendable {
    public var transport: FlowTransport
    public var port: UInt16
    /// Handed to the core. `false` means the provider counts the flow and
    /// returns it to macOS.
    public var claimed: Bool

    public init(transport: FlowTransport, port: UInt16, claimed: Bool) {
        self.transport = transport
        self.port = port
        self.claimed = claimed
    }
}

/// An address prefix the provider names in a network rule.
public struct NetworkPrefix: Equatable, Sendable {
    public var address: String
    public var prefix: Int

    public init(address: String, prefix: Int) {
        self.address = address
        self.prefix = prefix
    }
}

/// What to do with one new flow.
public enum FlowDecision: Equatable, Sendable {
    /// Hand it to the core.
    case bridge
    /// Leave it to macOS.
    case leave(FlowDecision.Reason)

    public enum Reason: Equatable, Sendable {
        /// Loopback: ProxyLab's own bridge and control traffic, and anything
        /// else that never leaves this Mac.
        case loopback
        /// TCP, and not a port this provider carries.
        case port
        /// UDP. Port 443 is QUIC, which this provider does not capture.
        case udp
        /// The flow is from ProxyLab's core.
        case proxyLab
        /// The core was unreachable a moment ago.
        case coreUnavailable
        /// The destination could not be read.
        case unreadable
    }
}

/// The destination and origin of one new flow.
public struct FlowCandidate: Sendable {
    public let host: String
    public let port: UInt16
    public let source: SourceProcess?
    public let transport: FlowTransport

    public init(
        host: String,
        port: UInt16,
        source: SourceProcess?,
        transport: FlowTransport = .tcp
    ) {
        self.host = host
        self.port = port
        self.source = source
        self.transport = transport
    }
}

/// The provider's decisions, and what it has learned while making them.
///
/// Called from whichever queue the system delivers a flow on, so everything
/// it keeps is behind one lock, and every call is short.
public final class FlowPolicy: Sendable {
    /// The ports whose TCP flows are offered to the core.
    public static let interceptedPorts: Set<UInt16> = [80, 443]

    /// What the provider asks macOS to deliver, once per address family.
    ///
    /// TCP 80 and 443 are claimed. UDP is absent on purpose. Returning a UDP
    /// flow from `handleNewUDPFlow` is specified, on current macOS, as
    /// terminating it — which would break QUIC rather than count it — so
    /// QUIC is not asked for. `decide` still leaves a UDP flow unclaimed if
    /// one is offered. Other TCP ports are absent too, and are not counted.
    public static let deliveredRules: [TransparentNetworkRule] = [
        TransparentNetworkRule(transport: .tcp, port: 80, claimed: true),
        TransparentNetworkRule(transport: .tcp, port: 443, claimed: true),
    ]

    /// IPv4 and IPv6, the whole of each. A family missing from this list is
    /// a family whose traffic bypasses the provider.
    public static let includedNetworks: [NetworkPrefix] = [
        NetworkPrefix(address: "0.0.0.0", prefix: 0),
        NetworkPrefix(address: "::", prefix: 0),
    ]

    /// Loopback, excluded for every protocol the provider asks about.
    public static let excludedLoopback: [NetworkPrefix] = [
        NetworkPrefix(address: "127.0.0.0", prefix: 8),
        NetworkPrefix(address: "::1", prefix: 128),
    ]

    /// What the diagnostics sheet says about the rules above.
    ///
    /// No host and no URL: it is the shape of the capture, not the traffic.
    public static let captureScope =
        "TCP ports 80 and 443 are claimed on IPv4 and IPv6, except loopback. "
        + "UDP, including QUIC on port 443, is not delivered, so it bypasses capture and is not counted. "
        + "Other TCP ports are not delivered either. "
        + "A TLS failure on a claimed TCP flow is stored as a failed exchange."

    /// How long an unreachable core is left alone before it is tried again.
    public static let coreRetryInterval: TimeInterval = 5

    private struct State {
        var own: Set<SourceProcess> = []
        var coreUnavailableUntil: Date?
        var counters = ProviderStatus.Counters()
        var lastBridgeFailure: String?
        var bridgeHealth: ProviderStatus.Health = .unknown
    }

    private let state = OSAllocatedUnfairLock(initialState: State())

    public init() {}

    /// Decides what happens to `candidate`, and counts the decision.
    public func decide(_ candidate: FlowCandidate, now: Date = Date()) -> FlowDecision {
        let decision = Self.decision(for: candidate)

        return state.withLock { state in
            let final: FlowDecision
            if decision != .bridge {
                final = decision
            } else if let source = candidate.source, state.own.contains(source) {
                final = .leave(.proxyLab)
            } else if let until = state.coreUnavailableUntil, now < until {
                final = .leave(.coreUnavailable)
            } else {
                final = .bridge
            }

            switch final {
            case .bridge:
                state.counters.bridged += 1
                if candidate.port == 80 {
                    state.counters.claimedTcp80 += 1
                } else {
                    state.counters.claimedTcp443 += 1
                }
            case .leave(.proxyLab):
                state.counters.proxyLab += 1
                state.counters.bypassedProxyLab += 1
            case .leave(.coreUnavailable):
                state.counters.passedThrough += 1
                state.counters.bridgeFallback += 1
            case .leave(.loopback):
                state.counters.bypassedLoopback += 1
            case .leave(.port):
                state.counters.bypassedPort += 1
            case .leave(.udp):
                state.counters.bypassedUdp += 1
            case .leave(.unreadable):
                state.counters.bypassedUnreadable += 1
            }

            return final
        }
    }

    /// Records that the core refused `source` as its own.
    public func learnProxyLab(_ source: SourceProcess?) {
        state.withLock { state in
            if let source { state.own.insert(source) }
            state.counters.proxyLab += 1
            state.counters.bypassedProxyLab += 1
            state.counters.bridged = max(0, state.counters.bridged - 1)
        }
    }

    /// Records that the bridge could not be reached, and why.
    public func coreUnreachable(_ reason: String, now: Date = Date()) {
        state.withLock { state in
            state.coreUnavailableUntil = now.addingTimeInterval(Self.coreRetryInterval)
            state.lastBridgeFailure = reason
            state.bridgeHealth = .unreachable
            state.counters.passedThrough += 1
            state.counters.bridgeFallback += 1
            state.counters.bridged = max(0, state.counters.bridged - 1)
        }
    }

    /// Records that the bridge answered.
    public func coreReached() {
        state.withLock { state in
            state.coreUnavailableUntil = nil
            state.bridgeHealth = .reachable
        }
    }

    /// What the app is told when it asks.
    public func status() -> ProviderStatus {
        state.withLock { state in
            ProviderStatus(
                bridge: state.bridgeHealth,
                counters: state.counters,
                lastBridgeFailure: state.lastBridgeFailure
            )
        }
    }

    /// The part of the decision that depends on the flow alone.
    static func decision(for candidate: FlowCandidate) -> FlowDecision {
        // UDP is never bridged. Loopback is named on its own when the
        // address can be read, so a packet to this Mac is not reported as
        // QUIC leaving the machine.
        if candidate.transport == .udp {
            if !candidate.host.isEmpty, isLoopback(candidate.host) { return .leave(.loopback) }
            return .leave(.udp)
        }

        guard !candidate.host.isEmpty else { return .leave(.unreadable) }
        if isLoopback(candidate.host) { return .leave(.loopback) }
        guard interceptedPorts.contains(candidate.port) else { return .leave(.port) }

        return .bridge
    }

    /// Whether `host` is a loopback address or this machine's own name for
    /// itself.
    ///
    /// The network rules the provider installs already exclude loopback, so
    /// the system should never deliver one. This is the same rule again,
    /// because a flow to the bridge that was handed to the bridge is the one
    /// mistake this provider must not be able to make.
    static func isLoopback(_ host: String) -> Bool {
        let bare = host.hasPrefix("[") && host.hasSuffix("]")
            ? String(host.dropFirst().dropLast())
            : host
        let lowered = bare.lowercased()

        if lowered == "localhost" || lowered.hasSuffix(".localhost") { return true }
        if lowered == "::1" || lowered == "0:0:0:0:0:0:0:1" { return true }
        if lowered.hasPrefix("::ffff:127.") { return true }

        let octets = lowered.split(separator: ".", omittingEmptySubsequences: false)
        if octets.count == 4, octets.allSatisfy({ UInt8($0) != nil }) {
            return octets[0] == "127" || lowered == "0.0.0.0"
        }

        return lowered == "::" || lowered == "0:0:0:0:0:0:0:0"
    }
}
