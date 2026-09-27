//  Capturing this Mac's own traffic without a proxy setting.
//
//      Turn On ─▶ install the extension ─▶ (approve it in System Settings)
//                                        ─▶ switch the configuration on
//
//      Turn Off ─▶ switch it off
//
//  # Off until someone says otherwise, every time
//
//  Nothing here reads a preference on launch or turns anything on by itself.
//  Refreshing asks the system what it says and changes nothing, and quitting
//  the app turns capture off again (see `SystemTransparentProxyControl`). The
//  one thing that continues on its own is a Turn On that stopped to wait for
//  approval: once macOS says the extension is allowed, the switch that was
//  pressed finishes — in this session, and never in a later one.
//
//  # The system decides, and this shows what it decided
//
//  Nothing on screen says "on" until the system's configuration says it is
//  connected. A switch that says on is a configuration that is carrying
//  traffic, and never an intention.

import Foundation
import Observation
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
    /// Something to do that is not an error: approving the extension, or
    /// restarting the Mac.
    public private(set) var notice: String?

    private let control: any TransparentProxyControl
    private let bridgePort: UInt16

    /// Turn On was pressed and is waiting for the extension to be approved.
    private var finishWhenApproved = false

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

    public var canTurnOn: Bool {
        readiness == .ready && !isWorking && !isOn && snapshot?.tunnel != .stopping
    }

    public var canTurnOff: Bool { !isWorking && isOn }

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
                + "through and nothing is captured. Start the core, or turn this off."
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

    /// Installs the extension if it has to, and switches capture on.
    public func turnOn() async {
        guard readiness == .ready, !isWorking else { return }

        isWorking = true
        lastError = nil
        notice = nil
        defer { isWorking = false }

        do {
            let current = await control.snapshot()
            snapshot = current

            if current.extensionState != .installed {
                switch try await control.activate() {
                case .installed:
                    break
                case .awaitingApproval:
                    finishWhenApproved = true
                    notice = Self.approvalInstructions
                    snapshot = await control.snapshot()
                    return
                case .willCompleteAfterReboot:
                    notice = "Restart this Mac to finish installing the extension, then turn this on."
                    snapshot = await control.snapshot()
                    return
                }
            }

            finishWhenApproved = false
            try await control.turnOn(ProviderConfiguration(bridgePort: bridgePort))
        } catch {
            finishWhenApproved = false
            lastError = TransparentProxyFailure.from(error).message
        }

        snapshot = await control.snapshot()
    }

    /// Switches capture off. The extension stays installed.
    public func turnOff() async {
        guard !isWorking else { return }

        isWorking = true
        lastError = nil
        notice = nil
        finishWhenApproved = false
        defer { isWorking = false }

        do {
            try await control.turnOff()
        } catch {
            lastError = TransparentProxyFailure.from(error).message
        }

        snapshot = await control.snapshot()
        providerStatus = nil
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
    }
}
