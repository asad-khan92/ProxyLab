//  What a simulator will connect through.
//
//  The dictionaries here are the shape `SCDynamicStoreCopyProxies` actually
//  answers with, and the first one is this Mac's own configuration as it was
//  when a booted simulator was asked `CFNetworkCopySystemProxySettings()` — it
//  answered exactly this, which is the fact the whole feature rests on.

import Foundation
import SystemConfiguration
import Testing

@testable import SimulatorTools

@Suite("The route a simulator takes")
struct ProxyRouteTests {
    private let proxyLab = ProxyEndpoint(host: "127.0.0.1", port: 9_090)

    private func settings(
        httpEnabled: Int? = nil,
        httpHost: String? = nil,
        httpPort: Int? = nil,
        httpsEnabled: Int? = nil,
        httpsHost: String? = nil,
        httpsPort: Int? = nil
    ) -> [String: Any] {
        var settings: [String: Any] = [:]
        settings[kSCPropNetProxiesHTTPEnable as String] = httpEnabled
        settings[kSCPropNetProxiesHTTPProxy as String] = httpHost
        settings[kSCPropNetProxiesHTTPPort as String] = httpPort
        settings[kSCPropNetProxiesHTTPSEnable as String] = httpsEnabled
        settings[kSCPropNetProxiesHTTPSProxy as String] = httpsHost
        settings[kSCPropNetProxiesHTTPSPort as String] = httpsPort

        return settings
    }

    @Test("a Mac proxied to ProxyLab is a simulator routed to ProxyLab")
    func readsBothProtocols() {
        let route = SystemProxyRoute.route(from: settings(
            httpEnabled: 1, httpHost: "127.0.0.1", httpPort: 9_090,
            httpsEnabled: 1, httpsHost: "127.0.0.1", httpsPort: 9_090
        ))

        #expect(route.http == proxyLab)
        #expect(route.https == proxyLab)
        #expect(route.reaches(proxyLab))
    }

    @Test("a proxy that is configured but switched off is not a proxy")
    func disabledIsDirect() {
        let route = SystemProxyRoute.route(from: settings(
            httpEnabled: 0, httpHost: "127.0.0.1", httpPort: 9_090,
            httpsEnabled: 0, httpsHost: "127.0.0.1", httpsPort: 9_090
        ))

        #expect(route == .direct)
        #expect(!route.reaches(proxyLab))
    }

    @Test("nothing configured at all is direct")
    func nothingConfigured() {
        #expect(SystemProxyRoute.route(from: [:]) == .direct)
    }

    @Test("half a route does not reach the proxy")
    func httpsOnlyDoesNotReach() {
        // A debugging proxy that sees plaintext and not TLS looks broken
        // rather than half-configured, so both have to point at it.
        let route = SystemProxyRoute.route(from: settings(
            httpsEnabled: 1, httpsHost: "127.0.0.1", httpsPort: 9_090
        ))

        #expect(route.http == nil)
        #expect(!route.reaches(proxyLab))
    }

    @Test("another proxy on another port does not reach this one")
    func anotherProxy() {
        let route = SystemProxyRoute.route(from: settings(
            httpEnabled: 1, httpHost: "127.0.0.1", httpPort: 8_888,
            httpsEnabled: 1, httpsHost: "127.0.0.1", httpsPort: 8_888
        ))

        #expect(!route.reaches(proxyLab))
        #expect(route.summary == "HTTP 127.0.0.1:8888, HTTPS 127.0.0.1:8888")
    }

    @Test("a route with no port, or a port of zero, is not a route")
    func nonsensePorts() {
        #expect(
            SystemProxyRoute.route(from: settings(httpEnabled: 1, httpHost: "127.0.0.1")).http
                == nil
        )
        #expect(
            SystemProxyRoute.route(
                from: settings(httpEnabled: 1, httpHost: "127.0.0.1", httpPort: 0)
            ).http == nil
        )
        #expect(
            SystemProxyRoute.route(
                from: settings(httpEnabled: 1, httpHost: "", httpPort: 9_090)
            ).http == nil
        )
    }

    @Test("a direct route is written out as direct")
    func directSummary() {
        #expect(ProxyRoute.direct.summary == "HTTP direct, HTTPS direct")
    }
}
