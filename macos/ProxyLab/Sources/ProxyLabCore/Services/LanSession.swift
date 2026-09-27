//  What the iPhone section is allowed to ask the core.
//
//  The three LAN operations the IPC protocol has, and nothing else. It sits
//  beside ``TrafficSession`` and ``RulesSession`` for the same reason they sit
//  beside each other: the screens ask the core for quite different things and
//  none of them needs another's reach — but they are the same protocol on the
//  same socket, and `IPCClient` implements all three, so the app has one
//  connection and not three.
//
//  A protocol for the same reason the others are: turning LAN exposure on and
//  off is testable without a socket, a server, or a network.

import Foundation

/// The core's local-network exposure, as the interface uses it.
public protocol LanSession: Sendable {
    /// Whether the proxy is on the local network, and which of this Mac's
    /// addresses it could use.
    func lanStatus() async throws -> LanStatusResult
    /// Puts the proxy on the local network at `address`, as well as on
    /// loopback.
    func enableLan(address: String) async throws -> LanStatusResult
    /// Takes the proxy off the local network, leaving loopback alone.
    func disableLan() async throws -> LanStatusResult
}

extension IPCClient: LanSession {}
