//  The real `xcrun simctl`, and this Mac's real proxy configuration.
//
//  Skipped unless `PROXYLAB_TEST_SIMULATOR=1`, for the reason the live core
//  suite in the macOS package is skipped unless a socket is named: these
//  assertions depend on the machine they run on, and a suite that fails
//  because nobody booted a simulator is a suite people learn to ignore.
//
//  Everything here reads. Nothing boots, erases, or shuts down a simulator,
//  and nothing installs a certificate — trusting a CA changes a developer's
//  simulator, and a test run is not the place to decide that for them.

import Foundation
import Testing

@testable import SimulatorTools

@Suite("Against the real simulator tooling")
struct LiveSimulatorTests {
    /// Whether the developer asked for these.
    private static var enabled: Bool {
        ProcessInfo.processInfo.environment["PROXYLAB_TEST_SIMULATOR"] == "1"
    }

    @Test("the real simctl answers the booted device list", .enabled(if: enabled))
    func realDiscovery() async throws {
        let simulators = try await SimulatorCatalog().bootedSimulators()

        // Whatever is booted, every row is a row: a UDID that is a UDID, a
        // name, and an iOS runtime.
        for simulator in simulators {
            #expect(simulator.udid.count == 36)
            #expect(!simulator.name.isEmpty)
            #expect(simulator.runtime.hasPrefix("iOS"))
        }
    }

    @Test("this Mac's proxy configuration can be read", .enabled(if: enabled))
    func realRoute() throws {
        // Read, and only read. What it says depends on the machine, so what is
        // asserted is that it says something coherent.
        let route = try SystemProxyRoute().currentRoute()

        #expect(!route.summary.isEmpty)
        if let http = route.http {
            #expect(!http.host.isEmpty)
            #expect(http.port > 0)
        }
    }

    @Test("a UDID that is not a device is refused by simctl", .enabled(if: enabled))
    func realInvalidDevice() async {
        let store = SimulatorTrustStore(certificateExists: { _ in true })

        // The path is never read: `simctl` rejects the device first, which is
        // the failure this asserts on.
        await #expect(throws: SimulatorToolsError.self) {
            try await store.trustRootCertificate(
                at: "/dev/null",
                on: "00000000-0000-0000-0000-000000000000"
            )
        }
    }
}
