//  Connecting an emulator, and putting it back exactly as it was.
//
//  The cases that matter are the reversible ones: an emulator with no proxy, an
//  emulator that never had the setting, an emulator that already had someone
//  else's proxy — and doing any of it twice.

import Foundation
import Testing

import AndroidTools

@testable import ProxyLabCore

@MainActor
@Suite("The Android Emulator session")
struct AndroidSessionModelTests {
    private func model(
        _ adb: FakeAdb,
        certificatePath: String? = "/home/dev/.proxylab/ca/ca-cert.pem"
    ) -> AndroidSessionModel {
        AndroidSessionModel(
            runner: adb,
            port: 9_090,
            certificatePath: certificatePath,
            certificateExists: { _ in true }
        )
    }

    private func connected(_ adb: FakeAdb) async -> AndroidSessionModel {
        let model = model(adb)
        await model.refresh()
        await model.connect(coreIsRunning: true)

        return model
    }

    // MARK: - Discovery

    @Test("a refresh lists the running emulator and selects it")
    func refreshLists() async {
        let model = model(FakeAdb())

        await model.refresh()

        #expect(model.emulators.map(\.serial) == ["emulator-5554"])
        #expect(model.selected == "emulator-5554")
        #expect(model.note == nil)
    }

    @Test("nothing running says so, and says what to do about it")
    func nothingRunning() async {
        let adb = FakeAdb(present: false)
        let model = model(adb)

        await model.refresh()

        #expect(model.emulators.isEmpty)
        #expect(model.note?.text.contains("No Android Emulator is running") == true)
        #expect(model.statusText == "No emulator running")
        #expect(!model.canConnect)
    }

    // MARK: - Connect

    @Test("connecting with the core stopped changes nothing and says why")
    func coreNotRunning() async {
        let adb = FakeAdb(proxy: .disabled)
        let model = model(adb)
        await model.refresh()

        await model.connect(coreIsRunning: false)

        #expect(model.state == .notConnected)
        #expect(model.note?.text.contains("ProxyLab is not running") == true)
        #expect(adb.proxy == .disabled)
        #expect(!adb.invocations.contains { $0.contains("put") })
    }

    @Test("connecting points the emulator at 10.0.2.2 and the ProxyLab port")
    func connects() async {
        let adb = FakeAdb(proxy: .disabled)

        let model = await connected(adb)

        #expect(model.state == .connected(serial: "emulator-5554"))
        #expect(adb.proxy == .configured("10.0.2.2:9090"))
        #expect(model.statusText.contains("10.0.2.2:9090"))
        #expect(model.note?.isFailure == false)
    }

    @Test("the success note does not promise that every app will trust the CA")
    func doesNotOverpromise() async {
        let model = await connected(FakeAdb())
        let note = try? #require(model.note?.text)

        #expect(note?.contains("HTTP is captured either way") == true)
        #expect(note?.contains("pinned") == true)
    }

    @Test("an emulator that is not ready is refused with its state")
    func unreadyEmulator() async {
        let model = AndroidSessionModel(
            runner: UnreadyAdb(),
            port: 9_090,
            certificatePath: "/ca.pem"
        )
        await model.refresh()

        await model.connect(coreIsRunning: true)

        #expect(model.state == .notConnected)
        #expect(model.note?.text.contains("offline") == true)
        #expect(!model.canConnect)
    }

    // MARK: - Restoration

    @Test("an emulator with no proxy is left with no proxy")
    func restoresDisabled() async {
        let adb = FakeAdb(proxy: .disabled)
        let model = await connected(adb)

        await model.disconnect()

        #expect(adb.proxy == .disabled)
        #expect(model.state == .notConnected)
        #expect(model.note?.text.contains("back to none") == true)
    }

    @Test("an emulator that never had the setting has it removed again")
    func restoresUnset() async {
        let adb = FakeAdb(proxy: .unset)
        let model = await connected(adb)

        await model.disconnect()

        // Deleted, not set to `:0`: the settings database is as it was found.
        #expect(adb.proxy == .unset)
        #expect(adb.invocations.contains {
            $0 == ["-s", "emulator-5554", "shell", "settings", "delete", "global", "http_proxy"]
        })
    }

    @Test("an emulator that already had a proxy gets that proxy back, not nothing")
    func restoresAnotherProxy() async {
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let model = await connected(adb)
        #expect(adb.proxy == .configured("10.0.2.2:9090"))

        await model.disconnect()

        #expect(adb.proxy == .configured("proxy.corp:3128"))
        #expect(model.note?.text.contains("proxy.corp:3128") == true)
    }

    // MARK: - Idempotency

    @Test("connecting twice does not record ProxyLab as what was there before")
    func connectingTwiceKeepsTheOriginal() async {
        // The bug this exists to catch: the second connect saving
        // `10.0.2.2:9090` and a later disconnect "restoring" ProxyLab.
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let model = await connected(adb)

        await model.connect(coreIsRunning: true)
        #expect(model.state == .connected(serial: "emulator-5554"))

        await model.disconnect()

        #expect(adb.proxy == .configured("proxy.corp:3128"))
    }

    @Test("disconnecting twice is one restore and then nothing")
    func disconnectingTwice() async {
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let model = await connected(adb)

        await model.disconnect()
        let after = adb.invocations.count
        await model.disconnect()

        #expect(adb.invocations.count == after)
        #expect(adb.proxy == .configured("proxy.corp:3128"))
    }

    @Test("connecting an emulator already pointed at ProxyLab leaves it clearable")
    func leftoverFromAPreviousRun() async {
        // A crash left ProxyLab's own address behind. Restoring it would not
        // be restoring anything, so it is treated as no proxy.
        let adb = FakeAdb(proxy: .configured("10.0.2.2:9090"))
        let model = await connected(adb)

        await model.disconnect()

        #expect(adb.proxy == .disabled)
    }

    // MARK: - The emulator going away

    @Test("an emulator shut down while connected is reported, and its setting kept")
    func shutDownWhileConnected() async {
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let model = await connected(adb)
        adb.shutDown()

        await model.refresh()

        #expect(model.state == .notConnected)
        #expect(model.emulators.isEmpty)
        let note = try? #require(model.note?.text)
        #expect(note?.contains("no longer running") == true)
        #expect(note?.contains("keeps it across a restart") == true)
    }

    @Test("a restore that cannot reach the emulator keeps the saved setting")
    func restoreFailsCleanly() async {
        let adb = FakeAdb(proxy: .configured("proxy.corp:3128"))
        let model = await connected(adb)
        adb.shutDown()

        await model.disconnect()

        #expect(model.state == .notConnected)
        #expect(model.note?.isFailure == true)
        #expect(model.note?.text.contains("saved setting is kept") == true)
    }

    // MARK: - The CA

    @Test("sending the CA pushes it and says what to tap")
    func deliversCertificate() async {
        let adb = FakeAdb()
        let model = model(adb)
        await model.refresh()

        await model.deliverCertificate()

        #expect(adb.invocations.contains {
            $0 == [
                "-s", "emulator-5554", "push",
                "/home/dev/.proxylab/ca/ca-cert.pem", "/sdcard/Download/proxylab-ca.crt",
            ]
        })
        #expect(model.note?.isFailure == false)
        #expect(model.note?.text.contains("CA certificate") == true)
    }

    // MARK: - No adb

    @Test("no adb on this Mac is said once, and says where to put one")
    func noAdb() async {
        let model = AndroidSessionModel(runner: nil, port: 9_090, certificatePath: "/ca.pem")

        await model.refresh()

        #expect(model.note?.text.contains("ANDROID_HOME") == true)
        #expect(!model.canConnect)
    }
}

/// An adb whose emulator is still booting.
private struct UnreadyAdb: AdbCommandRunner {
    func run(_ arguments: [String]) async throws -> CommandOutcome {
        CommandOutcome(
            status: 0,
            standardOutput: arguments == ["devices", "-l"]
                ? "List of devices attached\nemulator-5554\toffline"
                : "",
            standardError: ""
        )
    }
}
