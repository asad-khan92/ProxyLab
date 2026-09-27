//  Where the proxy can be reached from, as the core reports it.
//
//      {"enabled": true, "address": "192.168.1.42", "port": 9090,
//       "available": [{"interface": "en0", "address": "192.168.1.42"}]}
//
//  The core's `LanStatus`, spelled in Swift. Like every other model here it
//  mirrors the Rust type rather than restating it: `proxy-ipc` decides what a
//  status is, and this decodes it.
//
//  # The port is always there, and the address is not
//
//  `port` is the port the proxy is listening on, on loopback and on the local
//  network alike, so it is present whether or not LAN exposure is on — it is
//  half of what an iPhone's proxy settings need. `address` is the LAN address
//  the proxy is bound to, and is `nil` exactly when it is bound to none.
//
//  # Nothing here is persisted
//
//  The app never writes an address into `UserDefaults` or anywhere else. The
//  addresses are the core's answer about the machine as it is at that moment,
//  and a Mac that has moved between networks has different ones. A pinned
//  domain is a preference; this is not.

import Foundation

/// One address of this Mac a LAN listener could be bound to.
public struct LanAddress: Codable, Sendable, Equatable, Hashable, Identifiable {
    /// The interface it belongs to, as the operating system names it.
    public let interface: String
    /// The address itself.
    public let address: String

    public init(interface: String, address: String) {
        self.interface = interface
        self.address = address
    }

    /// The address, which is what a choice is made by: two interfaces never
    /// hold the same one, and the core already removed duplicates.
    public var id: String { address }

    /// What a picker shows: the interface, then the address.
    public var label: String { "\(interface) — \(address)" }
}

/// Whether the proxy is on the local network, and where it could be.
public struct LanStatusResult: Codable, Sendable, Equatable {
    /// Whether the proxy is listening on a LAN address as well as loopback.
    public let enabled: Bool
    /// The LAN address it is listening on, or `nil` when it is on none.
    public let address: String?
    /// The port the proxy listens on, whether or not LAN exposure is on.
    public let port: UInt16
    /// The addresses this Mac is currently offering.
    public let available: [LanAddress]

    public init(enabled: Bool, address: String?, port: UInt16, available: [LanAddress]) {
        self.enabled = enabled
        self.address = address
        self.port = port
        self.available = available
    }

    /// What an iPhone's Wi-Fi proxy settings need, when there is one to give.
    ///
    /// `nil` rather than a guess when LAN exposure is off: there is no
    /// address to type in, and showing the one that would be used if it were
    /// turned on would be telling a developer to configure a phone that
    /// cannot connect.
    public var endpoint: String? {
        guard enabled, let address else { return nil }

        return "\(address):\(port)"
    }
}
