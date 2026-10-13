//  App-owned capture, exercised without a bundled engine and without macOS
//  installing an extension.
//
//  The core process, the readiness check, and the system are stand-ins, so
//  what is asserted is the order: the engine starts, readiness is asked, and
//  only then is transparent capture turned on; Stop and quit turn that off
//  before the owned process is stopped; a port that is already taken is not
//  killed.

import Darwin
import Foundation
import SwiftUI
import Testing
import TransparentProxyKit

@testable import ProxyLabCore

/// Wall-clock ceiling for a wait whose own bound is a fraction of a second.
/// A busy main actor can postpone a short sleep. This still rejects a hang.
private let wallClock = Duration.seconds(8)

@MainActor
@Suite("App-owned capture")
struct CaptureLifecycleTests {
    @Test("launching the app starts the owned core and then listens")
    func launchStartsTheCore() async {
        let harness = CaptureHarness()

        await harness.lifecycle.startOnLaunch()

        #expect(harness.core.launchCount == 1)
        #expect(harness.lifecycle.phase == .listening)
        #expect(harness.lifecycle.coreIsRunning)
        #expect(harness.lifecycle.presentation.title == "Listening")
        #expect(harness.lifecycle.presentation.action == .stop)
    }

    @Test("transparent capture starts only after the core is ready")
    func transparentWaitsForReadiness() async {
        let harness = CaptureHarness()

        await harness.lifecycle.start()

        #expect(harness.trace.events == ["launch", "ready", "trust", "claim"])
        #expect(harness.readiness.turnOnHadAlreadyHappened == false)
        #expect(harness.control.count(.turnOn(9_091)) == 1)
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("a core that is not ready does not get transparent capture")
    func notReadyDoesNotCapture() async {
        let harness = CaptureHarness(readiness: .bridgeUnavailable("The transparent bridge is not listening."))

        await harness.lifecycle.start()

        #expect(harness.core.launchCount == 1)
        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(harness.control.count(.turnOn(9_091)) == 0)
        #expect(harness.control.count(.activate) == 0)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.statusDetail?.contains("bridge") == true)
        #expect(!harness.lifecycle.transparent.isOn)
    }

    @Test("starting again does not launch a second core")
    func startDoesNotDuplicate() async {
        let harness = CaptureHarness()

        await harness.lifecycle.start()
        await harness.lifecycle.start()

        #expect(harness.core.launchCount == 1)
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("two overlapping starts still launch one core")
    func overlappingStartsLaunchOnce() async {
        let harness = CaptureHarness()

        let first = Task { await harness.lifecycle.start() }
        let second = Task { await harness.lifecycle.start() }
        await first.value
        await second.value

        #expect(harness.core.launchCount == 1)
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("stop turns transparent capture off before it stops the owned core")
    func stopOrder() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.core.sawTurnOffBeforeStop)
        #expect(harness.core.stopCount == 1)
        #expect(!harness.core.isRunning)
        #expect(!harness.lifecycle.transparent.isOn)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.control.current.extensionState == .installed)
    }

    @Test("stop terminates the owned core")
    func stopTerminatesTheCore() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.core.stopCount == 1)
        #expect(!harness.lifecycle.coreIsRunning)
        #expect(harness.lifecycle.presentation.title == "Stopped")
        #expect(harness.lifecycle.presentation.action == .start)
        #expect(!harness.lifecycle.requiresCleanupBeforeQuit)
        #expect(harness.lifecycle.canExit)
    }

    @Test("quit performs the same shutdown and leaves the extension installed")
    func quitShutsDown() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()
        #expect(harness.lifecycle.requiresCleanupBeforeQuit)

        await harness.lifecycle.quit()

        #expect(harness.core.sawTurnOffBeforeStop)
        #expect(harness.core.stopCount == 1)
        #expect(!harness.core.isRunning)
        #expect(!harness.lifecycle.transparent.isOn)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
        #expect(harness.control.current.extensionState == .installed)
        #expect(harness.control.count(.activate) == 0)
    }

    @Test("an unexpected core exit turns capture off and stops claiming Listening")
    func coreCrash() async throws {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        harness.core.crash()
        try await waitUntil { Self.isTerminal(harness.lifecycle.phase) }

        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.statusDetail?.contains("Transparent capture was turned off") == true)
        #expect(harness.proxies.restoreCount == 1)
        #expect(!harness.proxies.hasClaim)
        #expect(harness.control.count(.turnOff) == 1)
        #expect(!harness.lifecycle.transparent.isOn)
        #expect(!harness.lifecycle.coreIsRunning)
    }

    @Test("a leftover proxy is restored after the listener is ready, and before a new claim")
    func abandonedClaimIsRestoredAfterTheListenerIsReady() async {
        let proxies = FakeSystemProxy()
        proxies.hasClaim = true
        let harness = CaptureHarness(proxies: proxies)

        await harness.lifecycle.start()

        let launch = harness.trace.events.firstIndex(of: "launch")
        let ready = harness.trace.events.firstIndex(of: "ready")
        let restore = harness.trace.events.firstIndex(of: "restore")
        let claim = harness.trace.events.firstIndex(of: "claim")
        #expect(launch != nil && ready != nil && restore != nil && claim != nil)
        #expect(launch! < ready! && ready! < restore! && restore! < claim!)
        #expect(harness.proxies.claimPorts == [9_090])
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("a leftover proxy that cannot be put back leaves the ready core running")
    func abandonedClaimThatCannotBeRestoredKeepsTheCore() async {
        let proxies = FakeSystemProxy()
        proxies.hasClaim = true
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: [],
            vanished: [],
            stillClaimed: true,
            message: "This Mac is still sending HTTP and HTTPS through ProxyLab at 127.0.0.1:9090. The proxy engine was left running."
        )
        let harness = CaptureHarness(proxies: proxies)

        await harness.lifecycle.start()

        #expect(harness.trace.events == ["launch", "ready", "restore"])
        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(harness.proxies.hasClaim)
        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.presentation.action == .stop)
        #expect(harness.lifecycle.statusDetail?.contains("127.0.0.1:9090") == true)
    }

    @Test("the system proxy is not configured until the explicit listener is ready")
    func proxyWaitsForTheListener() async {
        let harness = CaptureHarness(readiness: .timedOut)

        await harness.lifecycle.start()

        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(harness.trace.events == ["launch", "ready"])
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(!harness.proxies.hasClaim)
    }

    @Test("start claims the system proxy only after the core is ready, and before transparent capture")
    func proxyIsClaimedAfterReady() async {
        let harness = CaptureHarness()

        await harness.lifecycle.start()

        #expect(harness.proxies.claimPorts == [9_090])
        #expect(harness.trace.events == ["launch", "ready", "trust", "claim"])
        let claim = harness.trace.events.firstIndex(of: "claim")
        #expect(claim != nil)
        #expect(harness.control.count(.turnOn(9_091)) == 1)
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("stop restores the system proxy before it stops the owned core")
    func stopRestoresTheProxyBeforeTheCore() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.proxies.restoreCount == 1)
        #expect(!harness.proxies.hasClaim)
        let restore = harness.trace.events.firstIndex(of: "restore")
        let stopped = harness.trace.events.firstIndex(of: "core-stop")
        #expect(restore != nil && stopped != nil && restore! < stopped!)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
    }

    @Test("quit restores the system proxy before it stops the owned core")
    func quitRestoresTheProxyBeforeTheCore() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        await harness.lifecycle.quit()

        let restore = harness.trace.events.firstIndex(of: "restore")
        let stopped = harness.trace.events.firstIndex(of: "core-stop")
        #expect(restore != nil && stopped != nil && restore! < stopped!)
        #expect(!harness.proxies.hasClaim)
        #expect(harness.lifecycle.canExit)
    }

    @Test("an expired restore approval leaves the core running until a later restore succeeds")
    func expiredAuthorizationKeepsTheCoreUntilRestoreSucceeds() async {
        let proxies = FakeSystemProxy()
        proxies.restoreResults = [
            SystemProxyRestoreReport(
                restored: [],
                leftInPlace: [],
                vanished: [],
                stillClaimed: true,
                message: SystemProxyPlan.authorizationBlockedMessage(host: "127.0.0.1", port: 9_090)
            ),
            SystemProxyRestoreReport.none,
        ]
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.statusDetail?.contains("127.0.0.1:9090") == true)
        #expect(harness.lifecycle.statusDetail?.contains("approval") == true)

        await harness.lifecycle.stop()

        let restore = harness.trace.events.lastIndex(of: "restore")
        let stopped = harness.trace.events.lastIndex(of: "core-stop")
        #expect(restore != nil && stopped != nil && restore! < stopped!)
        #expect(!harness.core.isRunning)
        #expect(harness.lifecycle.canExit)
    }

    @Test("cancelling restore approval does not stop the core or allow quit")
    func cancelledAuthorizationBlocksQuit() async {
        let proxies = FakeSystemProxy()
        proxies.restoreError = SystemProxyError.authorizationCancelled
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.quit()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(harness.proxies.hasClaim)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.requiresCleanupBeforeQuit)
        let detail = harness.lifecycle.statusDetail ?? ""
        #expect(detail.contains("127.0.0.1:9090"))
        #expect(detail.contains("before quitting"))
        #expect(harness.lifecycle.presentation.action == .stop)
    }

    @Test("a restore whose read-back is still the owned proxy leaves the core running")
    func readBackStillOwnedKeepsTheCore() async {
        let proxies = FakeSystemProxy()
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: [],
            vanished: [],
            stillClaimed: true,
            message: SystemProxyPlan.stillOwnedMessage(host: "127.0.0.1", port: 9_090)
        )
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.statusDetail?.contains("127.0.0.1:9090") == true)
    }

    @Test("a proxy that is already back at its baseline can stop the core")
    func alreadyRestoredProxyCanStop() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()
        harness.proxies.restoreReport = .none

        await harness.lifecycle.stop()

        #expect(harness.proxies.restoreCount == 1)
        #expect(!harness.core.isRunning)
        #expect(harness.lifecycle.canExit)
        #expect(harness.lifecycle.phase == .stopped)
    }

    @Test("an unreadable proxy state leaves the core running")
    func unreadableProxyKeepsTheCore() async {
        let proxies = FakeSystemProxy()
        proxies.restoreError = SystemProxyError.unreadable
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.quit()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.statusDetail?.contains("could not verify") == true)
        #expect(harness.lifecycle.statusDetail?.contains("127.0.0.1:9090") == true)
    }

    @Test("a startup failure after the proxy is owned restores it before stopping the core")
    func startupFailureAfterClaimRestoresBeforeTheCoreStops() async {
        let harness = CaptureHarness()
        harness.lifecycle.tunnelStartBound = .zero
        harness.control.turnOnError = NSError(domain: "NEVPNErrorDomain", code: 1)

        await harness.lifecycle.start()

        let restore = harness.trace.events.firstIndex(of: "restore")
        let stopped = harness.trace.events.firstIndex(of: "core-stop")
        #expect(restore != nil && stopped != nil && restore! < stopped!)
        #expect(!harness.core.isRunning)
        #expect(harness.lifecycle.canExit)
        #expect(harness.lifecycle.presentation.title == "Error")
    }

    @Test("a startup failure that cannot restore the proxy leaves the core running")
    func startupFailureThatCannotRestoreKeepsTheCore() async {
        let proxies = FakeSystemProxy()
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: [],
            vanished: [],
            stillClaimed: true,
            message: SystemProxyPlan.stillOwnedMessage(host: "127.0.0.1", port: 9_090)
        )
        let harness = CaptureHarness(proxies: proxies)
        harness.lifecycle.tunnelStartBound = .zero
        harness.control.turnOnError = NSError(domain: "NEVPNErrorDomain", code: 1)

        await harness.lifecycle.start()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.proxies.claimPorts == [9_090])
    }

    @Test("the system proxy is not claimed when certificate trust is not ready")
    func trustGatesTheSystemProxy() async {
        let harness = CaptureHarness()
        harness.trust.result = .notReady("ProxyLab cannot yet check whether this Mac trusts its certificate.")

        await harness.lifecycle.start()

        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(harness.core.isRunning)
        #expect(harness.trace.events == ["launch", "ready", "trust"])
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.statusDetail?.contains("trusts its certificate") == true)
        #expect(harness.control.count(.turnOn(9_091)) == 0)
    }

    @Test("an unexpected core exit that cannot restore the proxy is a critical error")
    func coreExitThatCannotRestoreStaysBlocked() async throws {
        let proxies = FakeSystemProxy()
        proxies.restoreError = SystemProxyError.unreadable
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        harness.core.crash()
        try await waitUntil { Self.isTerminal(harness.lifecycle.phase) }

        #expect(harness.proxies.restoreCount == 1)
        #expect(!harness.core.isRunning)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.requiresCleanupBeforeQuit)
        let detail = harness.lifecycle.statusDetail ?? ""
        #expect(detail.contains("127.0.0.1:9090"))
        #expect(detail.contains("could not verify"))
    }

    @Test("an externally changed proxy is reported and not treated as still owned")
    func externalProxyChangeIsLeftAlone() async {
        let proxies = FakeSystemProxy()
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: ["Ethernet"],
            vanished: [],
            stillClaimed: false,
            message: "The HTTP and HTTPS proxy on Ethernet changed after ProxyLab set it. ProxyLab left that setting in place."
        )
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(!harness.core.isRunning)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.statusDetail?.contains("Ethernet") == true)
        #expect(harness.lifecycle.statusDetail?.contains("left that setting in place") == true)
        #expect(harness.lifecycle.canExit)
    }

    @Test("a proxy that cannot be restored leaves the owned core running")
    func failedRestoreKeepsTheCore() async {
        let proxies = FakeSystemProxy()
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: [],
            vanished: [],
            stillClaimed: true,
            message: "ProxyLab could not put the previous HTTP and HTTPS proxy back. The proxy was left running."
        )
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.stop()

        #expect(harness.core.isRunning)
        #expect(harness.proxies.hasClaim)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.presentation.action == .stop)
    }

    @Test("a network change asks the proxy owner, and does not before a claim")
    func networkChangeAdoptsOnlyWhileClaimed() async {
        let harness = CaptureHarness()

        harness.lifecycle.activeNetworkChanged()
        #expect(harness.proxies.adoptCount == 0)

        await harness.lifecycle.start()
        harness.lifecycle.activeNetworkChanged()

        #expect(harness.proxies.adoptCount == 1)
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("occupied ports are an error and the other process is not stopped")
    func occupiedPortsAreLeftAlone() async {
        let harness = CaptureHarness(occupied: [9_090, 9_091])

        await harness.lifecycle.startOnLaunch()

        #expect(harness.core.launchCount == 0)
        #expect(harness.core.stopCount == 0)
        #expect(harness.readiness.waitCount == 0)
        #expect(harness.control.calls.isEmpty)
        #expect(harness.lifecycle.presentation.title == "Error")
        let detail = harness.lifecycle.statusDetail ?? ""
        #expect(detail.contains("9090"))
        #expect(detail.contains("9091"))
        #expect(detail.contains("left the other process running"))
        #expect(!harness.lifecycle.transparent.isOn)
    }

    @Test("the first launch waits for approval instead of hanging or asking twice")
    func needsApproval() async {
        let harness = CaptureHarness(
            extensionState: .notInstalled,
            activation: .success(.awaitingApproval)
        )

        await harness.lifecycle.startOnLaunch()

        #expect(harness.lifecycle.phase == .needsApproval)
        #expect(harness.lifecycle.presentation.title == "Needs Approval")
        #expect(harness.lifecycle.presentation.action == .stop)
        #expect(harness.control.count(.activate) == 1)
        #expect(harness.control.count(.turnOn(9_091)) == 0)
        #expect(!harness.lifecycle.transparent.isOn)
        #expect(harness.model.notice == TransparentProxyModel.approvalInstructions)

        await harness.lifecycle.start()

        #expect(harness.control.count(.activate) == 1)
        #expect(harness.lifecycle.phase == .needsApproval)
    }

    @Test("approval finishes startup, and Stop before that does not")
    func approvalContinuesUnlessStopped() async throws {
        let waiting = CaptureHarness(
            extensionState: .notInstalled,
            activation: .success(.awaitingApproval)
        )
        await waiting.lifecycle.start()
        waiting.control.approve()
        try await waitUntil { waiting.lifecycle.phase == .listening }

        #expect(waiting.control.count(.activate) == 1)
        #expect(waiting.control.count(.turnOn(9_091)) == 1)
        #expect(waiting.lifecycle.transparent.isOn)

        let cancelled = CaptureHarness(
            extensionState: .notInstalled,
            activation: .success(.awaitingApproval)
        )
        await cancelled.lifecycle.start()
        await cancelled.lifecycle.stop()
        let calls = cancelled.control.calls.count
        cancelled.control.approve()
        try await waitUntil { cancelled.control.calls.count > calls }

        #expect(cancelled.lifecycle.phase == .stopped)
        #expect(cancelled.control.count(.turnOn(9_091)) == 0)
        #expect(!cancelled.lifecycle.transparent.isOn)
        #expect(!cancelled.core.isRunning)
    }

    @Test("an installed extension starts capture without being activated again")
    func installedExtensionStarts() async {
        let harness = CaptureHarness(extensionState: .installed, tunnel: .off)

        await harness.lifecycle.startOnLaunch()

        #expect(harness.control.count(.activate) == 0)
        #expect(harness.control.count(.turnOn(9_091)) == 1)
        #expect(harness.lifecycle.phase == .listening)
        #expect(harness.lifecycle.transparent.isOn)
    }

    @Test("a Stop is remembered, so an automatic start does not turn capture back on")
    func stopBlocksAutomaticRestart() async {
        let harness = CaptureHarness()
        await harness.lifecycle.startOnLaunch()
        await harness.lifecycle.stop()

        await harness.lifecycle.startOnLaunch()
        await harness.lifecycle.startAutomatically()

        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.core.launchCount == 1)
        #expect(!harness.core.isRunning)

        await harness.lifecycle.start()

        #expect(harness.lifecycle.phase == .listening)
        #expect(harness.core.launchCount == 2)
    }

    @Test("stop does not stop the core while transparent capture is still on")
    func stopLeavesTheCoreWhenTheTunnelStaysOn() async {
        let harness = CaptureHarness()
        harness.lifecycle.transparentStopBound = .zero
        await harness.lifecycle.start()
        harness.control.turnOffError = NSError(domain: "NEVPNErrorDomain", code: 1)

        await harness.lifecycle.stop()

        #expect(harness.lifecycle.transparent.isOn)
        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.presentation.action == .stop)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.requiresCleanupBeforeQuit)
    }

    @Test("the toolbar and This Mac show the same capture status")
    func toolbarAndThisMacAgree() async {
        let harness = CaptureHarness()

        await harness.lifecycle.start()
        #expect(CaptureLifecycleControl.displayedStatus(lifecycle: harness.lifecycle) == "Listening")
        #expect(
            TransparentProxySectionView.displayedStatus(
                lifecycle: harness.lifecycle,
                model: harness.model
            ) == "Listening"
        )
        #expect(
            CaptureLifecycleControl.displayedStatus(lifecycle: harness.lifecycle)
                == TransparentProxySectionView.displayedStatus(
                    lifecycle: harness.lifecycle,
                    model: harness.model
                )
        )

        await harness.lifecycle.stop()
        #expect(CaptureLifecycleControl.displayedStatus(lifecycle: harness.lifecycle) == "Stopped")
        #expect(
            TransparentProxySectionView.displayedStatus(
                lifecycle: harness.lifecycle,
                model: harness.model
            ) == "Stopped"
        )
        #expect(harness.lifecycle.presentation.action == .start)
    }

    @Test("each capture phase has the status word the toolbar shows")
    func phaseWords() {
        #expect(CapturePhase.stopped.presentation.title == "Stopped")
        #expect(CapturePhase.stopped.presentation.action == .start)
        #expect(CapturePhase.starting.presentation.title == "Starting…")
        #expect(CapturePhase.starting.presentation.action == nil)
        #expect(CapturePhase.listening.presentation.title == "Listening")
        #expect(CapturePhase.listening.presentation.action == .stop)
        #expect(CapturePhase.stopping.presentation.title == "Stopping…")
        #expect(CapturePhase.stopping.presentation.action == nil)
        #expect(CapturePhase.needsApproval.presentation.title == "Needs Approval")
        #expect(CapturePhase.needsApproval.presentation.action == .stop)
        #expect(CapturePhase.needsTrust.presentation.title == "Needs CA Trust")
        #expect(CapturePhase.needsTrust.presentation.action == .stop)
        #expect(CapturePhase.failed("Ports are in use.").presentation.title == "Error")
        #expect(CapturePhase.failed("Ports are in use.").presentation.action == .start)
        #expect(CapturePhase.failed("Ports are in use.").presentation.help == "Ports are in use.")
    }

    @Test("the capture control and This Mac lay out")
    func draws() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        let control = ImageRenderer(
            content: CaptureLifecycleControl(lifecycle: harness.lifecycle).frame(width: 240)
        )
        control.scale = 1
        #expect((control.nsImage?.size.height ?? 0) > 0)

        let section = ImageRenderer(
            content: TransparentProxySectionView(
                model: harness.model,
                coreConnected: true,
                lifecycle: harness.lifecycle
            )
            .frame(width: 230)
        )
        section.scale = 1
        #expect((section.nsImage?.size.height ?? 0) > 0)
    }

    @Test("core readiness is asked before trust, and the proxy is claimed only after trust is ready")
    func trustFollowsReadiness() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.trustAfterInstall = .trusted
        let run = makeTrustLifecycle(store: store, path: fixture.pemPath)

        await run.lifecycle.start()

        let ready = run.trace.events.firstIndex(of: "ready")
        let claim = run.trace.events.firstIndex(of: "claim")
        #expect(ready != nil && claim != nil && ready! < claim!)
        #expect(store.installs == [fixture.der])
        #expect(run.proxies.claimPorts == [9_090])
        #expect(run.lifecycle.phase == .listening)
        #expect(run.core.isRunning)
    }

    @Test("a trusted CA may be claimed, and a trust failure leaves the proxy untouched")
    func trustFailureDoesNotClaimTheProxy() async throws {
        let trusted = try TrustTestCertificate.issue()
        defer { trusted.cleanup() }
        let trustedStore = RecordingTrustStore()
        trustedStore.table[trusted.der] = .trusted
        let trustedRun = makeTrustLifecycle(store: trustedStore, path: trusted.pemPath)
        await trustedRun.lifecycle.start()
        #expect(trustedStore.installs.isEmpty)
        #expect(trustedRun.proxies.claimPorts == [9_090])
        let ready = trustedRun.trace.events.firstIndex(of: "ready")
        let claim = trustedRun.trace.events.firstIndex(of: "claim")
        #expect(ready != nil && claim != nil && ready! < claim!)

        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }

        let cancelled = RecordingTrustStore()
        cancelled.result = .cancelled
        let cancelledRun = makeTrustLifecycle(store: cancelled, path: fixture.pemPath)
        await cancelledRun.lifecycle.start()
        #expect(cancelledRun.proxies.claimPorts.isEmpty)
        #expect(cancelledRun.core.isRunning)
        #expect(cancelledRun.lifecycle.statusText == CaptureTrustWording.cancelled)
        #expect(cancelledRun.lifecycle.statusDetail?.contains("not changed") == true)
        #expect(cancelledRun.lifecycle.presentation.action == .stop)

        let failed = RecordingTrustStore()
        failed.result = .failed
        let failedRun = makeTrustLifecycle(store: failed, path: fixture.pemPath)
        await failedRun.lifecycle.start()
        #expect(failedRun.proxies.claimPorts.isEmpty)
        #expect(failedRun.core.isRunning)
        #expect(failedRun.lifecycle.statusText == CaptureTrustWording.failed)

        let unverified = RecordingTrustStore()
        unverified.trustAfterInstall = .absent
        let unverifiedRun = makeTrustLifecycle(store: unverified, path: fixture.pemPath)
        await unverifiedRun.lifecycle.start()
        #expect(unverified.installs == [fixture.der])
        #expect(unverifiedRun.proxies.claimPorts.isEmpty)
        #expect(unverifiedRun.lifecycle.statusDetail?.contains("could not confirm") == true)
        #expect(unverifiedRun.core.isRunning)
    }

    private func makeTrustLifecycle(store: RecordingTrustStore, path: String) -> TrustLifecycleRun {
        let trace = CaptureTrace()
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let core = FakeCoreProcess(trace: trace, control: control)
        let readiness = FakeReadiness(outcome: .ready, trace: trace, control: control)
        let proxies = FakeSystemProxy()
        proxies.trace = trace
        let trust = MacOSCaptureTrust(
            certificateLocator: { path },
            store: store,
            authorizationPermitted: { true }
        )
        let lifecycle = CaptureLifecycle(
            core: core,
            ports: FakePorts(),
            readiness: readiness,
            transparent: TransparentProxyModel(control: control),
            proxies: proxies,
            trust: trust
        )
        trust.onActivity = { [weak lifecycle] text in
            lifecycle?.noteTrustActivity(text)
        }
        return TrustLifecycleRun(trace: trace, core: core, proxies: proxies, lifecycle: lifecycle)
    }

    @Test("a start records the stages that got it to Listening")
    func historyNamesTheStages() async {
        let harness = CaptureHarness()

        await harness.lifecycle.start()

        let stages = harness.lifecycle.history.map(\.stage)
        #expect(stages.contains("Stopped -> Starting"))
        #expect(stages.contains("core ready"))
        #expect(stages.contains("trust ready"))
        #expect(stages.contains("proxy claimed"))
        #expect(stages.contains("transparent starting"))
        #expect(stages.contains("Starting -> Listening"))
        let trace = harness.lifecycle.trace
        #expect(trace.stage == "Listening")
        #expect(trace.events == harness.lifecycle.history)
    }

    @Test("core readiness that never arrives is an error, not Starting")
    func readinessThatNeverArrivesIsBounded() async throws {
        let readiness = PausingReadiness()
        let harness = CaptureHarness(readinessWait: readiness)
        harness.lifecycle.readinessBound = .milliseconds(80)
        let started = ContinuousClock.now

        await harness.lifecycle.start()

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.phase != .starting)
        #expect(harness.lifecycle.statusDetail?.contains("did not become ready") == true)
        #expect(harness.proxies.claimPorts.isEmpty)
        readiness.gate.fire()
    }

    @Test("the core exiting during Start is an error")
    func coreExitDuringStartIsAnError() async throws {
        let readiness = PausingReadiness()
        let harness = CaptureHarness(readinessWait: readiness)
        let starting = Task { await harness.lifecycle.start() }
        try await waitUntil { readiness.entered }

        harness.core.crash()
        try await waitUntil { harness.lifecycle.phase != .starting }
        readiness.gate.fire()
        await starting.value

        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(!harness.lifecycle.coreIsRunning)
        #expect(harness.lifecycle.phase != .listening)
        #expect(harness.proxies.claimPorts.isEmpty)
    }

    @Test("Stop during Start cancels it and ignores the late result")
    func stopDuringStartReachesStopped() async throws {
        let readiness = PausingReadiness()
        readiness.outcome = .ready
        let harness = CaptureHarness(readinessWait: readiness)
        harness.lifecycle.readinessBound = .seconds(30)
        let starting = Task { await harness.lifecycle.start() }
        try await waitUntil { readiness.entered }

        let started = ContinuousClock.now
        await harness.lifecycle.stop()
        readiness.gate.fire()
        await starting.value

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(!harness.lifecycle.coreIsRunning)
        #expect(harness.lifecycle.canExit)
        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(harness.control.count(.turnOn(9_091)) == 0)
    }

    @Test("quit during Start shuts down inside a bound")
    func quitDuringStartIsBounded() async throws {
        let readiness = PausingReadiness()
        let harness = CaptureHarness(readinessWait: readiness)
        harness.lifecycle.readinessBound = .seconds(30)
        let starting = Task { await harness.lifecycle.start() }
        try await waitUntil { readiness.entered }

        let started = ContinuousClock.now
        await harness.lifecycle.quit()
        readiness.gate.fire()
        await starting.value

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
        #expect(!harness.lifecycle.coreIsRunning)
    }

    @Test("waiting for CA trust is Needs CA Trust, not Starting")
    func pendingTrustIsNotStarting() async throws {
        let trust = PausingTrust()
        let harness = CaptureHarness(trust: trust)
        trust.announce = { [weak lifecycle = harness.lifecycle] in
            lifecycle?.noteTrustActivity(CaptureTrustWording.needsTrust)
        }
        let starting = Task { await harness.lifecycle.start() }
        try await waitUntil { harness.lifecycle.phase == .needsTrust }

        #expect(harness.lifecycle.statusText == "Needs CA Trust")
        #expect(harness.lifecycle.presentation.action == .stop)
        #expect(harness.proxies.claimPorts.isEmpty)
        #expect(harness.lifecycle.phase != .starting)

        trust.gate.fire()
        await starting.value
        #expect(harness.lifecycle.phase == .listening)
    }

    @Test("a Turn On that never returns restores the proxy and stops the core")
    func tunnelStartThatNeverReturnsRollsBack() async throws {
        let harness = CaptureHarness()
        harness.control.hangTurnOn = true
        harness.lifecycle.tunnelOperationBound = .milliseconds(60)
        let started = ContinuousClock.now

        await harness.lifecycle.start()

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(!harness.lifecycle.coreIsRunning)
        #expect(!harness.proxies.hasClaim)
        let restore = harness.trace.events.firstIndex(of: "restore")
        let stopped = harness.trace.events.firstIndex(of: "core-stop")
        #expect(restore != nil && stopped != nil && restore! < stopped!)
        harness.control.turnOnError = NSError(domain: "NEVPNErrorDomain", code: 1)
        harness.control.finishTurnOn()
        try await Task.sleep(for: .milliseconds(30))
        #expect(harness.lifecycle.phase != .listening)
        #expect(!harness.lifecycle.coreIsRunning)
    }

    @Test("a restore that never verifies keeps the core and refuses quit")
    func restoreThatStaysClaimedRefusesQuit() async {
        let proxies = FakeSystemProxy()
        proxies.restoreReport = SystemProxyRestoreReport(
            restored: [],
            leftInPlace: [],
            vanished: [],
            stillClaimed: true,
            message: SystemProxyPlan.stillOwnedMessage(host: "127.0.0.1", port: 9_090)
        )
        let harness = CaptureHarness(proxies: proxies)
        await harness.lifecycle.start()

        await harness.lifecycle.quit()

        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(!harness.lifecycle.canExit)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(harness.lifecycle.requiresCleanupBeforeQuit)
    }

    @Test("Turn Off that never returns does not wait forever, and leaves the core while the tunnel is on")
    func tunnelStopThatNeverReturnsIsBounded() async throws {
        let harness = CaptureHarness()
        await harness.lifecycle.start()
        harness.control.hangTurnOff = true
        harness.lifecycle.turnOffCallBound = .milliseconds(40)
        harness.lifecycle.transparentStopBound = .milliseconds(40)
        let started = ContinuousClock.now

        await harness.lifecycle.stop()

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.transparent.isOn)
        #expect(harness.core.isRunning)
        #expect(harness.core.stopCount == 0)
        #expect(harness.lifecycle.presentation.title == "Error")
        #expect(!harness.lifecycle.canExit)
        harness.control.finishTurnOff()
        try await Task.sleep(for: .milliseconds(30))
        #expect(harness.core.isRunning)
    }

    @Test("two Starts are one launch")
    func twoStartsAreOneOperation() async {
        let harness = CaptureHarness()
        let first = Task { await harness.lifecycle.start() }
        let second = Task { await harness.lifecycle.start() }
        await first.value
        await second.value

        #expect(harness.core.launchCount == 1)
        #expect(harness.lifecycle.phase == .listening)
        #expect(harness.proxies.claimPorts == [9_090])
    }

    @Test("two Stops are one shutdown")
    func twoStopsAreOneOperation() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()

        let first = Task { await harness.lifecycle.stop() }
        let second = Task { await harness.lifecycle.stop() }
        await first.value
        await second.value

        #expect(harness.core.stopCount == 1)
        #expect(harness.proxies.restoreCount == 1)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
    }

    @Test("a delayed approval after Stop does not start capture")
    func delayedStartCallbackIsIgnored() async throws {
        let harness = CaptureHarness(
            extensionState: .notInstalled,
            activation: .success(.awaitingApproval)
        )
        await harness.lifecycle.start()
        #expect(harness.lifecycle.phase == .needsApproval)

        await harness.lifecycle.stop()
        harness.control.current.tunnel = .on
        harness.control.approve()
        try await Task.sleep(for: .milliseconds(30))

        #expect(harness.lifecycle.phase == .stopped)
        #expect(!harness.lifecycle.coreIsRunning)
        #expect(harness.control.count(.turnOn(9_091)) == 0)
    }

    @Test("automatic start and a manual start launch one core")
    func automaticAndManualStartAreOneLaunch() async {
        let harness = CaptureHarness()
        let automatic = Task { await harness.lifecycle.startOnLaunch() }
        let manual = Task { await harness.lifecycle.start() }
        await automatic.value
        await manual.value

        #expect(harness.core.launchCount == 1)
        #expect(harness.lifecycle.phase == .listening)

        await harness.lifecycle.stop()
        await harness.lifecycle.startAutomatically()
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.core.launchCount == 1)
    }

    @Test("quit from Listening finishes inside a bound")
    func quitFromListeningIsBounded() async {
        let harness = CaptureHarness()
        await harness.lifecycle.start()
        let started = ContinuousClock.now

        await harness.lifecycle.quit()

        #expect(ContinuousClock.now - started < wallClock)
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
        #expect(!harness.core.isRunning)
        let stages = harness.lifecycle.history.map(\.stage)
        #expect(stages.contains("Listening -> Stopping"))
        #expect(stages.contains("proxy restored"))
        #expect(stages.contains("transparent off"))
        #expect(stages.contains("core stopped"))
        #expect(stages.contains("Stopping -> Stopped"))
    }

    @Test("quit says it is restoring, and names the administrator dialog when one is required")
    func quitNamesTheRestore() async throws {
        let harness = CaptureHarness()
        await harness.lifecycle.start()
        harness.control.hangTurnOff = true
        harness.lifecycle.turnOffCallBound = .seconds(5)
        let quitting = Task { await harness.lifecycle.quit() }
        try await waitUntil {
            harness.lifecycle.statusText == CaptureLifecycle.quitRestoreActivity
        }
        try await waitUntil { harness.control.count(.turnOff) >= 1 }

        #expect(harness.lifecycle.phase == .stopping)
        harness.control.finishTurnOff()
        await quitting.value
        #expect(harness.lifecycle.canExit)

        let prompting = FakeSystemProxy()
        prompting.promptsForAuthorization = true
        let prompted = CaptureHarness(proxies: prompting)
        await prompted.lifecycle.start()
        await prompted.lifecycle.quit()
        #expect(prompted.lifecycle.history.map(\.stage).contains("quit needs authorization"))
        #expect(prompted.lifecycle.canExit)
    }

    @Test("quit with nothing owned does not wait on an administrator")
    func quitWithoutAClaimDoesNotAskForAuthorization() async {
        let proxies = FakeSystemProxy()
        proxies.promptsForAuthorization = true
        let harness = CaptureHarness(proxies: proxies)
        harness.trust.result = .notReady("ProxyLab cannot yet check whether this Mac trusts its certificate.")
        await harness.lifecycle.start()
        #expect(harness.proxies.claimPorts.isEmpty)

        await harness.lifecycle.quit()

        #expect(!harness.lifecycle.history.map(\.stage).contains("quit needs authorization"))
        #expect(harness.lifecycle.phase == .stopped)
        #expect(harness.lifecycle.canExit)
        #expect(!harness.core.isRunning)
    }

    /// Stopping is in the middle of cleanup. These tests wait for the phase
    /// cleanup actually ends in.
    private static func isTerminal(_ phase: CapturePhase) -> Bool {
        switch phase {
        case .failed, .stopped: true
        case .starting, .listening, .stopping, .needsApproval, .needsTrust: false
        }
    }

    private func waitUntil(_ condition: @MainActor () -> Bool) async throws {
        for _ in 0..<200 where !condition() {
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(condition())
    }
}

@MainActor
@Suite("Core readiness bounds")
struct CoreReadinessBoundTests {
    @Test("a socket that never answers ready is a timeout, not a hang")
    func missingSocketTimesOut() async {
        let readiness = IPCCoreReadiness(socketPath: "/tmp/proxylab-no-such-\(UUID().uuidString).sock")
        readiness.timeout = .milliseconds(200)
        readiness.poll = .milliseconds(20)
        readiness.probeLimit = .milliseconds(50)
        let started = ContinuousClock.now

        let outcome = await readiness.waitUntilReady(
            isProcessRunning: { true },
            isCancelled: { false }
        )

        #expect(outcome == .timedOut)
        #expect(ContinuousClock.now - started < wallClock)
    }

    @Test("a connection that accepts and never answers is still bounded")
    func acceptedAndSilentIsBounded() async throws {
        let server = try SilentUnixServer()
        defer { server.stop() }
        let readiness = IPCCoreReadiness(socketPath: server.path)
        readiness.timeout = .milliseconds(250)
        readiness.poll = .milliseconds(20)
        readiness.probeLimit = .milliseconds(60)
        let started = ContinuousClock.now

        let outcome = await readiness.waitUntilReady(
            isProcessRunning: { true },
            isCancelled: { false }
        )

        #expect(outcome == .timedOut)
        #expect(ContinuousClock.now - started < wallClock)
    }
}

/// Listens, accepts, and never reads. Closed by the test.
private final class SilentUnixServer: @unchecked Sendable {
    let path: String
    private let fd: Int32

    init() throws {
        let path = "/tmp/plr-\(getpid())-\(UInt32.random(in: 0...9999)).sock"
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw SilentUnixServerError.socket }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
            Darwin.close(fd)
            throw SilentUnixServerError.socket
        }
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: bytes)
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let bound = withUnsafePointer(to: &address) { stored in
            stored.withMemoryRebound(to: sockaddr.self, capacity: 1) { pointer in
                Darwin.bind(fd, pointer, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(fd, 8) == 0 else {
            Darwin.close(fd)
            unlink(path)
            throw SilentUnixServerError.socket
        }
        self.fd = fd
        self.path = path
        DispatchQueue.global().async {
            while true {
                let client = accept(fd, nil, nil)
                if client < 0 { return }
            }
        }
    }

    func stop() {
        Darwin.close(fd)
        unlink(path)
    }
}

private enum SilentUnixServerError: Error {
    case socket
}

@Suite("What a diagnostics answer means for capture")
struct CoreReadinessInterpretationTests {
    @Test("both listeners up is ready")
    func ready() {
        let outcome = IPCCoreReadiness.interpret(report(
            localhost: "listening",
            transparent: "listening"
        ))

        #expect(outcome == .ready)
    }

    @Test("a bridge that is unavailable is not a reason to wait or to capture")
    func bridgeUnavailable() {
        let outcome = IPCCoreReadiness.interpret(report(
            localhost: "listening",
            transparent: "unavailable",
            transparentMessage: "the transparent capture bridge is not listening"
        ))

        #expect(outcome == .bridgeUnavailable("the transparent capture bridge is not listening"))
    }

    @Test("an answer that is not finished yet is not a failure")
    func notYet() {
        #expect(IPCCoreReadiness.interpret(report(localhost: "unknown", transparent: "unknown")) == nil)
    }

    private func report(
        localhost: String,
        transparent: String,
        transparentMessage: String? = nil
    ) -> CoreDiagnostics {
        CoreDiagnostics(
            version: "0.1.0",
            core: "running",
            localhost: DiagnosticEndpoint(state: localhost, address: localhost == "listening" ? "127.0.0.1:9090" : nil),
            transparent: DiagnosticEndpoint(
                state: transparent,
                address: transparent == "listening" ? "127.0.0.1:9091" : nil,
                message: transparentMessage
            ),
            storage: DiagnosticComponent(state: "open"),
            ipc: DiagnosticComponent(state: "serving"),
            recent: []
        )
    }
}

@MainActor
@Suite("The bundled proxy engine")
struct BundledCoreExecutableTests {
    @Test("the engine lives in Contents/MacOS inside the app bundle")
    func location() {
        let url = BundledCoreExecutable.url(bundleURL: URL(fileURLWithPath: "/Applications/ProxyLab.app"))

        #expect(url.path == "/Applications/ProxyLab.app/Contents/MacOS/proxy-engine")
    }

    @Test("a missing engine is reported and not launched")
    func missing() async {
        let missing = URL(fileURLWithPath: "/tmp/proxylab-no-such-engine")
        let core = BundledCoreProcess(executable: missing)
        let control = FakeTransparentProxyControl(extensionState: .installed, tunnel: .off)
        let lifecycle = CaptureLifecycle(
            core: core,
            ports: FakePorts(),
            readiness: FakeReadiness(outcome: .ready),
            transparent: TransparentProxyModel(control: control),
            proxies: FakeSystemProxy(),
            trust: ScriptedCaptureTrust()
        )

        await lifecycle.start()

        #expect(lifecycle.presentation.title == "Error")
        #expect(lifecycle.statusDetail?.contains("could not find its proxy engine") == true)
        #expect(control.calls.isEmpty)
        #expect(!core.isRunning)
    }
}

@MainActor
@Suite("Owned core process")
struct OwnedCoreProcessTests {
    @Test("stop ends the process this object launched and leaves another sleep running")
    func stopOwnsOnlyItsProcess() async throws {
        let owned = BundledCoreProcess(
            executable: URL(fileURLWithPath: "/bin/sleep"),
            arguments: ["30"]
        )
        let other = Process()
        other.executableURL = URL(fileURLWithPath: "/bin/sleep")
        other.arguments = ["30"]
        try other.run()
        defer { if other.isRunning { other.terminate() } }

        try owned.launch()
        let ownedPID = owned.processIdentifier
        #expect(owned.isRunning)
        #expect(other.isRunning)
        #expect(ownedPID != other.processIdentifier)

        await owned.stop()

        #expect(!owned.isRunning)
        #expect(other.isRunning)
    }

    @Test("SIGINT that is ignored still ends on TERM or KILL, inside the bound")
    func ignoredInterruptIsStillBounded() async throws {
        let owned = BundledCoreProcess(
            executable: URL(fileURLWithPath: "/bin/bash"),
            arguments: ["-c", "trap '' INT; while true; do :; done"]
        )
        owned.gracefulTimeout = .milliseconds(200)
        owned.terminateTimeout = .milliseconds(500)
        owned.killTimeout = .milliseconds(500)
        try owned.launch()
        let pid = owned.processIdentifier
        let started = ContinuousClock.now

        await owned.stop()

        let elapsed = ContinuousClock.now - started
        #expect(!owned.isRunning)
        #expect(elapsed < .seconds(3))
        if let pid {
            #expect(Darwin.kill(pid, 0) != 0)
        }
    }
}

@MainActor
@Suite("Loopback port check")
struct LoopbackPortCheckTests {
    @Test("a port this test bound is occupied, and is free again once the socket is closed")
    func reportsTheBoundPort() throws {
        let descriptor = socket(AF_INET, SOCK_STREAM, 0)
        #expect(descriptor >= 0)
        var stillOpen = true
        defer {
            if stillOpen { close(descriptor) }
        }

        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr.s_addr = INADDR_LOOPBACK.bigEndian
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        #expect(bound == 0)
        #expect(listen(descriptor, 1) == 0)

        var name = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let named = withUnsafeMutablePointer(to: &name) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(descriptor, $0, &length)
            }
        }
        #expect(named == 0)
        let port = UInt16(bigEndian: name.sin_port)
        #expect(port != 0)

        #expect(LoopbackPortCheck(ports: [port]).occupiedPorts() == [port])
        close(descriptor)
        stillOpen = false
        #expect(LoopbackPortCheck(ports: [port]).occupiedPorts().isEmpty)
    }
}

private struct TrustLifecycleRun {
    let trace: CaptureTrace
    let core: FakeCoreProcess
    let proxies: FakeSystemProxy
    let lifecycle: CaptureLifecycle
}

@MainActor
private final class CaptureTrace {
    var events: [String] = []

    func add(_ event: String) {
        events.append(event)
    }
}

@MainActor
private struct CaptureHarness {
    let trace = CaptureTrace()
    let control: FakeTransparentProxyControl
    let core: FakeCoreProcess
    let readiness: FakeReadiness
    let proxies: FakeSystemProxy
    let trust: ScriptedCaptureTrust
    let model: TransparentProxyModel
    let lifecycle: CaptureLifecycle

    init(
        extensionState: ExtensionState = .installed,
        tunnel: TunnelState = .off,
        activation: Result<ActivationOutcome, TransparentProxyFailure> = .success(.installed),
        readiness outcome: CoreReadinessOutcome = .ready,
        occupied: [UInt16] = [],
        proxies: FakeSystemProxy = FakeSystemProxy(),
        readinessWait: (any CoreReadinessWaiting)? = nil,
        trust: (any CaptureTrustChecking)? = nil
    ) {
        let control = FakeTransparentProxyControl(extensionState: extensionState, tunnel: tunnel)
        control.activation = activation
        let core = FakeCoreProcess(trace: trace, control: control)
        let scriptedReadiness = FakeReadiness(outcome: outcome, trace: trace, control: control)
        let readiness = readinessWait ?? scriptedReadiness
        let model = TransparentProxyModel(control: control)
        proxies.trace = trace
        let scriptedTrust = ScriptedCaptureTrust()
        scriptedTrust.trace = trace
        let trust = trust ?? scriptedTrust
        let lifecycle = CaptureLifecycle(
            core: core,
            ports: FakePorts(occupied: occupied),
            readiness: readiness,
            transparent: model,
            proxies: proxies,
            trust: trust
        )
        self.control = control
        self.core = core
        self.readiness = scriptedReadiness
        self.proxies = proxies
        self.trust = scriptedTrust
        self.model = model
        self.lifecycle = lifecycle
    }
}

/// Readiness that waits until the test fires `gate`.
@MainActor
private final class PausingReadiness: CoreReadinessWaiting {
    let gate = WakeSignal()
    var outcome: CoreReadinessOutcome = .ready
    private(set) var entered = false

    func waitUntilReady(
        isProcessRunning: @escaping @MainActor () -> Bool,
        isCancelled: @escaping @MainActor () -> Bool
    ) async -> CoreReadinessOutcome {
        entered = true
        await gate.wait()
        if isCancelled() { return .cancelled }
        if !isProcessRunning() { return .coreExited }
        return outcome
    }
}

/// Trust that tells the lifecycle a person is deciding, then waits.
@MainActor
private final class PausingTrust: CaptureTrustChecking {
    let gate = WakeSignal()
    var announce: (@MainActor () -> Void)?

    func readiness() async -> CaptureTrustReadiness {
        announce?()
        await gate.wait()
        return .ready
    }
}

@MainActor
private final class ScriptedCaptureTrust: CaptureTrustChecking {
    var result: CaptureTrustReadiness = .ready
    var trace: CaptureTrace?

    func readiness() async -> CaptureTrustReadiness {
        trace?.add("trust")
        return result
    }
}

@MainActor
private final class FakePorts: CapturePortChecking {
    var occupied: [UInt16]

    init(occupied: [UInt16] = []) {
        self.occupied = occupied
    }

    func occupiedPorts() -> [UInt16] { occupied }
}

@MainActor
private final class FakeReadiness: CoreReadinessWaiting {
    var outcome: CoreReadinessOutcome
    private let trace: CaptureTrace?
    private let control: FakeTransparentProxyControl?
    private(set) var waitCount = 0
    private(set) var turnOnHadAlreadyHappened = false

    init(
        outcome: CoreReadinessOutcome,
        trace: CaptureTrace? = nil,
        control: FakeTransparentProxyControl? = nil
    ) {
        self.outcome = outcome
        self.trace = trace
        self.control = control
    }

    func waitUntilReady(
        isProcessRunning: @escaping @MainActor () -> Bool,
        isCancelled: @escaping @MainActor () -> Bool
    ) async -> CoreReadinessOutcome {
        waitCount += 1
        trace?.add("ready")
        if let control {
            turnOnHadAlreadyHappened = control.calls.contains { call in
                if case .turnOn = call { return true }
                return false
            }
        }
        if isCancelled() { return .cancelled }
        if !isProcessRunning() { return .coreExited }
        return outcome
    }
}

@MainActor
private final class FakeCoreProcess: CoreProcessControlling {
    var isRunning = false
    var onExit: (@MainActor () -> Void)?
    private(set) var launchCount = 0
    private(set) var stopCount = 0
    private(set) var sawTurnOffBeforeStop = false
    private let trace: CaptureTrace?
    private let control: FakeTransparentProxyControl?

    init(trace: CaptureTrace? = nil, control: FakeTransparentProxyControl? = nil) {
        self.trace = trace
        self.control = control
    }

    func launch() throws {
        if isRunning { return }
        launchCount += 1
        trace?.add("launch")
        isRunning = true
    }

    func stop() async {
        stopCount += 1
        if let control {
            sawTurnOffBeforeStop = control.calls.contains(.turnOff)
        }
        trace?.add("core-stop")
        isRunning = false
    }

    func crash() {
        guard isRunning else { return }
        isRunning = false
        onExit?()
    }
}

@MainActor
private final class FakeSystemProxy: SystemProxyControlling {
    var hasClaim = false
    var claimError: Error?
    var restoreError: Error?
    var restoreReport = SystemProxyRestoreReport.none
    var restoreResults: [SystemProxyRestoreReport] = []
    private(set) var claimPorts: [Int] = []
    private(set) var adoptCount = 0
    private(set) var restoreCount = 0
    var trace: CaptureTrace?
    var adoption = SystemProxyAdoption.none

    var promptsForAuthorization = false

    func authorizationNeedsPrompt() -> Bool { promptsForAuthorization }

    func claim(port: Int) throws {
        trace?.add("claim")
        if let claimError { throw claimError }
        claimPorts.append(port)
        hasClaim = true
    }

    func adoptActiveService() throws -> SystemProxyAdoption {
        adoptCount += 1
        trace?.add("adopt")
        return adoption
    }

    func restore() throws -> SystemProxyRestoreReport {
        restoreCount += 1
        trace?.add("restore")
        if let restoreError { throw restoreError }
        let report = restoreResults.isEmpty ? restoreReport : restoreResults.removeFirst()
        hasClaim = report.stillClaimed
        return report
    }
}
