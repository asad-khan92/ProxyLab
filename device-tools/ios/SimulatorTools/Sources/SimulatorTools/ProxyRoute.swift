//  What a booted simulator will connect through, and whether that is ProxyLab.
//
//  # The simulator has no proxy settings of its own
//
//  This is the fact the whole of this file exists to state. `simctl` has no
//  proxy command — `simctl help` lists none, and the binary contains no proxy
//  strings at all — and CoreSimulator exposes no per-device network
//  configuration. A booted simulator asked `CFNetworkCopySystemProxySettings()`
//  answers this Mac's settings verbatim, down to the host's own `__SCOPED__`
//  interface names. The simulator does not read a proxy configuration; it
//  reads *ours*.
//
//  So there is nothing per-simulator to set, nothing to save before setting
//  it, and nothing to restore afterwards. What there is, is the Mac's own
//  configuration — and this file reads it and never writes it. ProxyLab does
//  not change this Mac's proxy settings; it reports what they are and what
//  they would need to be.
//
//      SCDynamicStoreCopyProxies  ──▶  HTTP / HTTPS host and port
//                                          │
//                                          ▼
//                          does it point at 127.0.0.1:<proxy port>?
//
//  CFNetwork honours no proxy environment variable either, so launching an app
//  with `SIMCTL_CHILD_http_proxy` is not a second way round this.

import Foundation
import SystemConfiguration

/// A host and a port to proxy through.
public struct ProxyEndpoint: Equatable, Sendable, CustomStringConvertible {
    public let host: String
    public let port: Int

    public init(host: String, port: Int) {
        self.host = host
        self.port = port
    }

    public var description: String { "\(host):\(port)" }
}

/// The HTTP and HTTPS proxies in force, which is what the simulator will use.
///
/// `nil` for either means that protocol is not proxied — the setting is
/// absent, or present and switched off, which are the same thing to a request.
public struct ProxyRoute: Equatable, Sendable {
    public let http: ProxyEndpoint?
    public let https: ProxyEndpoint?

    public init(http: ProxyEndpoint?, https: ProxyEndpoint?) {
        self.http = http
        self.https = https
    }

    /// Nothing is proxied.
    public static let direct = ProxyRoute(http: nil, https: nil)

    /// Whether `endpoint` is what both protocols go through.
    ///
    /// Both, because a debugging proxy that sees plaintext and not TLS — or
    /// the other way about — looks broken rather than half-configured.
    public func reaches(_ endpoint: ProxyEndpoint) -> Bool {
        http == endpoint && https == endpoint
    }

    /// What the route is, written out for a person.
    public var summary: String {
        func spell(_ label: String, _ endpoint: ProxyEndpoint?) -> String {
            guard let endpoint else { return "\(label) direct" }

            return "\(label) \(endpoint)"
        }

        return "\(spell("HTTP", http)), \(spell("HTTPS", https))"
    }
}

/// Reads the proxy configuration a booted simulator inherits.
///
/// A protocol so the decision that depends on it is testable without changing
/// anything about the machine the tests run on — and so that the only type
/// that touches SystemConfiguration is the one implementation below.
public protocol ProxyRouteReading: Sendable {
    /// The route in force right now.
    func currentRoute() throws -> ProxyRoute
}

/// The real one: this Mac's proxy configuration, read and never written.
///
/// `SCDynamicStoreCopyProxies` is the read half of SystemConfiguration. There
/// is a write half; it is deliberately not used, not imported through any
/// other type here, and not reachable from anything ProxyLab's interface can
/// press.
public struct SystemProxyRoute: ProxyRouteReading {
    public init() {}

    public func currentRoute() throws -> ProxyRoute {
        guard let proxies = SCDynamicStoreCopyProxies(nil) as? [String: Any] else {
            throw SimulatorToolsError.proxyRouteUnreadable
        }

        return Self.route(from: proxies)
    }

    /// Reads a proxy dictionary into a route.
    ///
    /// Separated from the call that produces the dictionary so the reading is
    /// tested against known dictionaries rather than against whatever the
    /// developer's Mac is configured for.
    static func route(from proxies: [String: Any]) -> ProxyRoute {
        ProxyRoute(
            http: endpoint(
                in: proxies,
                enable: kSCPropNetProxiesHTTPEnable,
                host: kSCPropNetProxiesHTTPProxy,
                port: kSCPropNetProxiesHTTPPort
            ),
            https: endpoint(
                in: proxies,
                enable: kSCPropNetProxiesHTTPSEnable,
                host: kSCPropNetProxiesHTTPSProxy,
                port: kSCPropNetProxiesHTTPSPort
            )
        )
    }

    private static func endpoint(
        in proxies: [String: Any],
        enable: CFString,
        host: CFString,
        port: CFString
    ) -> ProxyEndpoint? {
        guard let enabled = proxies[enable as String] as? Int, enabled != 0 else { return nil }
        guard let host = proxies[host as String] as? String, !host.isEmpty else { return nil }
        guard let port = proxies[port as String] as? Int, port > 0 else { return nil }

        return ProxyEndpoint(host: host, port: port)
    }
}
