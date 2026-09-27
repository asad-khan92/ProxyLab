//  What the iPhone section shows, and what it refuses to claim.
//
//  The one thing worth being strict about here is that nothing on screen says
//  the proxy is on the local network until the core has said it is. A switch
//  that flips optimistically is a developer who thinks their phone can connect
//  and cannot — and, worse when it goes the other way, a developer who thinks
//  the proxy is off the Wi-Fi when it is still listening.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("The iPhone section")
@MainActor
struct IPhoneSetupModelTests {
    private func model(_ session: FakeLanSession) -> IPhoneSetupModel {
        IPhoneSetupModel(session: session)
    }

    @Test("it starts knowing nothing, and asks")
    func startsByAsking() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)

        #expect(model.state == .loading)
        await model.refresh()

        #expect(await session.calls == [.status])
        #expect(model.state == .off)
    }

    @Test("a proxy on loopback alone is shown as off")
    func offIsOff() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)

        await model.refresh()

        #expect(!model.isOn)
        #expect(model.statusText.contains("Off"))
        #expect(model.status?.endpoint == nil)
    }

    @Test("the port is known whether or not the proxy is on the local network")
    func portIsAlwaysThere() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)

        await model.refresh()

        #expect(model.port == 9_090)
    }

    @Test("the first address offered is the one picked, so there is always one to turn on")
    func picksAnAddress() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)

        await model.refresh()

        #expect(model.selected == LanFixtures.en0.address)
        #expect(model.canEnable)
    }

    @Test("an address the developer chose is kept across a refresh")
    func keepsTheChoice() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)
        await model.refresh()

        model.selected = LanFixtures.en1.address
        await model.refresh()

        #expect(model.selected == LanFixtures.en1.address)
    }

    @Test("an address the Mac no longer has is replaced rather than left to fail")
    func dropsAStaleChoice() async {
        let session = FakeLanSession()
        await session.answerStatuses(with: [
            .success(LanFixtures.off),
            .success(
                LanStatusResult(
                    enabled: false,
                    address: nil,
                    port: 9_090,
                    available: [LanFixtures.en1]
                )
            ),
        ])
        let model = model(session)
        await model.refresh()
        model.selected = LanFixtures.en0.address

        await model.refresh()

        #expect(model.selected == LanFixtures.en1.address)
    }

    @Test("a Mac with no usable address has nothing to turn on")
    func nothingToTurnOn() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.noAddresses)
        let model = model(session)

        await model.refresh()

        #expect(model.addresses.isEmpty)
        #expect(model.selected == nil)
        #expect(!model.canEnable)
    }

    @Test("while it is on, the address shown is the one the proxy is bound to")
    func showsTheBoundAddress() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        await session.answerEnable(with: .success(LanFixtures.on))
        let model = model(session)
        await model.refresh()

        // Picked one address, and the core bound another — which is what a
        // second client changing it looks like from here.
        model.selected = LanFixtures.en1.address
        await model.enable()

        #expect(model.selected == LanFixtures.en0.address)
    }

    @Test("turning it on sends the selected address and shows what the core answered")
    func turningOn() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        await session.answerEnable(with: .success(LanFixtures.on))
        let model = model(session)
        await model.refresh()

        await model.enable()

        #expect(await session.calls == [.status, .enable(LanFixtures.en0.address)])
        #expect(model.isOn)
        #expect(model.state == .on(endpoint: "192.168.1.42:9090"))
    }

    @Test("a refused switch leaves the proxy shown as off, and says why")
    func refusedSwitch() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        await session.answerEnable(
            with: .failure(
                .server(
                    IPCErrorBody(
                        code: .lanUnavailable,
                        message: "that address is not one of this machine's usable local "
                            + "network addresses"
                    )
                )
            )
        )
        let model = model(session)
        await model.refresh()

        await model.enable()

        #expect(!model.isOn)
        #expect(model.lastError?.contains("usable local network") == true)
        #expect(model.exposureWarning == nil)
    }

    @Test("turning it off shows it off only once the core says the socket is closed")
    func turningOff() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.on)
        await session.answerDisable(with: .success(LanFixtures.off))
        let model = model(session)
        await model.refresh()
        #expect(model.isOn)

        await model.disable()

        #expect(await session.calls == [.status, .disable])
        #expect(!model.isOn)
        #expect(model.state == .off)
    }

    @Test("a warning is shown exactly while the proxy is on the local network")
    func warnsWhileOn() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.on)
        await session.answerDisable(with: .success(LanFixtures.off))
        let model = model(session)

        await model.refresh()
        let whileOn = model.exposureWarning

        await model.disable()

        #expect(whileOn?.contains("Any device on this Wi-Fi") == true)
        #expect(whileOn?.contains("no password") == true)
        #expect(model.exposureWarning == nil)
    }

    @Test("a core that cannot be reached is said so rather than shown as off")
    func coreUnreachable() async {
        let session = FakeLanSession()
        await session.answerStatuses(with: [.failure(.notConnected)])
        let model = model(session)

        await model.refresh()

        #expect(model.state == .failed(IPCError.notConnected.message))
        #expect(!model.isOn)
    }

    @Test("nothing is turned on without an address to turn it on at")
    func neverEnablesWithoutAnAddress() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.noAddresses)
        let model = model(session)
        await model.refresh()

        await model.enable()

        #expect(await session.calls == [.status])
    }

    @Test("the instructions name the address and the port the core actually bound")
    func instructionsNameTheEndpoint() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.on)
        let model = model(session)

        await model.refresh()

        let steps = model.instructions.joined(separator: "\n")
        #expect(steps.contains("192.168.1.42"))
        #expect(steps.contains("9090"))
        #expect(steps.contains(IPhoneSetupModel.setupHost))
        #expect(steps.contains("Certificate Trust Settings"))
    }

    @Test("the instructions say to turn it on first when it is off")
    func instructionsWhenOff() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.off)
        let model = model(session)

        await model.refresh()

        #expect(model.instructions.first?.contains("Turn on local network access") == true)
    }

    @Test("the trust step is one of the steps, because it is the one people miss")
    func trustIsASeparateStep() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.on)
        let model = model(session)

        await model.refresh()

        #expect(
            model.instructions.contains { $0.contains("HTTPS is not readable until this step") }
        )
    }
}
