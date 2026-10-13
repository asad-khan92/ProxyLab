//  Moving the LAN listener when the network changes, and not moving loopback.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("LAN address changes")
struct LanMigrationTests {
    private let old = LanAddress(interface: "en0", address: "192.168.110.134")
    private let newer = LanAddress(interface: "en0", address: "192.168.1.25")
    private let ethernet = LanAddress(interface: "en7", address: "10.0.0.8")

    @Test("a bound address that is still offered is left where it is")
    func keepsALiveAddress() {
        let action = LanMigration.action(
            wantsLan: true,
            enabled: true,
            boundAddress: old.address,
            available: [old, ethernet],
            primaryInterface: "en7"
        )

        #expect(action == .none)
    }

    @Test("a gone address is replaced by the one on the primary interface")
    func movesToThePrimaryInterface() {
        let action = LanMigration.action(
            wantsLan: true,
            enabled: true,
            boundAddress: old.address,
            available: [ethernet, newer],
            primaryInterface: "en0"
        )

        #expect(action == .bind(newer.address))
    }

    @Test("no usable address suspends a listener that is up")
    func suspendsWhenTheNetworkIsGone() {
        let action = LanMigration.action(
            wantsLan: true,
            enabled: true,
            boundAddress: old.address,
            available: [],
            primaryInterface: nil
        )

        #expect(action == .suspend)
    }

    @Test("an address coming back binds again")
    func recovers() {
        let action = LanMigration.action(
            wantsLan: true,
            enabled: false,
            boundAddress: nil,
            available: [newer],
            primaryInterface: "en0"
        )

        #expect(action == .bind(newer.address))
    }

    @Test("waiting is not a bind when there is still no address")
    func waits() {
        let action = LanMigration.action(
            wantsLan: true,
            enabled: false,
            boundAddress: nil,
            available: [],
            primaryInterface: nil
        )

        #expect(action == .wait)
    }

    @Test("local network access that is off does not bind because an interface changed")
    func offStaysOff() {
        let action = LanMigration.action(
            wantsLan: false,
            enabled: false,
            boundAddress: nil,
            available: [newer],
            primaryInterface: "en0"
        )

        #expect(action == .none)
    }
}

@Suite("The iPhone section follows the network")
@MainActor
struct IPhoneNetworkChangeTests {
    @Test("an address change removes the old listener by binding the new one")
    func movesTheListener() async {
        let session = FakeLanSession()
        let gone = LanStatusResult(
            enabled: true,
            address: "192.168.110.134",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        let moved = LanStatusResult(
            enabled: true,
            address: "192.168.1.25",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        await session.answerStatuses(with: [.success(LanFixtures.on), .success(gone)])
        await session.answerEnable(with: .success(moved))
        let model = IPhoneSetupModel(session: session)
        await model.refresh()

        await model.networkDidChange(primaryInterface: "en0")

        #expect(await session.calls.contains(.enable("192.168.1.25")))
        #expect(model.state == .on(endpoint: "192.168.1.25:9090"))
        #expect(model.statusText.contains("192.168.110.134") == false)
    }

    @Test("losing the network closes the LAN listener and does not keep the old address")
    func waitsWithoutAStaleAddress() async {
        let session = FakeLanSession()
        let gone = LanStatusResult(
            enabled: true,
            address: LanFixtures.en0.address,
            port: 9_090,
            available: []
        )
        let waiting = LanStatusResult(enabled: false, address: nil, port: 9_090, available: [])
        await session.answerStatuses(with: [.success(LanFixtures.on), .success(gone)])
        await session.answerDisable(with: .success(waiting))
        let model = IPhoneSetupModel(session: session)
        await model.refresh()

        await model.networkDidChange(primaryInterface: nil)

        #expect(await session.calls.contains(.disable))
        #expect(model.state == .waitingForNetwork)
        #expect(model.statusText.contains("Waiting for a network") == true)
        #expect(model.statusText.contains(LanFixtures.en0.address) == false)
        #expect(model.status?.endpoint == nil)
    }

    @Test("an address coming back binds the listener again")
    func recoversTheListener() async {
        let session = FakeLanSession()
        let gone = LanStatusResult(
            enabled: true,
            address: LanFixtures.en0.address,
            port: 9_090,
            available: []
        )
        let waiting = LanStatusResult(enabled: false, address: nil, port: 9_090, available: [])
        let back = LanStatusResult(
            enabled: false,
            address: nil,
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        let moved = LanStatusResult(
            enabled: true,
            address: "192.168.1.25",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        await session.answerStatuses(with: [
            .success(LanFixtures.on),
            .success(gone),
            .success(back),
        ])
        await session.answerDisable(with: .success(waiting))
        await session.answerEnable(with: .success(moved))
        let model = IPhoneSetupModel(session: session)
        await model.refresh()
        await model.networkDidChange(primaryInterface: nil)

        await model.networkDidChange(primaryInterface: "en0")

        #expect(model.state == .on(endpoint: "192.168.1.25:9090"))
    }

    @Test("local network access that is off is not turned on by an interface change")
    func offIsNotEnabledByANetworkChange() async {
        let session = FakeLanSession()
        let changed = LanStatusResult(
            enabled: false,
            address: nil,
            port: 9_090,
            available: [LanAddress(interface: "en5", address: "10.1.1.5")]
        )
        await session.answerStatuses(with: [.success(LanFixtures.off), .success(changed)])
        let model = IPhoneSetupModel(session: session)
        await model.refresh()

        await model.networkDidChange(primaryInterface: "en5")

        let calls = await session.calls
        #expect(!calls.contains { if case .enable = $0 { true } else { false } })
        #expect(model.state == .off)
        #expect(model.selected == "10.1.1.5")
    }
}
