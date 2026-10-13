//  App-owned capture, from launch to quit.
//
//      Start  ─▶ launch the bundled proxy-engine, unless this app already has
//                one running
//             ─▶ wait until diagnostics says 9090 and 9091 are listening
//             ─▶ if a previous session still owns the system proxy, put it
//                back while that listener is up. A restore that does not
//                land leaves the engine running
//             ─▶ certificate trust must report ready. Until it does, the
//                system proxy is not changed. A prompt is Needs CA Trust,
//                not Starting
//             ─▶ save the active service's HTTP and HTTPS proxy, then point
//                both at 127.0.0.1:9090
//             ─▶ turn transparent capture on
//             ─▶ Listening
//
//      Stop   ─▶ stop accepting a new Start
//             ─▶ put back the HTTP and HTTPS proxy ProxyLab saved
//             ─▶ read the live route. The engine stays up unless that read
//                shows the owned endpoint is gone
//             ─▶ turn transparent capture off
//             ─▶ stop the proxy-engine this app launched
//             ─▶ Stopped
//
//      Quit   ─▶ the same Stop. The application is not allowed to exit until
//                that verification has landed. Cancelling the administrator's
//                approval is not an exit
//
//  Every wait that is not a person deciding has a bound. Stop and quit do
//  not sit behind a wait that will never return: they invalidate the Start
//  that is in progress, and a callback from that Start cannot change the
//  phase after that. A prompt is Needs CA Trust or Needs Approval. Starting
//  means work is still moving.
//
//  Transparent capture is never left on while the core this app started is
//  gone. If the tunnel cannot be turned off, the core is left running and
//  the failure is shown: a dead core with the tunnel still on is the state
//  this refuses. A system call that has not started the tunnel is not that
//  state, and does not hold quit open.
//
//  An explicit Stop is remembered for this process only. Launching the app
//  starts capture once. A later refresh does not start it again after Stop
//  until the user presses Start. Nothing here is a preferences system.
//
//  Ports that are already taken are left alone. This object never signals a
//  process it did not launch.

import Foundation
import Observation
import os

/// The status string both the toolbar and This Mac display.
@MainActor
public enum CaptureSurfaces {
    public static func title(_ lifecycle: CaptureLifecycle) -> String {
        lifecycle.statusText
    }
}

/// One stage of Start, Stop, or quit, kept in memory for Diagnostics.
public struct CaptureLifecycleEvent: Equatable, Sendable, Identifiable {
    public let id: Int
    public let stage: String
    /// How long the previous stage had been current.
    public let milliseconds: Int

    public init(id: Int, stage: String, milliseconds: Int) {
        self.id = id
        self.stage = stage
        self.milliseconds = milliseconds
    }
}

/// Where capture is right now, and the stages that got it there.
public struct CaptureLifecycleTrace: Equatable, Sendable {
    public var stage: String
    public var stageMilliseconds: Int
    public var events: [CaptureLifecycleEvent]

    public init(stage: String, stageMilliseconds: Int, events: [CaptureLifecycleEvent]) {
        self.stage = stage
        self.stageMilliseconds = stageMilliseconds
        self.events = events
    }
}

/// Owns the bundled core and transparent capture as one lifecycle.
@MainActor
@Observable
public final class CaptureLifecycle {
    /// What both surfaces show.
    public private(set) var phase: CapturePhase = .stopped

    /// The transparent-capture model. The same instance the This Mac panel
    /// reads, so the panel and this lifecycle cannot describe two tunnels.
    public let transparent: TransparentProxyModel

    private let core: any CoreProcessControlling
    private let ports: any CapturePortChecking
    private let readiness: any CoreReadinessWaiting
    private let proxies: any SystemProxyControlling
    private let trust: any CaptureTrustChecking

    /// The last restore could not prove the live route had left ProxyLab.
    /// Quit stays blocked even when the ownership file is already gone.
    private var proxyUnreleased = false

    /// The system proxy was set or restored. The simulator panel reads the
    /// route again when this fires.
    public var onProxyChanged: (@MainActor () -> Void)?

    /// A proxy ProxyLab used to own was changed by someone else, and was left
    /// as they set it. Capture can still be listening.
    public private(set) var proxyNotice: String?

    /// The user pressed Stop (or quit) and has not pressed Start since.
    private var userStopped = false
    /// `startOnLaunch` has already been asked, so a view refresh does not
    /// start capture a second time.
    private var didBeginAutomaticStart = false

    /// Start and stop run one at a time. A new request waits for the one
    /// already in the queue, then runs only if it is still the latest.
    private var tail: Task<Void, Never>?
    private var activeStart: Task<Void, Never>?
    private var activeStop: Task<Void, Never>?

    /// Bumped by Stop, quit, and a core exit. A Start holds the value from
    /// when it began. A callback whose value no longer matches is stale.
    private var generation: UInt64 = 0
    /// The Start that may still promote a tunnel callback to Listening.
    ///
    /// Needs Approval returns from Start and then waits for macOS. Stop
    /// clears this, so that later approval cannot turn the tunnel into
    /// Listening.
    private var acceptTunnelCallback = false
    /// Set while `trust.readiness` is the Start this generation is in.
    private var trustGeneration: UInt64?
    /// Quit is the Stop now running. The toolbar says so.
    private var quitting = false
    /// A core-exit cleanup is already running.
    private var recoveringExit = false

    /// Fires the Start wait that is in progress. Replaced afterwards so the
    /// next Start is not born already cancelled.
    private var interrupt = WakeSignal()

    /// Recent stages. Newest last. Diagnostics reads this; nothing writes it
    /// to disk.
    public private(set) var history: [CaptureLifecycleEvent] = []
    private var nextEventID = 0
    private var stageStarted = ContinuousClock.now

    /// How long Stop waits for the tunnel to report off after asking.
    var transparentStopBound: Duration = .seconds(5)
    /// How long Start waits for the tunnel to report connected.
    var tunnelStartBound: Duration = .seconds(8)
    /// Last bound on the readiness check. The check itself returns earlier
    /// when it is cancelled.
    var readinessBound: Duration = .seconds(21)
    /// A person deciding is not this bound. Stop ends the wait; the bound
    /// is only the backstop.
    var trustBound: Duration = .seconds(60 * 30)
    /// How long Start waits for Turn On to return.
    var tunnelOperationBound: Duration = .seconds(20)
    /// How long Stop waits for Turn Off to return.
    var turnOffCallBound: Duration = .seconds(20)

    /// Toolbar sentence while quit is putting the proxy back.
    public static let quitRestoreActivity = "Restoring network settings before quitting…"
    /// Toolbar sentence while quit is waiting for the administrator dialog.
    public static let quitAuthorizationActivity =
        "ProxyLab must restore this Mac's proxy before quitting."

    private static let log = Logger(subsystem: "com.proxylab.ProxyLab", category: "capture-lifecycle")
    private static let historyLimit = 32

    public init(
        core: any CoreProcessControlling,
        ports: any CapturePortChecking,
        readiness: any CoreReadinessWaiting,
        transparent: TransparentProxyModel,
        proxies: any SystemProxyControlling,
        trust: any CaptureTrustChecking
    ) {
        self.core = core
        self.ports = ports
        self.readiness = readiness
        self.transparent = transparent
        self.proxies = proxies
        self.trust = trust

        core.onExit = { [weak self] in
            guard let self else { return }
            Task { await self.handleUnexpectedExit() }
        }
        transparent.onSettled = { [weak self] in
            self?.adoptTransparentState()
        }
        transparent.shouldStartTunnel = { [weak self] in
            guard let self else { return false }
            return !self.userStopped && self.activeStop == nil
        }
    }

    /// Whether the core this lifecycle launched is running.
    public var coreIsRunning: Bool { core.isRunning }

    /// Whether quit has nothing left to turn off or stop.
    public var requiresCleanupBeforeQuit: Bool {
        !canExit
            || phase == .starting
            || phase == .listening
            || phase == .stopping
            || phase == .needsApproval
            || phase == .needsTrust
    }

    /// Whether the process may exit: the tunnel is off, the owned core is
    /// gone, and the live proxy route is not still ProxyLab's.
    public var canExit: Bool {
        !transparent.isOn && !core.isRunning && !proxies.hasClaim && !proxyUnreleased
    }

    /// A trust step's own word, while that step is what the user is waiting
    /// on. Nil leaves the phase's title in place.
    public private(set) var trustActivity: String?

    /// A lifecycle sentence that replaces the phase title: quitting, or the
    /// administrator dialog. Nil leaves trust or the phase title in place.
    public private(set) var activityText: String?

    /// The stages Diagnostics shows, including how long the current one has
    /// been current.
    public var trace: CaptureLifecycleTrace {
        CaptureLifecycleTrace(
            stage: statusText,
            stageMilliseconds: Self.milliseconds(ContinuousClock.now - stageStarted),
            events: history
        )
    }

    /// The word on the toolbar and on This Mac.
    public var statusText: String {
        if let activityText { return activityText }
        if phase == .needsTrust {
            if trustActivity == CaptureTrustWording.installing { return CaptureTrustWording.installing }
            return phase.presentation.title
        }
        return trustActivity ?? presentation.title
    }

    /// Whether a trust prompt may be shown. False after Stop or quit has
    /// been asked, so a panel is not raised on the way out.
    public var acceptsTrustPrompt: Bool { !userStopped && activeStop == nil }

    /// Sets the trust step's word, or clears it when `text` is nil.
    ///
    /// A prompt becomes Needs CA Trust. Checking the file stays Starting.
    /// A callback from a Start that Stop already replaced is ignored.
    public func noteTrustActivity(_ text: String?) {
        guard let trustGeneration, trustGeneration == generation else { return }
        trustActivity = text
        switch text {
        case CaptureTrustWording.needsTrust, CaptureTrustWording.installing:
            if phase == .starting || phase == .needsTrust {
                setPhase(.needsTrust)
            }
        case CaptureTrustWording.checking:
            if phase == .needsTrust {
                setPhase(.starting)
            }
        default:
            break
        }
    }

    /// The error sentence, when the phase has one.
    public var statusDetail: String? { phase.detail }

    /// The control both surfaces draw.
    ///
    /// A failure that left the owned core running offers Stop, so that core
    /// can be shut down. Start is what a fully stopped capture offers.
    public var presentation: CaptureControlPresentation {
        var presentation = phase.presentation
        if case .failed = phase, core.isRunning {
            presentation.action = .stop
        }
        return presentation
    }

    /// Runs `action`.
    public func perform(_ action: CaptureControlAction) async {
        switch action {
        case .start: await start()
        case .stop: await stop()
        }
    }

    /// Starts capture once per process, unless the user has already pressed Stop.
    public func startOnLaunch() async {
        guard !didBeginAutomaticStart else { return }
        didBeginAutomaticStart = true
        await startAutomatically()
    }

    /// Starts capture unless the user pressed Stop and has not pressed Start.
    ///
    /// What a reconnect or a repeated launch task calls. It does not clear a
    /// Stop.
    public func startAutomatically() async {
        guard !userStopped else { return }
        await start()
    }

    /// Starts capture. Clears a Stop from this session.
    ///
    /// A second Start while one is already running waits for that one. It
    /// does not launch a second core.
    public func start() async {
        if let activeStart {
            await activeStart.value
            return
        }

        userStopped = false
        let token = nextGeneration()
        let previous = tail
        let task = Task { @MainActor in
            await previous?.value
            guard self.generation == token, !self.userStopped else { return }
            await self.performStart(token)
        }
        activeStart = task
        tail = task
        await task.value
        if activeStart == task { activeStart = nil }
    }

    /// Stops capture and remembers that the user asked for it.
    public func stop() async {
        userStopped = true
        if activeStart == nil, activeStop == nil, phase == .stopped, canExit {
            return
        }
        await halt(quitting: false)
    }

    /// The same shutdown as Stop, used when the application is quitting.
    ///
    /// The network extension stays installed. Turning the tunnel off is not
    /// removing it. The toolbar says the proxy is being put back. A restore
    /// that does not land leaves the app open and the core running.
    public func quit() async {
        userStopped = true
        await halt(quitting: true)
    }

    // MARK: - Queue

    /// Stop or quit. Invalidates the Start that is in progress, then runs
    /// one shutdown. A second call waits for that shutdown.
    private func halt(quitting: Bool) async {
        userStopped = true
        acceptTunnelCallback = false
        trustGeneration = nil
        transparent.cancelPendingStart()
        _ = nextGeneration()
        cancelWaits()
        if quitting {
            self.quitting = true
            activityText = Self.quitRestoreActivity
        }
        if phase != .stopping {
            setPhase(.stopping)
        }

        if let activeStop {
            await activeStop.value
            return
        }

        let previous = tail
        let task = Task { @MainActor in
            await previous?.value
            await self.performStop()
        }
        activeStop = task
        tail = task
        await task.value
        if activeStop == task { activeStop = nil }
        if quitting, activeStop == nil {
            self.quitting = false
        }
    }

    private func nextGeneration() -> UInt64 {
        generation += 1
        return generation
    }

    private func isCurrent(_ token: UInt64) -> Bool {
        generation == token && !userStopped && !recoveringExit
    }

    private func cancelWaits() {
        let current = interrupt
        interrupt = WakeSignal()
        current.fire()
    }

    // MARK: - Start

    private func performStart(_ token: UInt64) async {
        guard isCurrent(token) else { return }
        if phase == .listening, core.isRunning, transparent.isOn { return }
        // One activation request is enough. Pressing Start again while macOS
        // is still asking for approval would submit another.
        if phase == .needsApproval, core.isRunning { return }
        if phase == .needsTrust, core.isRunning { return }

        acceptTunnelCallback = false
        trustActivity = nil
        activityText = nil
        setPhase(.starting)

        // The listener comes up before the system proxy is touched. A leftover
        // claim is restored only after that, so a restore that cannot land
        // still has something accepting 127.0.0.1:9090.
        if !core.isRunning {
            let occupied = ports.occupiedPorts()
            if !occupied.isEmpty {
                if proxies.hasClaim || proxyUnreleased {
                    let restored = await restoreProxy()
                    guard isCurrent(token) else { return }
                    if restored.stillClaimed {
                        let message = restored.message ?? Self.portInUseMessage(occupied)
                        Self.log.error("\(message, privacy: .public)")
                        setPhase(.failed(message))
                        return
                    }
                }
                let message = Self.portInUseMessage(occupied)
                Self.log.error("\(message, privacy: .public)")
                setPhase(.failed(message))
                return
            }

            do {
                try core.launch()
            } catch {
                let message = (error as? LocalizedError)?.errorDescription
                    ?? "ProxyLab could not start its proxy engine."
                Self.log.error("\(message, privacy: .public)")
                guard isCurrent(token) else { return }
                if proxies.hasClaim || proxyUnreleased {
                    await releaseOwnedProxyIfListenerIsDown(message)
                } else {
                    setPhase(.failed(message))
                }
                return
            }
        }

        guard isCurrent(token) else { return }

        let readinessWake = interrupt
        let outcome = await LifecycleBound.value(limit: readinessBound, wake: readinessWake) {
            await self.readiness.waitUntilReady(
                isProcessRunning: { self.core.isRunning },
                isCancelled: { !self.isCurrent(token) }
            )
        }

        guard isCurrent(token) else { return }

        switch outcome ?? .timedOut {
        case .ready:
            record("core ready")
        case .cancelled:
            return
        case .coreExited:
            await recoverFromCoreLoss(
                "The ProxyLab core stopped before it was ready. Capture was not started."
            )
            return
        case .timedOut:
            await releaseOwnedProxyIfListenerIsDown(
                "The ProxyLab core did not become ready. Capture was not started."
            )
            return
        case let .bridgeUnavailable(message):
            await releaseOwnedProxyIfListenerIsDown("\(message) Capture was not started.")
            return
        }

        guard isCurrent(token), core.isRunning else {
            await recoverFromCoreLoss(
                "The ProxyLab core stopped before it was ready. Capture was not started."
            )
            return
        }

        if proxies.hasClaim || proxyUnreleased {
            let restored = await restoreProxy()
            guard isCurrent(token) else { return }
            if restored.stillClaimed {
                let message = restored.message
                    ?? SystemProxyPlan.stillOwnedMessage(
                        host: SystemProxyPlan.ownedHost,
                        port: CoreLocations.proxyPort
                    )
                Self.log.error("\(message, privacy: .public)")
                setPhase(.failed(message))
                return
            }
        }
        guard isCurrent(token) else { return }

        trustGeneration = token
        let trustWake = interrupt
        let trustResult = await LifecycleBound.value(limit: trustBound, wake: trustWake) {
            await self.trust.readiness()
        }
        if trustGeneration == token { trustGeneration = nil }
        guard isCurrent(token) else { return }

        switch trustResult {
        case .ready:
            trustActivity = nil
            if phase == .needsTrust { setPhase(.starting) }
            record("trust ready")
        case let .notReady(message):
            Self.log.error("\(message, privacy: .public)")
            setPhase(.failed(message))
            return
        case nil:
            let message = phase == .needsTrust
                ? "Certificate trust did not finish. The system HTTP and HTTPS proxy was not changed."
                : "ProxyLab could not finish checking its certificate. The system HTTP and HTTPS proxy was not changed."
            Self.log.error("\(message, privacy: .public)")
            setPhase(.failed(message))
            return
        }
        guard isCurrent(token) else { return }

        // The listener is up and trust has reported ready. Only now is the
        // system proxy pointed at it.
        guard await claimProxy() else { return }
        guard isCurrent(token) else { return }
        record("proxy claimed")

        if transparent.readiness != .ready {
            // The explicit proxy is already claimed and the core stays up.
            // This build cannot turn the tunnel on; Stop is what puts the
            // proxy back.
            setPhase(.failed(
                transparent.readiness.message
                    ?? "Transparent capture is unavailable in this build."
            ))
            return
        }

        acceptTunnelCallback = true
        record("transparent starting")
        let turnWake = interrupt
        let turnedOn: Void? = await LifecycleBound.value(limit: tunnelOperationBound, wake: turnWake) {
            await self.transparent.turnOn()
        }
        guard isCurrent(token) else { return }

        if turnedOn == nil {
            acceptTunnelCallback = false
            transparent.cancelPendingStart()
            await rollbackOwnedStart(
                transparent.lastError
                    ?? transparent.notice
                    ?? "Transparent capture did not start."
            )
            return
        }

        if transparent.snapshot?.extensionState == .awaitingApproval {
            setPhase(.needsApproval)
            return
        }

        if transparent.snapshot?.extensionState == .willCompleteAfterReboot {
            acceptTunnelCallback = false
            setPhase(.failed(
                transparent.notice
                    ?? "Restart this Mac to finish installing the extension, then start capture again."
            ))
            return
        }

        if transparent.snapshot?.tunnel != .on {
            let connected = await waitUntilTunnelConnected(token)
            guard isCurrent(token) else { return }
            if !connected {
                acceptTunnelCallback = false
                if transparent.snapshot?.extensionState == .awaitingApproval {
                    setPhase(.needsApproval)
                    acceptTunnelCallback = true
                    return
                }
                if core.isRunning {
                    await rollbackOwnedStart(
                        transparent.lastError
                            ?? transparent.notice
                            ?? "Transparent capture did not start."
                    )
                } else {
                    await recoverFromCoreLoss(
                        "The ProxyLab core stopped while capture was starting."
                    )
                }
                return
            }
        }

        guard core.isRunning else {
            await recoverFromCoreLoss("The ProxyLab core stopped while capture was starting.")
            return
        }

        guard transparent.snapshot?.tunnel == .on else {
            acceptTunnelCallback = false
            await rollbackOwnedStart(transparent.lastError ?? "Transparent capture did not start.")
            return
        }

        acceptTunnelCallback = false
        setPhase(.listening)
    }

    /// The listener never became ready. A proxy ProxyLab still owns is put
    /// back before the owned engine is stopped. A restore that does not land
    /// leaves that engine running.
    private func releaseOwnedProxyIfListenerIsDown(_ message: String) async {
        guard proxies.hasClaim || proxyUnreleased else {
            setPhase(.failed(message))
            return
        }
        let restored = await restoreProxy()
        if restored.stillClaimed {
            let shown = restored.message ?? message
            Self.log.error("\(shown, privacy: .public)")
            setPhase(.failed(shown))
            return
        }
        await stopOwnedCore()
        setPhase(.failed(message))
    }

    /// Start claimed the proxy and then could not finish. The engine stops
    /// only after the live route no longer points at it.
    private func rollbackOwnedStart(_ message: String) async {
        let restored = await restoreProxy()
        if restored.stillClaimed {
            let shown = restored.message ?? message
            Self.log.error("\(shown, privacy: .public)")
            setPhase(.failed(shown))
            return
        }
        await ensureTransparentOff()
        if transparent.isOn {
            let shown = "Transparent capture is still on. The proxy was left running so traffic is not sent to a stopped proxy."
            Self.log.error("\(shown, privacy: .public)")
            setPhase(.failed(shown))
            return
        }
        await stopOwnedCore()
        setPhase(.failed(message))
    }

    /// Snapshot reads until the tunnel is connected, the Start is replaced,
    /// or the bound passes. The snapshot is the decision.
    private func waitUntilTunnelConnected(_ token: UInt64) async -> Bool {
        let deadline = ContinuousClock.now.advanced(by: tunnelStartBound)
        while ContinuousClock.now < deadline {
            if !isCurrent(token) || !core.isRunning { return false }
            if transparent.snapshot?.extensionState == .awaitingApproval { return false }
            if transparent.snapshot?.tunnel == .on { return true }
            let wake = interrupt
            _ = await LifecycleBound.value(limit: .seconds(2), wake: wake) {
                await self.transparent.refreshConfiguration()
            }
            if !isCurrent(token) { return false }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return transparent.snapshot?.tunnel == .on
    }

    // MARK: - Stop

    private func performStop() async {
        trustActivity = nil
        if phase != .stopping {
            setPhase(.stopping)
        }
        // Before the core is stopped, and before the tunnel is asked to go
        // away: a restore that does not land must leave the core running,
        // because the system proxy would otherwise point at nothing.
        let restored = await restoreProxy()
        if restored.stillClaimed {
            let message = restored.message
                ?? "ProxyLab could not put the previous HTTP and HTTPS proxy back. The proxy was left running."
            Self.log.error("\(message, privacy: .public)")
            activityText = nil
            setPhase(.failed(message))
            return
        }
        record("proxy restored")
        if quitting {
            activityText = Self.quitRestoreActivity
        }

        await ensureTransparentOff()

        if transparent.isOn {
            let message = "Transparent capture is still on. The proxy was left running so traffic is not sent to a stopped proxy."
            Self.log.error("\(message, privacy: .public)")
            activityText = nil
            setPhase(.failed(message))
            return
        }
        record("transparent off")

        await stopOwnedCore()
        record("core stopped")
        activityText = nil
        if let message = restored.message {
            setPhase(.failed(message))
        } else {
            proxyNotice = nil
            setPhase(.stopped)
        }
    }

    /// Turns the tunnel off and waits, bounded, for the system to agree.
    ///
    /// An unsigned build has no tunnel to turn off and is not asked. A Turn
    /// Off that never returns is not left running: the snapshot after the
    /// bound is what decides whether the core may stop.
    private func ensureTransparentOff() async {
        guard transparent.readiness == .ready else { return }

        let finished: Void? = await LifecycleBound.value(limit: turnOffCallBound) {
            await self.transparent.turnOff()
        }
        if finished == nil {
            transparent.cancelPendingStart()
        }
        if !transparent.isOn { return }

        let deadline = ContinuousClock.now.advanced(by: transparentStopBound)
        while ContinuousClock.now < deadline {
            if !transparent.isOn { return }
            _ = await LifecycleBound.value(limit: .seconds(2)) {
                await self.transparent.refreshConfiguration()
            }
            if !transparent.isOn { return }
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    private func stopOwnedCore() async {
        guard core.isRunning else { return }
        await core.stop()
    }

    // MARK: - Unexpected exit

    /// The active network service may have changed. A newly primary service
    /// is given ProxyLab's proxy. A service that no longer has it is left
    /// alone and said so.
    public func activeNetworkChanged() {
        guard proxies.hasClaim else { return }
        switch phase {
        case .starting, .listening, .needsApproval, .needsTrust, .failed:
            break
        case .stopped, .stopping:
            return
        }

        do {
            let adoption = try proxies.adoptActiveService()
            if let message = SystemProxyPlan.externalChangeMessage(adoption.ownershipLost) {
                proxyNotice = message
                Self.log.error("\(message, privacy: .public)")
            } else if !adoption.claimed.isEmpty {
                proxyNotice = nil
            }
            onProxyChanged?()
        } catch {
            let message = (error as? LocalizedError)?.errorDescription
                ?? "ProxyLab could not update the HTTP and HTTPS proxy for the active network service."
            proxyNotice = message
            Self.log.error("\(message, privacy: .public)")
        }
    }

    /// The owned core ended on its own. Restore immediately. The engine is
    /// already gone, so a restore that does not land is a critical error and
    /// quit stays blocked.
    private func handleUnexpectedExit() async {
        await recoverFromCoreLoss(
            "The ProxyLab core stopped. The previous HTTP and HTTPS proxy was restored. Transparent capture was turned off so traffic is not sent to it."
        )
    }

    /// One cleanup for a core that is already gone. A second caller, including
    /// the Start that just observed the same exit, does not restore twice.
    private func recoverFromCoreLoss(_ message: String) async {
        if recoveringExit { return }
        if case .stopping = phase { return }
        if case .stopped = phase { return }
        recoveringExit = true
        acceptTunnelCallback = false
        trustGeneration = nil
        _ = nextGeneration()
        cancelWaits()
        await coreDiedWhileOwned(message)
        recoveringExit = false
    }

    private func coreDiedWhileOwned(_ restoredMessage: String) async {
        Self.log.error("The ProxyLab core stopped unexpectedly.")
        if phase != .stopping {
            setPhase(.stopping)
        }
        let restored = await restoreProxy()
        await ensureTransparentOff()

        if transparent.isOn || restored.stillClaimed {
            let message = restored.message
                ?? "The ProxyLab core stopped, and this Mac is still sending HTTP and HTTPS through ProxyLab at 127.0.0.1:9090. Restoration could not be completed."
            Self.log.error("\(message, privacy: .public)")
            activityText = nil
            setPhase(.failed(message))
            return
        }

        activityText = nil
        setPhase(.failed(restored.message ?? restoredMessage))
    }

    /// Points the active service at the explicit listener. False when that
    /// did not happen; `phase` is already the failure.
    private func claimProxy() async -> Bool {
        if proxies.authorizationNeedsPrompt() {
            activityText = "Waiting for approval to set the HTTP and HTTPS proxy…"
            record("proxy authorization")
            await Task.yield()
        }
        do {
            try proxies.claim(port: CoreLocations.proxyPort)
            if activityText != nil, !quitting { activityText = nil }
            proxyUnreleased = false
            proxyNotice = nil
            onProxyChanged?()
            return true
        } catch let error as SystemProxyError {
            if case .ownedWithoutBaseline = error {
                proxyUnreleased = true
            }
            let message = error.errorDescription
                ?? "ProxyLab could not set this Mac's HTTP and HTTPS proxy."
            Self.log.error("\(message, privacy: .public)")
            setPhase(.failed(message))
            return false
        } catch {
            let message = (error as? LocalizedError)?.errorDescription
                ?? "ProxyLab could not set this Mac's HTTP and HTTPS proxy."
            Self.log.error("\(message, privacy: .public)")
            setPhase(.failed(message))
            return false
        }
    }

    private func restoreProxy() async -> SystemProxyRestoreReport {
        // A prompt is only required when a setting still has to be written.
        // Quit with nothing owned must not say it is waiting on an administrator.
        if (proxies.hasClaim || proxyUnreleased), proxies.authorizationNeedsPrompt() {
            activityText = quitting
                ? Self.quitAuthorizationActivity
                : "Waiting for approval to change the HTTP and HTTPS proxy…"
            record(quitting ? "quit needs authorization" : "proxy authorization")
            await Task.yield()
        }
        return restoreProxyNow()
    }

    private func restoreProxyNow() -> SystemProxyRestoreReport {
        do {
            let report = try proxies.restore()
            proxyUnreleased = report.stillClaimed
            onProxyChanged?()
            return report
        } catch let error as SystemProxyError {
            proxyUnreleased = true
            let message = Self.restoreFailureMessage(error)
            Self.log.error("\(message, privacy: .public)")
            return SystemProxyRestoreReport(
                restored: [],
                leftInPlace: [],
                vanished: [],
                stillClaimed: true,
                message: message
            )
        } catch {
            proxyUnreleased = true
            let message = SystemProxyPlan.unverifiedMessage(
                host: SystemProxyPlan.ownedHost,
                port: CoreLocations.proxyPort
            )
            Self.log.error("\(message, privacy: .public)")
            return SystemProxyRestoreReport(
                restored: [],
                leftInPlace: [],
                vanished: [],
                stillClaimed: true,
                message: message
            )
        }
    }

    private static func restoreFailureMessage(_ error: SystemProxyError) -> String {
        switch error {
        case .authorizationCancelled, .authorizationDenied:
            SystemProxyPlan.authorizationBlockedMessage(
                host: SystemProxyPlan.ownedHost,
                port: CoreLocations.proxyPort
            )
        case .unreadable:
            SystemProxyPlan.unverifiedMessage(
                host: SystemProxyPlan.ownedHost,
                port: CoreLocations.proxyPort
            )
        default:
            error.errorDescription
                ?? SystemProxyPlan.stillOwnedMessage(
                    host: SystemProxyPlan.ownedHost,
                    port: CoreLocations.proxyPort
                )
        }
    }

    /// A Turn On that was waiting for approval finished, or the system dropped
    /// the tunnel. Listening is only claimed when both halves are actually up.
    private func adoptTransparentState() {
        switch phase {
        case .needsApproval, .starting:
            guard acceptTunnelCallback, !userStopped, activeStop == nil else { return }
            if transparent.snapshot?.tunnel == .on, core.isRunning {
                acceptTunnelCallback = false
                setPhase(.listening)
            }
        case .listening:
            guard activeStop == nil, !userStopped else { return }
            if transparent.snapshot?.tunnel != .on, transparent.snapshot?.tunnel != .starting {
                setPhase(.failed("Transparent capture stopped."))
            }
        case .stopped, .stopping, .failed, .needsTrust:
            break
        }
    }

    // MARK: - Diagnostics

    private func setPhase(_ newPhase: CapturePhase) {
        guard phase != newPhase else { return }
        let previous = Self.label(phase)
        phase = newPhase
        if newPhase != .stopping && newPhase != .starting && newPhase != .needsTrust {
            activityText = nil
        }
        record("\(previous) -> \(Self.label(newPhase))")
    }

    private func record(_ stage: String) {
        let now = ContinuousClock.now
        let elapsed = Self.milliseconds(now - stageStarted)
        stageStarted = now
        history.append(CaptureLifecycleEvent(id: nextEventID, stage: stage, milliseconds: elapsed))
        nextEventID += 1
        if history.count > Self.historyLimit {
            history.removeFirst(history.count - Self.historyLimit)
        }
        Self.log.notice("\(stage, privacy: .public) (\(elapsed)ms)")
    }

    private static func label(_ phase: CapturePhase) -> String {
        switch phase {
        case .stopped: "Stopped"
        case .starting: "Starting"
        case .listening: "Listening"
        case .stopping: "Stopping"
        case .needsApproval: "Needs Approval"
        case .needsTrust: "Needs CA Trust"
        case .failed: "Error"
        }
    }

    private static func milliseconds(_ duration: Duration) -> Int {
        let parts = duration.components
        let value = parts.seconds * 1_000 + parts.attoseconds / 1_000_000_000_000_000
        return Int(value)
    }

    /// What an occupied port is told as. Names the ports and says the other
    /// process was left running.
    static func portInUseMessage(_ ports: [UInt16]) -> String {
        let numbers = ports.map(String.init)
        let list = numbers.count == 1
            ? numbers[0]
            : numbers.dropLast().joined(separator: ", ") + " and " + (numbers.last ?? "")
        let noun = ports.count == 1 ? "Port \(list) is" : "Ports \(list) are"
        return "\(noun) already in use. ProxyLab left the other process running and did not start capture."
    }
}
