//  The HTTP and HTTPS proxy settings ProxyLab saves, sets, and puts back.
//
//  A network service stores more than these two protocols — FTP, SOCKS, a
//  PAC file, an exceptions list. None of that is ProxyLab's. What is saved
//  and what is written back are the six values Start changes: whether HTTP
//  is proxied, where, and on which port, and the same three for HTTPS.
//
//  The owned configuration is always the explicit listener,
//  `127.0.0.1:<proxy port>`. The LAN address is a different listener and is
//  never written here. `127.0.0.1:9091` is the transparent bridge and is
//  never a system proxy.

import Foundation

/// HTTP or HTTPS, as one service stores it.
struct ProxyProtocolSetting: Equatable, Sendable, Codable {
    /// Whether that protocol is proxied. Off and absent are the same to a
    /// request, and they are not the same stored value: a restore puts back
    /// the value that was there.
    var enabled: Bool
    /// The host, or `nil` when the key was absent.
    var host: String?
    /// The port, or `nil` when the key was absent.
    var port: Int?

    /// Whether this is the explicit listener ProxyLab sets.
    func matchesOwned(host ownedHost: String, port ownedPort: Int) -> Bool {
        guard enabled, port == ownedPort, let host, !host.isEmpty else { return false }

        return host.compare(ownedHost, options: [.caseInsensitive]) == .orderedSame
    }
}

/// The HTTP and HTTPS proxy of one network service.
struct SystemProxySettings: Equatable, Sendable, Codable {
    var http: ProxyProtocolSetting
    var https: ProxyProtocolSetting

    static let direct = SystemProxySettings(
        http: ProxyProtocolSetting(enabled: false, host: nil, port: nil),
        https: ProxyProtocolSetting(enabled: false, host: nil, port: nil)
    )

    /// Both protocols pointing at the explicit listener.
    static func owned(host: String, port: Int) -> SystemProxySettings {
        let setting = ProxyProtocolSetting(enabled: true, host: host, port: port)
        return SystemProxySettings(http: setting, https: setting)
    }

    /// Whether both protocols are the explicit listener. One of them matching
    /// is not enough: a proxy that sees plaintext and not TLS is not the
    /// configuration ProxyLab set.
    func matchesOwned(host: String, port: Int) -> Bool {
        http.matchesOwned(host: host, port: port) && https.matchesOwned(host: host, port: port)
    }
}

/// One network service, as read from the system.
struct NetworkServiceSnapshot: Equatable, Sendable {
    var id: String
    var name: String
    var interfaceName: String?
    var enabled: Bool
    /// Whether this is the service the system is using for the default route.
    ///
    /// Selected by the service id the dynamic store reports, never by the
    /// service's name. "Wi-Fi" is a name a Mac may have; it is not a rule.
    var isPrimary: Bool
    var settings: SystemProxySettings
}

/// What ProxyLab saved for one service before it changed that service.
struct StoredProxyService: Equatable, Sendable, Codable {
    var id: String
    var name: String
    var previous: SystemProxySettings
}

/// The services ProxyLab has changed, and the endpoint it set.
///
/// Written to disk before the system is changed, so a crash still knows what
/// to put back. The next launch reads it and does not record ProxyLab's own
/// endpoint as "what was there before".
struct SystemProxyRecord: Equatable, Sendable, Codable {
    var version: Int
    var host: String
    var port: Int
    var services: [StoredProxyService]

    var owned: SystemProxySettings {
        .owned(host: host, port: port)
    }

    func stored(_ id: String) -> StoredProxyService? {
        services.first { $0.id == id }
    }
}

/// One write to one service.
struct SystemProxyWrite: Equatable, Sendable {
    var serviceID: String
    var serviceName: String
    var settings: SystemProxySettings
}
