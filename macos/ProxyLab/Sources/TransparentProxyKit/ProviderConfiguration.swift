//  What the app tells the provider when it turns it on.
//
//  The provider runs in a process of its own, as root, sandboxed, and cannot
//  read the app's preferences or the core's files. The one thing it needs —
//  where the core's bridge is — travels in the configuration the app saves
//  with `NETransparentProxyManager`, as `providerConfiguration`. That
//  dictionary is the whole of what the app hands it; nothing else is shared.

import Foundation

/// Where the provider hands its flows.
public struct ProviderConfiguration: Equatable, Sendable {
    /// The bridge is on loopback, always: the core binds it nowhere else.
    public static let bridgeHost = "127.0.0.1"

    /// The core's `DEFAULT_TRANSPARENT_ADDR` port, mirrored in
    /// `CoreLocations` for the app.
    public static let defaultBridgePort: UInt16 = 9_091

    public let bridgePort: UInt16

    public init(bridgePort: UInt16 = Self.defaultBridgePort) {
        self.bridgePort = bridgePort
    }

    private static let bridgePortKey = "bridgePort"

    /// The form `providerConfiguration` holds.
    public var dictionary: [String: Any] {
        [Self.bridgePortKey: Int(bridgePort)]
    }

    /// Reads a configuration back, or the default when there is none.
    ///
    /// A dictionary that names a port that is not one is `nil` rather than
    /// the default: a provider pointed at a port nobody meant is a provider
    /// handing traffic to whatever is listening there.
    public init?(dictionary: [String: Any]?) {
        guard let dictionary else {
            self.init()
            return
        }
        guard let value = dictionary[Self.bridgePortKey] else {
            self.init()
            return
        }
        guard let port = value as? Int, (1...Int(UInt16.max)).contains(port) else {
            return nil
        }

        self.init(bridgePort: UInt16(port))
    }
}
