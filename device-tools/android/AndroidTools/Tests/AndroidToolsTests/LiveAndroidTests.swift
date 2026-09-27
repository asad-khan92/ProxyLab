//  The real `adb`, and whatever emulator is actually running.
//
//  Skipped unless `PROXYLAB_TEST_ANDROID=1`, for the reason the live simulator
//  suite is skipped unless it is asked for: these assertions depend on the
//  machine they run on, and a suite that fails because nobody started an
//  emulator is a suite people learn to ignore.
//
//  Everything here reads. Nothing changes a proxy setting, pushes a
//  certificate, starts an emulator, or stops one — a test run is not the place
//  to reconfigure a developer's device.

import Foundation
import Testing

@testable import AndroidTools

@Suite("Against the real adb")
struct LiveAndroidTests {
    private static var enabled: Bool {
        ProcessInfo.processInfo.environment["PROXYLAB_TEST_ANDROID"] == "1"
    }

    private static func adb() throws -> AdbRunner {
        guard let path = AdbLocation.resolve() else { throw AndroidToolsError.adbNotFound }

        return AdbRunner(adb: path)
    }

    @Test("adb is where this Mac says it is", .enabled(if: enabled))
    func findsAdb() throws {
        let path = try #require(AdbLocation.resolve())

        #expect(path.hasSuffix("/adb"))
        #expect(FileManager.default.isExecutableFile(atPath: path))
    }

    @Test("the real adb lists the running emulators", .enabled(if: enabled))
    func realDiscovery() async throws {
        let emulators = try await EmulatorCatalog(runner: try Self.adb()).runningEmulators()

        // Whatever is running, every row is a row.
        for emulator in emulators {
            #expect(emulator.serial.hasPrefix("emulator-"))
            #expect(!emulator.state.isEmpty)
            #expect(!emulator.label.isEmpty)
        }
    }

    @Test("a running emulator's proxy setting can be read", .enabled(if: enabled))
    func realProxyRead() async throws {
        let emulators = try await EmulatorCatalog(runner: try Self.adb()).runningEmulators()
        guard let emulator = emulators.first(where: \.isUsable) else { return }

        // Read, and only read. What it says depends on the emulator, so what
        // is asserted is that it reads as one of the three states.
        let proxy = try await EmulatorProxyControl(runner: try Self.adb())
            .currentProxy(on: emulator.serial)

        #expect([EmulatorProxy.unset, .disabled].contains(proxy) || proxy.isProxied)
        #expect(proxy.isRestorable, "a real setting should always be one ProxyLab can put back")
    }

    @Test("a serial that is not a device is refused by adb", .enabled(if: enabled))
    func realUnknownDevice() async throws {
        await #expect(throws: AndroidToolsError.self) {
            try await EmulatorProxyControl(runner: try Self.adb())
                .currentProxy(on: "emulator-9999")
        }
    }
}
