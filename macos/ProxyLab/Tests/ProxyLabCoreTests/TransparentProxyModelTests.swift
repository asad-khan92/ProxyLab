//  What the Network Extension section shows, and what it refuses to claim.
//
//  The properties worth being strict about: nothing turns capture on unless
//  Turn On was pressed, nothing says "on" until the system does, and a
//  capture that is on while the core is not there says that nothing is being
//  captured.

import Foundation
import SystemExtensions
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

    @Test("a signed build with nothing installed yet can turn on, and turning on installs")
    func notInstalledCanTurnOn() async {
        let control = FakeTransparentProxyControl(extensionState: .notInstalled, tunnel: .off)
        let model = model(control)
        await model.refresh()

        #expect(model.canTurnOn)
        #expect(model.extensionText == "Extension not installed — Turn On installs it")
        #expect(model.statusText.hasPrefix("Off"))

        await model.turnOn()

        #expect(control.calls == [.snapshot, .snapshot, .activate, .turnOn(9_091), .snapshot])
        #expect(model.isOn)
    }

    @Test("an installed extension that is off can turn on without installing again")
    func installedAndOffCanTurnOn() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let model = model(control)
        await model.refresh()

        #expect(model.canTurnOn)
        #expect(!model.canTurnOff)

        await model.turnOn()

        #expect(control.count(.activate) == 0)
        #expect(control.count(.turnOn(9_091)) == 1)
    }

    @Test("capture that is on can turn off")
    func captureOnCanTurnOff() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .on)
        let model = model(control)
        await model.refresh()

        #expect(model.canTurnOff)
        #expect(!model.canTurnOn)

        await model.turnOff()

        #expect(control.count(.turnOff) == 1)
        #expect(!model.isOn)
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
            #expect(!model.canRefresh)
            #expect(model.extensionText == nil)
            #expect(model.readiness.message?.isEmpty == false)
        }
    }

    @Test("refresh stays available when the extension is simply not installed yet")
    func refreshWhileNotInstalled() async {
        let control = FakeTransparentProxyControl(extensionState: .notInstalled, tunnel: .notConfigured)
        let model = model(control)

        #expect(model.canRefresh)

        await model.refresh()

        #expect(control.calls == [.snapshot])
        #expect(model.canRefresh)
        #expect(model.extensionText == "Extension not installed — Turn On installs it")
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

    @Test("an enabled extension at the same version does not surface a validation failure as a signature error")
    func installedSameVersionSkipsActivation() async {
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        control.activation = .failure(TransparentProxyFailure.from(
            systemExtensionError(code: 9, description: "Extension validation failed")
        ))
        let model = model(control)

        await model.turnOn()

        #expect(control.count(.activate) == 0)
        #expect(control.count(.turnOn(9_091)) == 1)
        #expect(model.isOn)
        #expect(model.lastError == nil)
        #expect(model.lastErrorDetail == nil)
    }

    @Test("a different installed version still goes through activation before the configuration starts")
    func differentVersionIsReplaced() async {
        let control = FakeTransparentProxyControl(
            extensionState: .installed,
            tunnel: .off,
            requiresActivation: true
        )
        let model = model(control)

        await model.turnOn()

        #expect(control.calls == [.snapshot, .activate, .turnOn(9_091), .snapshot])
        #expect(model.isOn)
        #expect(model.lastError == nil)
    }

    @Test("a rejected signature stays a signature failure, with the system error kept")
    func signatureFailureKeepsTheSystemError() async {
        let control = FakeTransparentProxyControl(extensionState: .notInstalled)
        control.activation = .failure(TransparentProxyFailure.from(systemExtensionError(
            code: OSSystemExtensionError.Code.codeSignatureInvalid.rawValue,
            description: "Invalid code signature."
        )))
        let model = model(control)

        await model.turnOn()

        #expect(!model.isOn)
        #expect(model.lastError?.contains("rejected the extension's signature") == true)
        #expect(model.lastErrorDetail?.contains("OSSystemExtensionErrorDomain 8") == true)
        #expect(model.lastErrorDetail?.contains("Invalid code signature.") == true)
    }

    @Test("a validation failure is not described as a rejected signature")
    func validationFailureIsNotASignatureError() async {
        let control = FakeTransparentProxyControl(extensionState: .notInstalled)
        control.activation = .failure(TransparentProxyFailure.from(systemExtensionError(
            code: OSSystemExtensionError.Code.validationFailed.rawValue,
            description: "Extension validation failed",
            underlying: NSError(
                domain: "NSOSStatusErrorDomain",
                code: 4,
                userInfo: [NSLocalizedDescriptionKey: "validation status"]
            )
        )))
        let model = model(control)

        await model.turnOn()

        #expect(model.lastError?.contains("could not validate") == true)
        #expect(model.lastError?.contains("rejected the extension's signature") == false)
        #expect(model.lastErrorDetail?.contains("OSSystemExtensionErrorDomain 9") == true)
        #expect(model.lastErrorDetail?.contains("Extension validation failed") == true)
        #expect(model.lastErrorDetail?.contains("underlying NSOSStatusErrorDomain 4") == true)
        #expect(model.lastErrorDetail?.contains("validation status") == true)
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

@Suite("System extension errors and replacement")
struct SystemExtensionActivationTests {
    @Test("code 8 is a rejected signature and code 9 is validation")
    func appleCodes() {
        #expect(OSSystemExtensionError.Code.codeSignatureInvalid.rawValue == 8)
        #expect(OSSystemExtensionError.Code.validationFailed.rawValue == 9)
        #expect(OSSystemExtensionError.Code.requestCanceled.rawValue == 11)

        let signature = TransparentProxyFailure.from(systemExtensionError(
            code: OSSystemExtensionError.Code.codeSignatureInvalid.rawValue,
            description: "Invalid code signature."
        ))
        let validation = TransparentProxyFailure.from(systemExtensionError(
            code: OSSystemExtensionError.Code.validationFailed.rawValue,
            description: "Extension validation failed"
        ))

        #expect(signature.message.contains("rejected the extension's signature"))
        #expect(signature.domain == (OSSystemExtensionErrorDomain as String))
        #expect(signature.code == 8)
        #expect(signature.systemDescription == "Invalid code signature.")
        #expect(signature.diagnosticRecord?.contains("OSSystemExtensionErrorDomain 8") == true)

        #expect(validation.message.contains("could not validate"))
        #expect(validation.message.contains("rejected the extension's signature") == false)
        #expect(validation.code == 9)
        #expect(validation.systemDescription == "Extension validation failed")
    }

    @Test("an enabled extension at the same version is used, and a different version is activated")
    func plan() {
        let current = ExtensionVersion(shortVersion: "0.1.0", bundleVersion: "1")
        let rebuilt = ExtensionVersion(shortVersion: "0.1.0", bundleVersion: "2")

        #expect(ExtensionActivation.plan(state: .installed, installed: current, bundled: current) == .useInstalled)
        #expect(ExtensionActivation.plan(state: .installed, installed: current, bundled: rebuilt) == .activate)
        #expect(ExtensionActivation.plan(state: .installed, installed: nil, bundled: current) == .useInstalled)
        #expect(ExtensionActivation.plan(state: .notInstalled, installed: nil, bundled: current) == .activate)
        #expect(ExtensionActivation.plan(state: .awaitingApproval, installed: current, bundled: current) == .activate)
        #expect(ExtensionActivation.plan(
            state: .unknown, installed: nil, bundled: current, configurationSaved: true
        ) == .useInstalled)
        #expect(ExtensionActivation.plan(
            state: .unknown, installed: nil, bundled: current, configurationSaved: false
        ) == .activate)
        #expect(ExtensionActivation.plan(
            state: .notInstalled, installed: nil, bundled: current, configurationSaved: true
        ) == .activate)
    }

    @Test("replacement continues only when the version differs")
    func replacement() {
        let current = ExtensionVersion(shortVersion: "0.1.0", bundleVersion: "1")
        let newerBuild = ExtensionVersion(shortVersion: "0.1.0", bundleVersion: "2")
        let newerShort = ExtensionVersion(shortVersion: "0.2.0", bundleVersion: "1")

        #expect(ExtensionActivation.replacement(existing: current, proposed: current) == .keepInstalled)
        #expect(ExtensionActivation.replacement(existing: current, proposed: newerBuild) == .replace)
        #expect(ExtensionActivation.replacement(existing: current, proposed: newerShort) == .replace)
    }

    @Test("declining a same-version replacement leaves the installed extension usable")
    func declinedReplacement() {
        #expect(ExtensionActivation.useInstalledAfterCancellation(
            domain: OSSystemExtensionErrorDomain as String,
            code: OSSystemExtensionError.Code.requestCanceled.rawValue
        ))
        #expect(!ExtensionActivation.useInstalledAfterCancellation(
            domain: OSSystemExtensionErrorDomain as String,
            code: OSSystemExtensionError.Code.validationFailed.rawValue
        ))
        #expect(!ExtensionActivation.useInstalledAfterCancellation(
            domain: OSSystemExtensionErrorDomain as String,
            code: OSSystemExtensionError.Code.codeSignatureInvalid.rawValue
        ))
    }

    @Test("the bundle's two version strings are the ones macOS compares")
    func bundleVersion() {
        let version = ExtensionVersion(info: [
            "CFBundleShortVersionString": "0.1.0",
            "CFBundleVersion": "1",
        ])

        #expect(version == ExtensionVersion(shortVersion: "0.1.0", bundleVersion: "1"))
        #expect(ExtensionVersion(info: ["CFBundleShortVersionString": "0.1.0"]) == nil)
    }
}

/// An `OSSystemExtensionErrorDomain` error with the description and underlying
/// error a failing activation carries.
private func systemExtensionError(
    code: Int,
    description: String,
    underlying: NSError? = nil
) -> NSError {
    var info: [String: Any] = [NSLocalizedDescriptionKey: description]
    if let underlying {
        info[NSUnderlyingErrorKey] = underlying
    }
    return NSError(domain: OSSystemExtensionErrorDomain as String, code: code, userInfo: info)
}
