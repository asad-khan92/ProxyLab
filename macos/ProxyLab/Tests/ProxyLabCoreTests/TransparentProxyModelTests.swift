//  What the Network Extension section shows, and what it refuses to claim.
//
//  The properties worth being strict about: nothing turns capture on unless
//  Turn On was pressed, nothing says "on" until the system does, and a
//  capture that is on while the core is not there says that nothing is being
//  captured.

import Foundation
import Testing
import TransparentProxyKit

@testable import ProxyLabCore

@Suite("The Network Extension section")
@MainActor
struct TransparentProxyModelTests {
    private func model(_ control: FakeTransparentProxyControl) -> TransparentProxyModel {
        TransparentProxyModel(control: control, bridgePort: 9_091)
    }

    @Test("refreshing reads the system and changes nothing")
    func refreshChangesNothing() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let model = model(control)

        await model.refresh()

        #expect(control.calls == [.snapshot])
        #expect(!model.isOn)
        #expect(model.statusText.hasPrefix("Off"))
        #expect(model.canTurnOn)
    }

    @Test("a build that cannot install an extension asks nothing and says why")
    func unavailable() async {
        for readiness in [
            TransparentProxyReadiness.notAnAppBundle, .extensionMissing, .unsigned, .notInApplications,
        ] {
            let control = FakeTransparentProxyControl(readiness: readiness)
            let model = model(control)

            await model.refresh()
            await model.turnOn()

            #expect(control.calls.isEmpty, "\(readiness)")
            #expect(!model.canTurnOn)
            #expect(model.readiness.message?.isEmpty == false)
        }
    }

    @Test("turning on installs the extension first, then connects it to the core's bridge port")
    func turnOnInstalls() async {
        let control = FakeTransparentProxyControl(extensionState: .notInstalled)
        let model = model(control)

        await model.turnOn()

        #expect(control.calls == [.snapshot, .activate, .turnOn(9_091), .snapshot])
        #expect(model.isOn)
        #expect(model.statusText.hasPrefix("On"))
        #expect(model.lastError == nil)
    }

    @Test("an installed extension is not asked for again")
    func installedIsNotReinstalled() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let model = model(control)

        await model.turnOn()

        #expect(control.count(.activate) == 0)
        #expect(control.count(.turnOn(9_091)) == 1)
    }

    @Test("waiting for approval says where to give it, and does not claim to be on")
    func awaitingApproval() async {
        let control = FakeTransparentProxyControl()
        control.activation = .success(.awaitingApproval)
        let model = model(control)

        await model.turnOn()

        #expect(!model.isOn)
        #expect(control.count(.turnOn(9_091)) == 0)
        #expect(model.notice == TransparentProxyModel.approvalInstructions)
        #expect(model.extensionText == "Extension waiting for approval")
    }

    @Test("approval finishes the Turn On that was waiting for it")
    func approvalFinishesTurnOn() async throws {
        let control = FakeTransparentProxyControl()
        control.activation = .success(.awaitingApproval)
        let model = model(control)
        await model.turnOn()

        control.approve()
        try await waitUntil { model.isOn }

        #expect(control.count(.turnOn(9_091)) == 1)
        #expect(model.notice == nil)
    }

    @Test("an approval nobody was waiting for turns nothing on")
    func strayApproval() async throws {
        let control = FakeTransparentProxyControl(extensionState: .awaitingApproval)
        let model = model(control)
        await model.refresh()

        control.approve()
        try await waitUntil { model.snapshot?.extensionState == .installed }

        #expect(control.count(.turnOn(9_091)) == 0)
        #expect(!model.isOn)
    }

    @Test("an install that needs a restart says so, and turns nothing on")
    func reboot() async {
        let control = FakeTransparentProxyControl()
        control.activation = .success(.willCompleteAfterReboot)
        let model = model(control)

        await model.turnOn()

        #expect(!model.isOn)
        #expect(model.notice?.contains("Restart") == true)
    }

    @Test("a refusal from the system is shown as something to do")
    func refusal() async {
        let control = FakeTransparentProxyControl()
        control.activation = .failure(TransparentProxyFailure.from(
            NSError(domain: "OSSystemExtensionErrorDomain", code: 3)
        ))
        let model = model(control)

        await model.turnOn()

        #expect(!model.isOn)
        #expect(model.lastError?.contains("/Applications") == true)
    }

    @Test("a declined configuration prompt says how to try again")
    func declinedConfiguration() async {
        let control = FakeTransparentProxyControl(extensionState: .installed)
        control.turnOnError = NSError(domain: "NEVPNErrorDomain", code: 5)
        let model = model(control)

        await model.turnOn()

        #expect(!model.isOn)
        #expect(model.lastError?.contains("Allow") == true)
    }

    @Test("turning off disconnects, and leaves the extension installed")
    func turnOff() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .on)
        let model = model(control)
        await model.refresh()

        await model.turnOff()

        #expect(control.count(.turnOff) == 1)
        #expect(!model.isOn)
        #expect(model.snapshot?.extensionState == .installed)
    }

    @Test("while on, the warning says what capturing everything breaks")
    func warningWhileOn() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .on)
        control.status = ProviderStatus(bridge: .reachable, counters: .init(bridged: 3), lastBridgeFailure: nil)
        let model = model(control)
        await model.refresh()

        let warning = model.warning(coreConnected: true)

        #expect(warning?.contains("pin their certificates") == true)
        #expect(!model.isPassingThrough(coreConnected: true))
    }

    @Test("on with no core says nothing is being captured")
    func noCore() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .on)
        let model = model(control)
        await model.refresh()

        #expect(model.isPassingThrough(coreConnected: false))
        #expect(model.warning(coreConnected: false)?.contains("nothing is captured") == true)
    }

    @Test("the provider saying the bridge is unreachable is believed over the window's connection")
    func bridgeUnreachable() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .on)
        control.status = ProviderStatus(bridge: .unreachable, counters: .init(), lastBridgeFailure: "refused")
        let model = model(control)
        await model.refresh()

        #expect(model.isPassingThrough(coreConnected: true))
    }

    @Test("off says nothing about being on")
    func noWarningWhenOff() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let model = model(control)
        await model.refresh()

        #expect(model.warning(coreConnected: false) == nil)
        #expect(control.count(.providerStatus) == 0)
    }

    /// Waits for a change that arrives through `onChange`, which the model
    /// handles in a task of its own.
    private func waitUntil(_ condition: @MainActor () -> Bool) async throws {
        for _ in 0..<200 where !condition() {
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(condition())
    }
}

@Suite("Whether transparent capture can be used")
struct TransparentProxyReadinessTests {
    @Test("the most fundamental problem is the one reported")
    func order() {
        #expect(TransparentProxyReadiness.evaluate(
            bundlePath: "/Users/dev/.build/debug", isAppBundle: false,
            extensionPresent: false, hasInstallEntitlement: false
        ) == .notAnAppBundle)
        #expect(TransparentProxyReadiness.evaluate(
            bundlePath: "/Applications/ProxyLab.app", isAppBundle: true,
            extensionPresent: false, hasInstallEntitlement: true
        ) == .extensionMissing)
        #expect(TransparentProxyReadiness.evaluate(
            bundlePath: "/Applications/ProxyLab.app", isAppBundle: true,
            extensionPresent: true, hasInstallEntitlement: false
        ) == .unsigned)
        #expect(TransparentProxyReadiness.evaluate(
            bundlePath: "/Users/dev/Applications/ProxyLab.app", isAppBundle: true,
            extensionPresent: true, hasInstallEntitlement: true
        ) == .notInApplications)
        #expect(TransparentProxyReadiness.evaluate(
            bundlePath: "/Applications/ProxyLab.app", isAppBundle: true,
            extensionPresent: true, hasInstallEntitlement: true
        ) == .ready)
    }

    @Test("every problem says what to do, and ready says nothing")
    func messages() {
        #expect(TransparentProxyReadiness.ready.message == nil)
        for readiness in [
            TransparentProxyReadiness.notAnAppBundle, .extensionMissing, .unsigned, .notInApplications,
        ] {
            #expect(readiness.message?.isEmpty == false, "\(readiness)")
        }
    }

    @Test("every error the two frameworks publish becomes a sentence")
    func failures() {
        for code in 1...13 {
            let failure = TransparentProxyFailure.from(NSError(domain: "OSSystemExtensionErrorDomain", code: code))
            #expect(!failure.message.isEmpty, "OSSystemExtension \(code)")
        }
        for code in 1...6 {
            let failure = TransparentProxyFailure.from(NSError(domain: "NEVPNErrorDomain", code: code))
            #expect(!failure.message.isEmpty, "NEVPN \(code)")
        }
    }

    @Test("a missing entitlement points at how the app is signed")
    func missingEntitlement() {
        let failure = TransparentProxyFailure.from(NSError(domain: "OSSystemExtensionErrorDomain", code: 2))

        #expect(failure.message == TransparentProxyReadiness.unsigned.message)
    }

    @Test("the core's bridge port is the one the provider is given")
    func bridgePort() {
        #expect(CoreLocations.transparentBridgePort == Int(ProviderConfiguration.defaultBridgePort))
    }
}
