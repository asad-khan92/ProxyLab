//  Capturing this Mac's own traffic without a proxy setting.
//
//      Turn On ─▶ install the extension, unless this build's version is
//                 already enabled ─▶ (approve it in System Settings)
//                                 ─▶ switch the configuration on
//
//      Turn Off ─▶ switch it off
//
//  # This model changes the system only when it is asked
//
//  Nothing here reads a preference or turns anything on by itself. The app's
//  capture lifecycle is what asks, including on launch. Refreshing reads what
//  macOS says and changes nothing. The one thing that continues on its own is
//  a Turn On that stopped to wait for approval: once macOS says the extension
//  is allowed, that Turn On finishes — in this session, and never in a later
//  one, and not after startup was cancelled.
//
//  # The system decides, and this shows what it decided
//
//  Nothing on screen says "on" until the system's configuration says it is
//  connected. A switch that says on is a configuration that is carrying
//  traffic, and never an intention.

import Foundation
import Observation
import os
import TransparentProxyKit

/// Everything the Network Extension section shows.
@MainActor
@Observable
public final class TransparentProxyModel {
    /// What the system said last.
    public private(set) var snapshot: TransparentProxySnapshot?
    /// What the provider said last, while it was running.
    public private(set) var providerStatus: ProviderStatus?
    /// Whether a switch is in flight.
    public private(set) var isWorking = false
    /// The last thing that went wrong, as something to do about it.
    public private(set) var lastError: String?
    /// The `NSError` behind ``lastError``: domain, code, the system's
    /// description, and an underlying error when there is one. The panel
    /// shows ``lastError``. Diagnostics and the log show this.
    public private(set) var lastErrorDetail: String?
    /// Something to do that is not an error: approving the extension, or
    /// restarting the Mac.
    public private(set) var notice: String?

    private let control: any TransparentProxyControl
    private let bridgePort: UInt16
    private static let log = Logger(subsystem: "com.proxylab.ProxyLab", category: "transparent-proxy")

    /// Turn On was pressed and is waiting for the extension to be approved.
    private var finishWhenApproved = false
    /// Stop was asked while Turn On was still inside a system call.
    private var turnOffRequested = false
    /// Bumped when a newer Turn On or Turn Off replaces one that is in flight.
    ///
    /// The replaced call may still be inside a system callback. It must not
    /// publish a snapshot, or clear `isWorking`, after the newer call has
    /// taken over.
    private var operationTicket: UInt64 = 0
    /// How long Turn Off waits for an in-flight Turn On that has already
    /// connected the tunnel.
    ///
    /// A Turn On that has not connected anything is abandoned instead. Waiting
    /// for that system call would hold Stop and quit open for a tunnel that
    /// is not carrying traffic. Past this bound, a tunnel that is already on
    /// is left on: stopping the core under it is the failure this refuses.
    var turnOffWaitBound: Duration = .seconds(8)

    /// Called when a turn on, turn off, or system change has settled.
    ///
    /// The capture lifecycle uses this to notice an approval that finished
    /// after Turn On returned to wait, without polling.
    public var onSettled: (@MainActor () -> Void)?

    /// Asked immediately before the tunnel is started.
    ///
    /// `nil` means yes, which is what a caller that is not the capture
    /// lifecycle wants. The lifecycle returns no after Stop, so an approval
    /// that arrives later does not turn the tunnel on.
    public var shouldStartTunnel: (@MainActor () -> Bool)?

    public init(
        control: any TransparentProxyControl,
        bridgePort: UInt16 = UInt16(CoreLocations.transparentBridgePort)
    ) {
        self.control = control
        self.bridgePort = bridgePort
        control.onChange = { [weak self] in
            Task { await self?.systemChanged() }
        }
    }

    /// Whether this copy of ProxyLab can install an extension at all.
    public var readiness: TransparentProxyReadiness { control.readiness }

    /// Whether the configuration is carrying traffic, or about to.
    public var isOn: Bool {
        guard let tunnel = snapshot?.tunnel else { return false }
        return tunnel == .on || tunnel == .starting
    }

    /// Whether Turn On can install or connect.
    ///
    /// Not installed is a ready build's first run, not a broken one. Turn On
    /// is what asks macOS to install the extension; hiding it until the
    /// extension is installed means it can never be installed.
    public var canTurnOn: Bool {
        readiness == .ready && !isWorking && !isOn && snapshot?.tunnel != .stopping
    }

    public var canTurnOff: Bool { !isWorking && isOn }

    /// Whether asking macOS again is meaningful. A missing install is one of
    /// the answers that question can give, so it stays available then.
    public var canRefresh: Bool { readiness == .ready && !isWorking }

    /// The status line.
    public var statusText: String {
        if readiness != .ready { return "Unavailable in this build" }
        if isWorking { return "Working…" }
        guard let snapshot else { return "Checking…" }

        switch snapshot.tunnel {
        case .on: return "On — this Mac's web traffic goes through ProxyLab"
        case .starting: return "Starting…"
        case .stopping: return "Stopping…"
        case .notConfigured, .off: return "Off — only apps pointed at the proxy are captured"
        case .invalid: return "The saved configuration is not usable"
        }
    }

    /// The line about the extension itself.
    public var extensionText: String? {
        guard readiness == .ready, let state = snapshot?.extensionState else { return nil }

        switch state {
        case .unknown: return nil
        case .notInstalled: return "Extension not installed — Turn On installs it"
        case .awaitingApproval: return "Extension waiting for approval"
        case .installed: return "Extension installed"
        case .willCompleteAfterReboot: return "Extension finishes installing after a restart"
        }
    }

    /// What to say while capture is on, which is only true while it is on.
    ///
    /// - Parameter coreConnected: whether the window's IPC connection to the
    ///   core is up. With no core, the provider carries flows straight
    ///   through, and saying "on" without saying that would be a lie.
    public func warning(coreConnected: Bool) -> String? {
        guard isOn else { return nil }

        if isPassingThrough(coreConnected: coreConnected) {
            return "The ProxyLab core is not reachable, so traffic is passing straight "
                + "through and nothing is captured."
        }

        return "Every app's HTTP and HTTPS on this Mac goes through ProxyLab while this is on. "
            + "Apps that pin their certificates, and some macOS services, will fail until "
            + "you turn it off."
    }

    /// Whether capture is on and nothing is being captured, because the core
    /// is not there to hand flows to.
    public func isPassingThrough(coreConnected: Bool) -> Bool {
        isOn && (!coreConnected || providerStatus?.bridge == .unreachable)
    }

    /// Asks the system where things stand. Changes nothing.
    public func refresh() async {
        guard readiness == .ready else { return }

        snapshot = await control.snapshot()
        providerStatus = isOn ? await control.providerStatus() : nil
    }

    /// The tunnel configuration only.
    ///
    /// Lifecycle waits use this. A provider status message that never arrives
    /// is not part of deciding whether the tunnel is up.
    public func refreshConfiguration() async {
        guard readiness == .ready else { return }
        snapshot = await control.snapshot()
    }

    /// Forgets a Turn On that is waiting for approval, so a later approval
    /// does not start the tunnel. Synchronous: Stop sets it before any await.
    public func cancelPendingStart() {
        finishWhenApproved = false
        turnOffRequested = true
        control.abandonInFlightWork()
    }

    /// Installs the extension if it has to, and switches capture on.
    public func turnOn() async {
        guard readiness == .ready, !isWorking else { return }
        guard shouldStartTunnel?() ?? true else {
            finishWhenApproved = false
            return
        }

        operationTicket += 1
        let ticket = operationTicket
        isWorking = true
        lastError = nil
        lastErrorDetail = nil
        notice = nil
        turnOffRequested = false
        defer {
            if operationTicket == ticket { becameIdle() }
        }

        do {
            let current = await control.snapshot()
            guard operationTicket == ticket else { return }
            snapshot = current

            // An enabled extension at this build's version is started through
            // the configuration below. Submitting an activation request for
            // that same version makes macOS validate a replacement, and a
            // validation failure is not a rejected signature.
            if current.requiresActivation {
                switch try await control.activate() {
                case .installed:
                    break
                case .awaitingApproval:
                    guard operationTicket == ticket else { return }
                    finishWhenApproved = true
                    notice = Self.approvalInstructions
                    snapshot = await control.snapshot()
                    return
                case .willCompleteAfterReboot:
                    guard operationTicket == ticket else { return }
                    notice = "Restart this Mac to finish installing the extension, then turn this on."
                    snapshot = await control.snapshot()
                    return
                }
            }

            // Stop may have been pressed while activation was in flight, or
            // this Turn On is the approval continuation and Stop already won.
            guard !turnOffRequested, shouldStartTunnel?() ?? true else {
                finishWhenApproved = false
                return
            }

            finishWhenApproved = false
            let generation = control.beginTurnOn()
            guard !turnOffRequested, shouldStartTunnel?() ?? true else { return }
            try await control.turnOn(
                ProviderConfiguration(bridgePort: bridgePort),
                generation: generation
            )
        } catch {
            finishWhenApproved = false
            guard operationTicket == ticket else { return }
            record(error)
        }

        guard operationTicket == ticket else { return }
        snapshot = await control.snapshot()
    }

    /// Switches capture off. The extension stays installed.
    ///
    /// A Turn On that has already connected the tunnel is given
    /// ``turnOffWaitBound`` to finish, and then this turns the tunnel off.
    /// A Turn On that has not connected it is abandoned: its generation can
    /// no longer call `startVPNTunnel`, so Stop does not wait out that call.
    public func turnOff() async {
        guard readiness == .ready else { return }

        finishWhenApproved = false
        turnOffRequested = true
        control.abandonInFlightWork()
        if isWorking && isOn {
            let finished = await waitUntilIdle()
            if !finished || isWorking { return }
        }

        operationTicket += 1
        let ticket = operationTicket
        isWorking = true
        lastError = nil
        lastErrorDetail = nil
        notice = nil
        turnOffRequested = false
        defer {
            if operationTicket == ticket { becameIdle() }
        }

        do {
            try await control.turnOff()
        } catch {
            guard operationTicket == ticket else { return }
            record(error)
        }

        guard operationTicket == ticket else { return }
        snapshot = await control.snapshot()
        providerStatus = nil
    }

    /// Keeps the system's error next to the sentence the panel shows.
    private func record(_ error: Error) {
        let failure = TransparentProxyFailure.from(error)
        lastError = failure.message
        lastErrorDetail = failure.diagnosticRecord

        if let detail = failure.diagnosticRecord {
            Self.log.error(
                "transparent capture failed: \(failure.message, privacy: .public) \(detail, privacy: .public)"
            )
        } else {
            Self.log.error("transparent capture failed: \(failure.message, privacy: .public)")
        }
    }

    /// Where approval is given. Said in full, because the pane is several
    /// levels deep and nothing on screen points at it.
    public static let approvalInstructions =
        "Allow ProxyLab in System Settings › General › Login Items & Extensions › "
        + "Network Extensions. Capture turns on once it is allowed."

    /// The system said something changed: read it again, and finish a Turn On
    /// that was waiting for approval.
    private func systemChanged() async {
        guard !isWorking else { return }

        await refresh()

        if finishWhenApproved, snapshot?.extensionState == .installed {
            finishWhenApproved = false
            await turnOn()
        }
        onSettled?()
    }

    /// `isWorking` is false again, and anyone waiting to turn the tunnel off
    /// may proceed. ``onSettled`` runs after that, so a listener sees the
    /// result rather than the call that was still in flight.
    private func becameIdle() {
        isWorking = false
        onSettled?()
    }

    /// Polls until Turn On has returned, or the bound passes.
    ///
    /// `isWorking` is the condition. The sleep is the gap between checks.
    private func waitUntilIdle() async -> Bool {
        if !isWorking { return true }

        let deadline = ContinuousClock.now.advanced(by: turnOffWaitBound)
        while isWorking {
            if ContinuousClock.now >= deadline { return false }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return true
    }
}
