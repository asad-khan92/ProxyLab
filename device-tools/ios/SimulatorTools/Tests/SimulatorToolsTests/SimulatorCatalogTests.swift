//  Reading `simctl list devices booted -j`.
//
//  The fixture is real output, captured from `xcrun simctl list devices booted
//  -j` on a Mac with one booted simulator and several installed runtimes with
//  nothing booted on them. Parsing is tested against that rather than against
//  whatever happens to be booted while the tests run, which is the only way
//  these assertions mean the same thing twice.

import Foundation
import Testing

@testable import SimulatorTools

@Suite("Booted simulators")
struct SimulatorCatalogTests {
    /// Real `simctl list devices booted -j` output, with the runtimes that had
    /// nothing booted left in — because that is what it actually prints.
    private static let realListing = Data(#"""
    {
      "devices" : {
        "com.apple.CoreSimulator.SimRuntime.iOS-26-5" : [
        ],
        "com.apple.CoreSimulator.SimRuntime.iOS-18-5" : [
        ],
        "com.apple.CoreSimulator.SimRuntime.iOS-26-1" : [
          {
            "dataPath" : "\/Users\/dev\/Library\/Developer\/CoreSimulator\/Devices\/333035AE\/data",
            "dataPathSize" : 12426129408,
            "udid" : "333035AE-FED9-4B07-9787-2B91E3C8FCEA",
            "isAvailable" : true,
            "deviceTypeIdentifier" : "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro-Max",
            "state" : "Booted",
            "name" : "iPhone 17 Pro Max"
          }
        ]
      }
    }
    """#.utf8)

    @Test("the booted device is read, with its name, UDID and runtime")
    func readsTheBootedDevice() throws {
        let simulators = try SimulatorCatalog.parse(Self.realListing)

        #expect(simulators.count == 1)
        #expect(simulators[0].udid == "333035AE-FED9-4B07-9787-2B91E3C8FCEA")
        #expect(simulators[0].name == "iPhone 17 Pro Max")
        #expect(simulators[0].runtime == "iOS 26.1")
        #expect(simulators[0].label == "iPhone 17 Pro Max — iOS 26.1")
    }

    @Test("a runtime with nothing booted contributes nothing")
    func emptyRuntimesAreNotDevices() throws {
        let simulators = try SimulatorCatalog.parse(Self.realListing)

        #expect(simulators.allSatisfy { $0.runtime == "iOS 26.1" })
    }

    @Test("a device that is not booted is left out")
    func onlyBootedDevices() throws {
        let listing = Data(#"""
        {"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-1":[
          {"udid":"A","name":"Booted one","state":"Booted"},
          {"udid":"B","name":"Shutting down","state":"Shutdown"}
        ]}}
        """#.utf8)

        let simulators = try SimulatorCatalog.parse(listing)

        #expect(simulators.map(\.udid) == ["A"])
    }

    @Test("a booted watchOS or tvOS simulator is not an iOS Simulator")
    func onlyIOSRuntimes() throws {
        let listing = Data(#"""
        {"devices":{
          "com.apple.CoreSimulator.SimRuntime.watchOS-11-0":[
            {"udid":"W","name":"Apple Watch","state":"Booted"}],
          "com.apple.CoreSimulator.SimRuntime.tvOS-18-0":[
            {"udid":"T","name":"Apple TV","state":"Booted"}],
          "com.apple.CoreSimulator.SimRuntime.iOS-18-5":[
            {"udid":"I","name":"iPhone 16","state":"Booted"}]
        }}
        """#.utf8)

        let simulators = try SimulatorCatalog.parse(listing)

        #expect(simulators.map(\.udid) == ["I"])
    }

    @Test("the order does not depend on the order a dictionary came out in")
    func stableOrdering() throws {
        let listing = Data(#"""
        {"devices":{
          "com.apple.CoreSimulator.SimRuntime.iOS-26-1":[
            {"udid":"C","name":"iPhone 17","state":"Booted"},
            {"udid":"A","name":"iPad Pro","state":"Booted"}],
          "com.apple.CoreSimulator.SimRuntime.iOS-18-5":[
            {"udid":"B","name":"iPhone 17","state":"Booted"}]
        }}
        """#.utf8)

        let simulators = try SimulatorCatalog.parse(listing)

        // Name, then runtime, then UDID: the same two simulators come back in
        // the same order every time.
        #expect(simulators.map(\.udid) == ["A", "B", "C"])
    }

    @Test("nothing booted is an empty list, not a failure")
    func nothingBooted() throws {
        #expect(try SimulatorCatalog.parse(Data(#"{"devices":{}}"#.utf8)).isEmpty)
    }

    @Test("output that is not the device list is reported as unreadable")
    func unreadableOutput() {
        #expect(throws: SimulatorToolsError.deviceListUnreadable) {
            try SimulatorCatalog.parse(Data("not json".utf8))
        }
    }

    @Test("a runtime identifier becomes the name Xcode writes")
    func runtimeNames() {
        #expect(
            SimulatorCatalog.runtimeName(from: "com.apple.CoreSimulator.SimRuntime.iOS-26-1")
                == "iOS 26.1"
        )
        #expect(
            SimulatorCatalog.runtimeName(from: "com.apple.CoreSimulator.SimRuntime.iOS-18-3-1")
                == "iOS 18.3.1"
        )
        #expect(
            SimulatorCatalog.runtimeName(from: "com.apple.CoreSimulator.SimRuntime.watchOS-11-0")
                == nil
        )
    }

    @Test("a failing simctl is reported with its own message")
    func failingCommand() async {
        let catalog = SimulatorCatalog(
            runner: FakeRunner(failingWith: 148, saying: "Invalid device")
        )

        await #expect(throws: SimulatorToolsError.commandFailed(
            command: "list devices booted",
            status: 148,
            reason: "Invalid device"
        )) {
            try await catalog.bootedSimulators()
        }
    }

    @Test("the list is asked for as JSON")
    func asksForJSON() async throws {
        let runner = FakeRunner(succeedingWith: Data(#"{"devices":{}}"#.utf8))

        _ = try await SimulatorCatalog(runner: runner).bootedSimulators()

        #expect(runner.invocations == [["list", "devices", "booted", "-j"]])
    }
}
