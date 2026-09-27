//  Reading `adb devices -l`.
//
//  The fixture is real output, captured from this Mac with one emulator
//  running. Parsing is tested against that rather than against whatever
//  happens to be running while the tests do.

import Foundation
import Testing

@testable import AndroidTools

@Suite("Running emulators")
struct EmulatorCatalogTests {
    /// Real `adb devices -l` output.
    private static let realListing = """
    List of devices attached
    emulator-5554          device product:sdk_gphone16k_arm64 model:sdk_gphone16k_arm64 \
    device:emu64a16k transport_id:1

    """

    @Test("the running emulator is read, with its serial, state and model")
    func readsTheEmulator() {
        let emulators = EmulatorCatalog.parse(Self.realListing)

        #expect(emulators.count == 1)
        #expect(emulators[0].serial == "emulator-5554")
        #expect(emulators[0].state == "device")
        #expect(emulators[0].model == "sdk_gphone16k_arm64")
        #expect(emulators[0].isUsable)
    }

    @Test("the header line is not a device")
    func headerIsNotADevice() {
        #expect(EmulatorCatalog.parse("List of devices attached\n").isEmpty)
        #expect(EmulatorCatalog.parse("").isEmpty)
    }

    @Test("daemon chatter before the list is not a device either")
    func daemonChatter() {
        let output = """
        * daemon not running; starting now at tcp:5037
        * daemon started successfully
        List of devices attached
        emulator-5554	device
        """

        #expect(EmulatorCatalog.parse(output).map(\.serial) == ["emulator-5554"])
    }

    @Test("a plain `adb devices` row, which is tab separated, reads the same")
    func tabSeparated() {
        let emulators = EmulatorCatalog.parse("List of devices attached\nemulator-5554\tdevice\n")

        #expect(emulators.map(\.serial) == ["emulator-5554"])
        #expect(emulators[0].state == "device")
        #expect(emulators[0].model == nil)
    }

    @Test("a physical device and a networked one are not emulators")
    func onlyEmulators() {
        // A hardware serial, and the `host:port` a Genymotion or wirelessly
        // connected device gets. `10.0.2.2` means nothing to either, which is
        // why neither is offered here.
        let output = """
        List of devices attached
        emulator-5554	device
        39121FDJH003TG	device
        192.168.1.42:5555	device
        """

        #expect(EmulatorCatalog.parse(output).map(\.serial) == ["emulator-5554"])
    }

    @Test("an emulator that is not ready is listed, and says it is not ready")
    func unreadyEmulator() {
        // Listed rather than hidden: leaving it out would look like it was not
        // running at all, which is a different problem with a different fix.
        let emulators = EmulatorCatalog.parse(
            "List of devices attached\nemulator-5554\toffline\nemulator-5556\tunauthorized\n"
        )

        #expect(emulators.count == 2)
        #expect(emulators.allSatisfy { !$0.isUsable })
        #expect(emulators[0].state == "offline")
    }

    @Test("the order does not depend on what adb printed first")
    func stableOrder() {
        let emulators = EmulatorCatalog.parse(
            "List of devices attached\nemulator-5560\tdevice\nemulator-5554\tdevice\n"
        )

        #expect(emulators.map(\.serial) == ["emulator-5554", "emulator-5560"])
    }

    @Test("the AVD name is read from what `emu avd name` prints")
    func avdName() {
        // `emu` answers, and then says OK on a line of its own.
        #expect(EmulatorCatalog.readAvdName("Pixel_10_Pro_XL\nOK") == "Pixel_10_Pro_XL")
        #expect(EmulatorCatalog.readAvdName("OK") == nil)
        #expect(EmulatorCatalog.readAvdName("") == nil)
    }

    @Test("a listed emulator is named by its AVD, and always by its serial")
    func labels() async throws {
        let adb = FakeAdb()

        let emulators = try await EmulatorCatalog(runner: adb).runningEmulators()

        #expect(emulators.count == 1)
        #expect(emulators[0].avdName == "Pixel_10_Pro_XL")
        #expect(emulators[0].label == "Pixel_10_Pro_XL — emulator-5554")
        #expect(adb.invocations.contains(["devices", "-l"]))
        #expect(adb.invocations.contains(["-s", "emulator-5554", "emu", "avd", "name"]))
    }

    @Test("an emulator adb cannot name is still an emulator")
    func unnamedEmulator() async throws {
        let emulators = try await EmulatorCatalog(runner: FakeAdb(avd: nil)).runningEmulators()

        #expect(emulators[0].avdName == nil)
        #expect(emulators[0].label == "sdk_gphone16k_arm64 — emulator-5554")
    }

    @Test("adb failing to list is reported with adb's own message")
    func listingFails() async {
        let adb = FakeAdb()
        adb.failEverything(message: "adb: no devices/emulators found")

        await #expect(throws: AndroidToolsError.commandFailed(
            command: "devices -l",
            status: 1,
            reason: "adb: no devices/emulators found"
        )) {
            try await EmulatorCatalog(runner: adb).runningEmulators()
        }
    }

    @Test("nothing running is an empty list, not a failure")
    func nothingRunning() async throws {
        let adb = FakeAdb(listing: "List of devices attached\n")

        #expect(try await EmulatorCatalog(runner: adb).runningEmulators().isEmpty)
    }
}
