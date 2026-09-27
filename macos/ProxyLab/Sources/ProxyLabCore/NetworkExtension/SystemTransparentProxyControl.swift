//  The real thing: `SystemExtensions` to install the provider, and
//  `NETransparentProxyManager` to switch it on and off.
//
//      Turn On ─▶ OSSystemExtensionRequest.activationRequest
//                   │   macOS may ask for approval in System Settings
//                   ▼
//                 NETransparentProxyManager.saveToPreferences
//                   │   macOS may ask to allow proxy configurations
//                   ▼
//                 connection.startVPNTunnel
//
//      Turn Off ─▶ connection.stopVPNTunnel, and the configuration disabled
//
//  Nothing here runs by itself. Launching the app reads the state and changes
//  nothing, so a ProxyLab that has just started is capturing only what is
//  pointed at its explicit proxy, whatever was on when it was last quit. And
//  quitting the app turns transparent capture off: a Mac whose every
//  connection goes through a debugging proxy nobody can see is a Mac whose
//  owner will not know why a pinned app has stopped working.
//
//  The system's own proxy settings are never read or written here. A
//  transparent proxy configuration is a Network Extension configuration of
//  its own, listed under System Settings › Network › VPN & Filters, and
//  removing it there removes all of this.

import AppKit
import Foundation
import NetworkExtension
import Security
import SystemExtensions
import TransparentProxyKit

/// The transparent proxy, as macOS runs it.
@MainActor
public final class SystemTransparentProxyControl: TransparentProxyControl {
    public let readiness: TransparentProxyReadiness
    public var onChange: (@MainActor () -> Void)?

    /// The provider's bundle identifier: the app's own, with this suffix.
    public static let extensionSuffix = ".TransparentProxy"

    /// What the configuration is called in System Settings.
    static let configurationName = "ProxyLab Transparent Capture"

    private let extensionIdentifier: String
    private var manager: NETransparentProxyManager?
    /// Requests still waiting on the system, kept so their delegates live
    /// until they are answered.
    private var requests: [ExtensionRequest] = []
    private var observers: [NSObjectProtocol] = []

    public init(bundle: Bundle = .main) {
        let identifier = (bundle.bundleIdentifier ?? "com.proxylab.ProxyLab") + Self.extensionSuffix
        extensionIdentifier = identifier

        let extensionURL = bundle.bundleURL
            .appendingPathComponent("Contents/Library/SystemExtensions", isDirectory: true)
            .appendingPathComponent("\(identifier).systemextension", isDirectory: true)
        readiness = .evaluate(
            bundlePath: bundle.bundleURL.path,
            isAppBundle: bundle.bundleURL.pathExtension == "app",
            extensionPresent: FileManager.default.fileExists(atPath: extensionURL.path),
            hasInstallEntitlement: Self.hasEntitlement("com.apple.developer.system-extension.install")
        )

        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: .NEVPNStatusDidChange, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.onChange?() }
        })
        observers.append(center.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.stopForTermination() }
        })
    }

    // MARK: - Reading

    public func snapshot() async -> TransparentProxySnapshot {
        guard readiness == .ready else {
            return TransparentProxySnapshot(extensionState: .unknown, tunnel: .notConfigured)
        }

        let extensionState = await extensionProperties()
        let manager = try? await loadManager()

        return TransparentProxySnapshot(
            extensionState: extensionState,
            tunnel: manager.map { Self.tunnelState($0) } ?? .notConfigured
        )
    }

    public func providerStatus() async -> ProviderStatus? {
        guard let session = manager?.connection as? NETunnelProviderSession,
              session.status == .connected
        else { return nil }

        return await withCheckedContinuation { continuation in
            do {
                try session.sendProviderMessage(ProviderMessage.status.encoded) { data in
                    continuation.resume(returning: data.flatMap(ProviderStatus.decode))
                }
            } catch {
                continuation.resume(returning: nil)
            }
        }
    }

    // MARK: - Changing

    public func activate() async throws -> ActivationOutcome {
        let request = OSSystemExtensionRequest.activationRequest(
            forExtensionWithIdentifier: extensionIdentifier,
            queue: .main
        )

        return try await withCheckedThrowingContinuation { continuation in
            let delegate = ExtensionRequest(
                activation: { [weak self] result in
                    continuation.resume(with: result)
                    self?.onChange?()
                },
                // Approval arrives later — sometimes much later — as the
                // request finishing, which is a change like any other.
                finishedLater: { [weak self] in self?.onChange?() }
            )
            requests.removeAll(where: \.isAnswered)
            requests.append(delegate)
            request.delegate = delegate
            OSSystemExtensionManager.shared.submitRequest(request)
        }
    }

    public func turnOn(_ configuration: ProviderConfiguration) async throws {
        let manager = try await loadManager() ?? NETransparentProxyManager()

        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = extensionIdentifier
        // Required, and shown in System Settings; the provider reads its
        // port from `providerConfiguration`, not from here.
        proto.serverAddress = "\(ProviderConfiguration.bridgeHost):\(configuration.bridgePort)"
        proto.providerConfiguration = configuration.dictionary

        manager.localizedDescription = Self.configurationName
        manager.protocolConfiguration = proto
        manager.isEnabled = true
        // Never on demand: it connects when Turn On is pressed and at no other
        // time.
        manager.isOnDemandEnabled = false

        do {
            try await manager.saveToPreferences()
            // A saved configuration has to be read back before it can be
            // started — Apple's own requirement, not caution.
            try await manager.loadFromPreferences()
            try manager.connection.startVPNTunnel()
        } catch {
            throw TransparentProxyFailure.from(error)
        }

        self.manager = manager
    }

    public func turnOff() async throws {
        guard let manager = try await loadManager() else { return }

        manager.connection.stopVPNTunnel()
        manager.isEnabled = false

        do {
            try await manager.saveToPreferences()
        } catch {
            throw TransparentProxyFailure.from(error)
        }
    }

    // MARK: - Inside

    /// ProxyLab's own configuration, if one has been saved.
    ///
    /// Picked out by the provider it names, so another app's transparent
    /// proxy on the same Mac is never read, changed, or switched off.
    private func loadManager() async throws -> NETransparentProxyManager? {
        let all = try await NETransparentProxyManager.loadAllFromPreferences()
        let ours = all.first { manager in
            (manager.protocolConfiguration as? NETunnelProviderProtocol)?
                .providerBundleIdentifier == extensionIdentifier
        }
        manager = ours

        return ours
    }

    private func extensionProperties() async -> ExtensionState {
        let request = OSSystemExtensionRequest.propertiesRequest(
            forExtensionWithIdentifier: extensionIdentifier,
            queue: .main
        )

        return await withCheckedContinuation { continuation in
            let delegate = ExtensionRequest(properties: { state in
                continuation.resume(returning: state)
            })
            requests.removeAll(where: \.isAnswered)
            requests.append(delegate)
            request.delegate = delegate
            OSSystemExtensionManager.shared.submitRequest(request)
        }
    }

    /// Quitting ProxyLab turns transparent capture off. See the file header.
    private func stopForTermination() {
        guard let connection = manager?.connection,
              [.connected, .connecting, .reasserting].contains(connection.status)
        else { return }

        connection.stopVPNTunnel()
    }

    static func tunnelState(_ manager: NETransparentProxyManager) -> TunnelState {
        guard manager.isEnabled else {
            return manager.connection.status == .invalid ? .invalid : .off
        }

        switch manager.connection.status {
        case .connected: return .on
        case .connecting, .reasserting: return .starting
        case .disconnecting: return .stopping
        case .invalid: return .invalid
        case .disconnected: return .off
        @unknown default: return .off
        }
    }

    /// Whether this process was signed with `name` set to `true`.
    private static func hasEntitlement(_ name: String) -> Bool {
        guard let task = SecTaskCreateFromSelf(nil) else { return false }

        return SecTaskCopyValueForEntitlement(task, name as CFString, nil) as? Bool == true
    }
}

/// One request's delegate, turning the framework's callbacks into a result.
///
/// Every callback is delivered on the main queue — the queue each request
/// is made with — so everything here is main-actor state.
private final class ExtensionRequest: NSObject, OSSystemExtensionRequestDelegate, @unchecked Sendable {
    private var activation: (@MainActor (Result<ActivationOutcome, Error>) -> Void)?
    private var properties: (@MainActor (ExtensionState) -> Void)?
    private let finishedLater: (@MainActor () -> Void)?

    init(
        activation: @escaping @MainActor (Result<ActivationOutcome, Error>) -> Void,
        finishedLater: @escaping @MainActor () -> Void
    ) {
        self.activation = activation
        self.properties = nil
        self.finishedLater = finishedLater
    }

    init(properties: @escaping @MainActor (ExtensionState) -> Void) {
        self.activation = nil
        self.properties = properties
        self.finishedLater = nil
    }

    func request(
        _ request: OSSystemExtensionRequest,
        actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        // A rebuilt ProxyLab carries a rebuilt provider, and the one it
        // carries is the one that matches it.
        .replace
    }

    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        MainActor.assumeIsolated {
            _ = answerActivation(.success(.awaitingApproval))
        }
    }

    func request(
        _ request: OSSystemExtensionRequest,
        didFinishWithResult result: OSSystemExtensionRequest.Result
    ) {
        MainActor.assumeIsolated {
            if answerActivation(.success(result == .willCompleteAfterReboot
                ? .willCompleteAfterReboot
                : .installed))
            {
                return
            }
            if let properties {
                self.properties = nil
                properties(.notInstalled)
                return
            }
            finishedLater?()
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        MainActor.assumeIsolated {
            if answerActivation(.failure(TransparentProxyFailure.from(error))) { return }
            if let properties {
                self.properties = nil
                properties(.unknown)
            }
        }
    }

    func request(
        _ request: OSSystemExtensionRequest,
        foundProperties found: [OSSystemExtensionProperties]
    ) {
        // Read here, on the queue the framework called on, so what crosses
        // to the main actor is a value and not the framework's objects.
        let state: ExtensionState
        if found.contains(where: { $0.isEnabled && !$0.isUninstalling }) {
            state = .installed
        } else if found.contains(where: \.isAwaitingUserApproval) {
            state = .awaitingApproval
        } else {
            state = .notInstalled
        }

        MainActor.assumeIsolated {
            guard let properties else { return }
            self.properties = nil
            properties(state)
        }
    }

    /// Whether nothing more is waiting on this request.
    @MainActor
    var isAnswered: Bool { activation == nil && properties == nil && finishedLater == nil }

    /// Answers the activation, once. Whether there was one to answer.
    @MainActor
    @discardableResult
    private func answerActivation(_ result: Result<ActivationOutcome, Error>) -> Bool {
        guard let activation else { return false }
        self.activation = nil
        activation(result)

        return true
    }
}
