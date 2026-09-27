//  The proxy setting: reading it, changing it, and putting it back exactly.
//
//  The values here are what a real emulator answered: `:0` for an emulator
//  with no proxy, `null` after `settings delete`, and `10.0.2.2:9090` read
//  straight back after `settings put`.

import Foundation
import Testing

@testable import AndroidTools

@Suite("The emulator's proxy setting")
struct EmulatorProxyTests {
    private let proxyLab = EmulatorProxyAddress.onHostLoopback(port: 9_090)
    private let serial = "emulator-5554"

    // MARK: - Reading

    @Test("the three states adb can print are three different states")
    func readsThreeStates() {
        #expect(EmulatorProxy.read("null") == .unset)
        #expect(EmulatorProxy.read("") == .unset)
        #expect(EmulatorProxy.read(":0") == .disabled)
        #expect(EmulatorProxy.read("10.0.2.2:9090") == .configured("10.0.2.2:9090"))
        // adb's output arrives with a newline on it.
        #expect(EmulatorProxy.read("  10.0.2.2:9090 \n") == .configured("10.0.2.2:9090"))
    }

    @Test("an absent setting and a disabled one are restored differently")
    func unsetAndDisabledAreNotTheSame() {
        // Putting `:0` onto an emulator whose key was absent would leave a row
        // that was not there before.
        #expect(EmulatorProxy.unset.restoreArguments.contains("delete"))
        #expect(EmulatorProxy.disabled.restoreArguments.contains("put"))
        #expect(EmulatorProxy.disabled.restoreArguments.last == ":0")
        #expect(EmulatorProxy.unset != EmulatorProxy.disabled)
    }

    @Test("a proxy value is restored as the device spelled it")
    func restoresExactly() {
        #expect(
            EmulatorProxy.configured("proxy.corp:3128").restoreArguments
                == ["shell", "settings", "put", "global", "http_proxy", "proxy.corp:3128"]
        )
    }

    @Test("a value that could not go back through a device shell is refused")
    func refusesUnsafeValues() {
        // adb hands what follows `shell` to a shell on the device, so a value
        // that is not one word is never sent back.
        #expect(EmulatorProxy.configured("10.0.2.2:9090").isRestorable)
        #expect(EmulatorProxy.configured("proxy.corp.example:3128").isRestorable)
        #expect(!EmulatorProxy.configured("a b").isRestorable)
        #expect(!EmulatorProxy.configured("x; rm -rf /").isRestorable)
        #expect(!EmulatorProxy.configured("$(whoami)").isRestorable)
        #expect(EmulatorProxy.unset.isRestorable)
        #expect(EmulatorProxy.disabled.isRestorable)
    }

    // MARK: - The commands

    @Test("reading the proxy is one addressed adb command")
    func readCommand() async throws {
        let adb = FakeAdb(proxy: .disabled)

        let proxy = try await EmulatorProxyControl(runner: adb).currentProxy(on: serial)

        #expect(proxy == .disabled)
        #expect(adb.invocations == [
            ["-s", "emulator-5554", "shell", "settings", "get", "global", "http_proxy"],
        ])
    }

    @Test("connecting sets 10.0.2.2 and the ProxyLab port, then reads it back")
    func connectCommand() async throws {
        let adb = FakeAdb(proxy: .disabled)

        try await EmulatorProxyControl(runner: adb).connect(serial, to: proxyLab)

        #expect(adb.proxy == .configured("10.0.2.2:9090"))
        #expect(adb.invocations.first == [
            "-s", "emulator-5554", "shell", "settings", "put", "global", "http_proxy",
            "10.0.2.2:9090",
        ])
        // Verified, not assumed: the read-back is part of connecting.
        #expect(adb.invocations.last?.contains("get") == true)
    }

    @Test("10.0.2.2 is the standard emulator's alias for the Mac's loopback")
    func theAddress() {
        #expect(EmulatorProxyAddress.hostLoopbackAlias == "10.0.2.2")
        #expect(proxyLab.description == "10.0.2.2:9090")
    }

    // MARK: - Saving and restoring

    @Test("an emulator with no proxy is left with no proxy")
    func roundTripFromDisabled() async throws {
        let adb = FakeAdb(proxy: .disabled)
        let control = EmulatorProxyControl(runner: adb)

        let before = try await control.currentProxy(on: serial)
        try await control.connect(serial, to: proxyLab)
        try await control.restore(before, on: serial)

        #expect(adb.proxy == .disabled)
    }

    @Test("an emulator that never had the setting has it removed again")
    func roundTripFromUnset() async throws {
        let adb = FakeAdb(proxy: .unset)
        let control = EmulatorProxyControl(runner: adb)

        let before = try await control.currentProxy(on: serial)
        #expect(before == .unset)
        try await control.connect(serial, to: proxyLab)
        try await control.restore(before, on: serial)

        // Deleted, not set to `:0`: the settings database is as it was found.
        #expect(adb.proxy == .unset)
        #expect(adb.invocations.contains([
            "-s", "emulator-5554", "shell", "settings", "delete", "global", "http_proxy",
        ]))
    }

    @Test("an emulator that already had a proxy gets that proxy back, not nothing")
    func roundTripFromAnotherProxy() async throws {
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let control = EmulatorProxyControl(runner: adb)

        let before = try await control.currentProxy(on: serial)
        try await control.connect(serial, to: proxyLab)
        #expect(adb.proxy == .configured("10.0.2.2:9090"))

        try await control.restore(before, on: serial)

        #expect(adb.proxy == .configured("proxy.corp:3128"))
    }

    @Test("a setting that does not read back the way it was written is a failure")
    func verificationCatchesAWriteThatDidNotTake() async {
        // A device that accepts the write and keeps its old value: `settings
        // put` exits zero, and the proxy is not what anyone was told it is.
        let adb = StubbornAdb()

        await #expect(throws: AndroidToolsError.proxyNotApplied(
            expected: "10.0.2.2:9090",
            actual: "none"
        )) {
            try await EmulatorProxyControl(runner: adb).connect(serial, to: proxyLab)
        }
    }

    @Test("an emulator that has gone is reported as adb reports it")
    func emulatorGone() async {
        let adb = FakeAdb(proxy: .disabled)
        adb.shutDown()

        await #expect(throws: AndroidToolsError.self) {
            try await EmulatorProxyControl(runner: adb).currentProxy(on: serial)
        }
    }
}

/// An adb whose device takes every write and changes nothing.
private struct StubbornAdb: AdbCommandRunner {
    func run(_ arguments: [String]) async throws -> CommandOutcome {
        CommandOutcome(
            status: 0,
            standardOutput: arguments.contains("get") ? ":0" : "",
            standardError: ""
        )
    }
}
