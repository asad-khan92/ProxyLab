//  What to do with the LAN listener when the network changes.
//
//  The explicit listener stays on 127.0.0.1:9090 and the bridge stays on
//  127.0.0.1:9091. Neither moves because a Wi-Fi address moved. This decides
//  only the optional LAN listener: keep it when its address is still here,
//  move it when that address is gone and another usable one is, close it
//  when nothing usable is left, and open it again when an address comes back.
//
//  "Usable" is the core's list. The choice among those, when the bound
//  address has gone, prefers the interface the default route is using.

import Foundation

enum LanMigrationAction: Equatable, Sendable {
    /// The listener is where it should be, or local network access is off.
    case none
    /// Bind this address. The core closes the previous LAN listener first.
    case bind(String)
    /// Close the LAN listener. Local capture on 127.0.0.1 stays up.
    case suspend
    /// Local network access was requested and there is no address to bind.
    case wait
}

enum LanMigration {
    static func action(
        wantsLan: Bool,
        enabled: Bool,
        boundAddress: String?,
        available: [LanAddress],
        primaryInterface: String?
    ) -> LanMigrationAction {
        guard wantsLan else { return .none }

        if enabled, let boundAddress, available.contains(where: { $0.address == boundAddress }) {
            return .none
        }

        guard let address = replacement(available: available, primaryInterface: primaryInterface) else {
            return enabled ? .suspend : .wait
        }
        return .bind(address)
    }

    /// An address from `available`. The primary interface's address when it
    /// has one, otherwise the first address the core offered.
    static func replacement(available: [LanAddress], primaryInterface: String?) -> String? {
        if let primaryInterface,
           let match = available.first(where: { $0.interface == primaryInterface })
        {
            return match.address
        }
        return available.first?.address
    }
}
