//  Connecting a simulator, and what the interface is told about it.
//
//  The interesting cases are the honest ones: the core is not running, the
//  certificate is trusted but nothing routes the simulator to ProxyLab, the
//  simulator has been shut down since it was connected. Each of those is a
//  different sentence, and the sentence is the feature.

import Foundation
import SimulatorTools
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("The iOS Simulator session")
struct SimulatorSessionModelTests {
    private static let proxy = ProxyEndpoint(host: "127.0.0.1", port: 9_090)
    private static let toProxyLab = ProxyRoute(http: proxy, https: proxy)
    private static let elsewhere = ProxyRoute(
        http: ProxyEndpoint(host: "127.0.0.1", port: 8_888),
        https: ProxyEndpoint(host: "127.0.0.1", port: 8_888)
    )

    private func model(
        simctl: FakeSimctl,
        route: any ProxyRouteReading = FakeProxyRoute(route: toProxyLab),
        certificateExists: Bool = true,
        certificatePath: String? = "/home/dev/.proxylab/ca/ca-cert.pem"
    ) -> SimulatorSessionModel {
        SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: simctl),
            trust: SimulatorTrustStore(runner: simctl, certificateExists: { _ in
                certificateExists
            }),
            routes: route,
            endpoint: Self.proxy,
            certificatePath: certificatePath
        )
    }

    // MARK: - Discovery

    @Test("a refresh lists what is booted and selects the first of them")
    func refreshLists() async {
        let model = model(simctl: .listing([("A", "iPhone 17"), ("B", "iPad Pro")]))

        await model.refresh()

        #expect(model.simulators.map(\.name) == ["iPad Pro", "iPhone 17"])
        #expect(model.selected == "A" || model.selected == "B")
        #expect(model.selectedSimulator != nil)
        #expect(model.note == nil)
    }

    @Test("nothing booted says so, and says what to do about it")
    func nothingBooted() async {
        let model = model(simctl: .listing([]))

        await model.refresh()

        #expect(model.simulators.isEmpty)
        #expect(model.note?.isFailure == true)
        #expect(model.note?.text.contains("No iOS Simulator is booted") == true)
        #expect(model.statusText == "No simulator booted")
        #expect(!model.canConnect)
    }

    @Test("a refresh keeps a selection that is still booted")
    func keepsSelection() async {
        let model = model(simctl: .listing([("A", "iPhone 17"), ("B", "iPad Pro")]))
        await model.refresh()
        model.selected = "B"

        await model.refresh()

        #expect(model.selected == "B")
    }

    // MARK: - Connect

    @Test("connecting with the core stopped runs nothing and says why")
    func coreNotRunning() async {
        let simctl = FakeSimctl.listing([("A", "iPhone 17")])
        let model = model(simctl: simctl)
        await model.refresh()

        await model.connect(coreIsRunning: false)

        #expect(model.state == .notConnected)
        #expect(model.note?.isFailure == true)
        #expect(model.note?.text.contains("ProxyLab is not running") == true)
        // The listing, and nothing after it: no certificate was installed.
        #expect(simctl.invocations.allSatisfy { $0.first == "list" })
    }

    @Test("a simulator that trusts the CA and routes to ProxyLab is connected")
    func connects() async {
        let simctl = FakeSimctl.listing([("A", "iPhone 17")])
        let model = model(simctl: simctl)
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .connected(udid: "A"))
        #expect(model.note?.isFailure == false)
        #expect(model.statusText == "Connected — iPhone 17 via 127.0.0.1:9090")
        #expect(simctl.invocations.contains([
            "keychain", "A", "add-root-cert", "/home/dev/.proxylab/ca/ca-cert.pem",
        ]))
    }

    @Test("connecting twice runs the same command again and stays connected")
    func connectingIsIdempotent() async {
        let simctl = FakeSimctl.listing([("A", "iPhone 17")])
        let model = model(simctl: simctl)
        await model.refresh()

        await model.connect(coreIsRunning: true)
        await model.connect(coreIsRunning: true)

        #expect(model.state == .connected(udid: "A"))
        let installs = simctl.invocations.filter { $0.first == "keychain" }
        #expect(installs.count == 2)
        #expect(installs[0] == installs[1])
    }

    @Test("a route that is not ProxyLab's leaves the CA trusted and says what is missing")
    func routeDoesNotReachProxyLab() async {
        let simctl = FakeSimctl.listing([("A", "iPhone 17")])
        let model = model(simctl: simctl, route: FakeProxyRoute(route: Self.elsewhere))
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .certificateTrusted(udid: "A"))
        #expect(model.note?.isFailure == true)
        // The certificate went in all the same: it is the half that worked.
        #expect(simctl.invocations.contains(where: { $0.first == "keychain" }))

        let note = try? #require(model.note?.text)
        #expect(note?.contains("no proxy settings of its own") == true)
        #expect(note?.contains("127.0.0.1:8888") == true)
        #expect(note?.contains("127.0.0.1:9090") == true)
        #expect(note?.contains("ProxyLab does not change that setting for you") == true)
    }

    @Test("a proxy configuration that cannot be read is reported, not guessed at")
    func unreadableRoute() async {
        let model = model(simctl: .listing([("A", "iPhone 17")]), route: UnreadableProxyRoute())
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .certificateTrusted(udid: "A"))
        #expect(model.note?.text.contains("could not be read") == true)
    }

    @Test("a CA that has not been generated yet names the path it is not at")
    func missingCertificate() async {
        let simctl = FakeSimctl.listing([("A", "iPhone 17")])
        let model = model(simctl: simctl, certificateExists: false)
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .notConnected)
        #expect(model.note?.text.contains("/home/dev/.proxylab/ca/ca-cert.pem") == true)
        #expect(!simctl.invocations.contains(where: { $0.first == "keychain" }))
    }

    @Test("simctl's own failure is what the interface shows")
    func simctlFailure() async {
        let simctl = FakeSimctl(status: 148, errors: "Invalid device: A")
        let model = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([("A", "iPhone 17")])),
            trust: SimulatorTrustStore(runner: simctl, certificateExists: { _ in true }),
            routes: FakeProxyRoute(route: Self.toProxyLab),
            endpoint: Self.proxy,
            certificatePath: "/ca.pem"
        )
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .notConnected)
        #expect(model.note?.text.contains("Invalid device: A") == true)
    }

    // MARK: - Disconnect

    @Test("disconnecting says that nothing was changed and nothing is restored")
    func disconnects() async {
        let model = model(simctl: .listing([("A", "iPhone 17")]))
        await model.refresh()
        await model.connect(coreIsRunning: true)

        model.disconnect()

        #expect(model.state == .notConnected)
        #expect(model.note?.isFailure == false)

        let note = try? #require(model.note?.text)
        #expect(note?.contains("changed no proxy setting") == true)
        #expect(note?.contains("nothing to restore") == true)
        // The one thing it does not undo, said rather than left to be found.
        #expect(note?.contains("trust of the ProxyLab CA is left in place") == true)
    }

    @Test("disconnecting and connecting again is allowed as many times as wanted")
    func reconnects() async {
        let model = model(simctl: .listing([("A", "iPhone 17")]))
        await model.refresh()

        await model.connect(coreIsRunning: true)
        model.disconnect()
        await model.connect(coreIsRunning: true)

        #expect(model.state == .connected(udid: "A"))
        #expect(model.canDisconnect)
    }

    @Test("a simulator that has been shut down is no longer connected")
    func connectedSimulatorShutDown() async {
        let model = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([("A", "iPhone 17")])),
            trust: SimulatorTrustStore(
                runner: FakeSimctl.listing([("A", "iPhone 17")]),
                certificateExists: { _ in true }
            ),
            routes: FakeProxyRoute(route: Self.toProxyLab),
            endpoint: Self.proxy,
            certificatePath: "/ca.pem"
        )
        await model.refresh()
        await model.connect(coreIsRunning: true)
        #expect(model.state == .connected(udid: "A"))

        // The same model, now asking a `simctl` that reports nothing booted.
        let after = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([])),
            trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in true }),
            routes: FakeProxyRoute(route: Self.toProxyLab),
            endpoint: Self.proxy,
            certificatePath: "/ca.pem"
        )
        await after.refresh()

        #expect(after.state == .notConnected)
        #expect(!after.canDisconnect)
    }

    // MARK: - The core's locations

    @Test("the CA certificate is where the Rust core puts it")
    func caCertificateMirrorsTheCore() {
        #expect(
            CoreLocations.caCertificate(environment: ["HOME": "/home/dev"])
                == "/home/dev/.proxylab/ca/ca-cert.pem"
        )
        #expect(
            CoreLocations.caCertificate(environment: [
                "HOME": "/home/dev",
                "PROXYLAB_CA_DIR": "/elsewhere/ca",
            ]) == "/elsewhere/ca/ca-cert.pem"
        )
        // The same fallback order the core has, and the same refusal when
        // there is no home directory at all.
        #expect(
            CoreLocations.caCertificate(environment: ["USERPROFILE": "/home/dev"])
                == "/home/dev/.proxylab/ca/ca-cert.pem"
        )
        #expect(CoreLocations.caCertificate(environment: [:]) == nil)
        #expect(CoreLocations.caCertificate(environment: ["HOME": ""]) == nil)
    }

    @Test("the proxy endpoint is the core's DEFAULT_LISTEN_ADDR")
    func proxyEndpointMirrorsTheCore() {
        #expect(CoreLocations.proxyEndpoint.host == "127.0.0.1")
        #expect(CoreLocations.proxyEndpoint.port == 9_090)
        #expect(CoreLocations.proxyEndpoint.description == "127.0.0.1:9090")
    }
}
