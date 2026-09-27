//  Which flows the provider hands to the core, and which it leaves alone.
//
//      new TCP flow
//           │
//           ├─ loopback, or not port 80/443 ───────────▶ not claimed
//           ├─ from a process that is ProxyLab's core ─▶ not claimed
//           ├─ the core was unreachable a moment ago ──▶ not claimed
//           └─ everything else ────────────────────────▶ bridged to the core
//
//  A flow that is not claimed goes out exactly as it would have with no
//  provider installed: macOS carries it, and ProxyLab never sees it.
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
        /// Not a port this provider carries.
        case port
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

    public init(host: String, port: UInt16, source: SourceProcess?) {
        self.host = host
        self.port = port
        self.source = source
    }
}

/// The provider's decisions, and what it has learned while making them.
///
/// Called from whichever queue the system delivers a flow on, so everything
/// it keeps is behind one lock, and every call is short.
public final class FlowPolicy: Sendable {
    /// The ports whose flows are offered to the core.
    public static let interceptedPorts: Set<UInt16> = [80, 443]

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
            case .bridge: state.counters.bridged += 1
            case .leave(.proxyLab): state.counters.proxyLab += 1
            case .leave(.coreUnavailable): state.counters.passedThrough += 1
            case .leave: break
            }

            return final
        }
    }

    /// Records that the core refused `source` as its own.
    public func learnProxyLab(_ source: SourceProcess?) {
        state.withLock { state in
            if let source { state.own.insert(source) }
            state.counters.proxyLab += 1
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
