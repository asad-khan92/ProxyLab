//  The real thing: `SystemExtensions` to install the provider, and
//  `NETransparentProxyManager` to switch it on and off.
//
//      Turn On ─▶ an enabled extension at this build's version is already
//                 installed: skip ahead to the configuration
//                 otherwise OSSystemExtensionRequest.activationRequest
//                   │   macOS may ask for approval in System Settings
//                   │   a different version is replaced by that same request
//                   ▼
//                 NETransparentProxyManager.saveToPreferences
//                   │   macOS may ask to allow proxy configurations
//                   ▼
//                 connection.startVPNTunnel
//
//      Turn Off ─▶ connection.stopVPNTunnel, and the configuration disabled
//
//  Nothing here runs by itself. The app's capture lifecycle is what asks it
//  to install and to switch the tunnel, and only after the bundled core is
//  ready. Quitting turns the tunnel off and leaves the extension installed.
//  The notification below is a backstop for that tunnel if termination did
//  not already go through the lifecycle: it stops the tunnel and does not
//  uninstall the extension.
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

    /// How long the first activation callback is given.
    ///
    /// User approval is that callback returning `.awaitingApproval`. The time
    /// the person then spends in System Settings is not this bound.
    static let activationBound: Duration = .seconds(10)
    /// How long a properties request is given before the state is unknown.
    static let propertiesBound: Duration = .seconds(5)
    /// How long saving or loading a configuration is given.
    static let preferencesBound: Duration = .seconds(8)
    /// How long a provider status message is given.
    static let providerBound: Duration = .seconds(3)

    private let extensionIdentifier: String
    private let extensionBundleURL: URL
    private var manager: NETransparentProxyManager?
    /// Bumped to begin a Turn On and to abandon one. A Turn On keeps the
    /// value from when it began.
    private var turnGeneration: UInt64 = 0
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
        extensionBundleURL = extensionURL
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
            return TransparentProxySnapshot(
                extensionState: .unknown,
                tunnel: .notConfigured,
                requiresActivation: false
            )
        }

        let observed = await extensionProperties()
        let manager = try? await loadManager()
        let plan = ExtensionActivation.plan(
            state: observed.state,
            installed: observed.version,
            bundled: bundledVersion(),
            configurationSaved: manager != nil
        )

        return TransparentProxySnapshot(
            extensionState: observed.state,
            tunnel: manager.map { Self.tunnelState($0) } ?? .notConfigured,
            requiresActivation: plan == .activate
        )
    }

    public func providerStatus() async -> ProviderStatus? {
        guard let session = manager?.connection as? NETunnelProviderSession,
              session.status == .connected
        else { return nil }

        let gate = CallbackGate<ProviderStatus?>()
        return await withCheckedContinuation { continuation in
            do {
                try session.sendProviderMessage(ProviderMessage.status.encoded) { data in
                    gate.finish(continuation, data.flatMap(ProviderStatus.decode))
                }
            } catch {
                gate.finish(continuation, nil)
            }
            Task { @MainActor in
                try? await Task.sleep(for: Self.providerBound)
                gate.finish(continuation, nil)
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
            Task { @MainActor in
                try? await Task.sleep(for: Self.activationBound)
                delegate.expireActivation(.failure(Self.timedOut))
            }
        }
    }

    public func beginTurnOn() -> UInt64 {
        turnGeneration += 1
        return turnGeneration
    }

    public func abandonInFlightWork() {
        turnGeneration += 1
    }

    public func turnOn(_ configuration: ProviderConfiguration, generation: UInt64) async throws {
        guard turnGeneration == generation else { return }
        let manager = try await loadManager() ?? NETransparentProxyManager()
        guard turnGeneration == generation else { return }

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
            guard let saved = try await LifecycleBound.required(limit: Self.preferencesBound, {
                try await manager.saveToPreferences()
            }) else {
                throw Self.timedOut
            }
            _ = saved
            guard turnGeneration == generation else { return }
            // A saved configuration has to be read back before it can be
            // started — Apple's own requirement, not caution.
            guard let loaded = try await LifecycleBound.required(limit: Self.preferencesBound, {
                try await manager.loadFromPreferences()
            }) else {
                throw Self.timedOut
            }
            _ = loaded
            guard turnGeneration == generation else { return }
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
            guard let saved = try await LifecycleBound.required(limit: Self.preferencesBound, {
                try await manager.saveToPreferences()
            }) else {
                throw Self.timedOut
            }
            _ = saved
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
        guard let all = try await LifecycleBound.required(limit: Self.preferencesBound, {
            try await NETransparentProxyManager.loadAllFromPreferences()
        }) else {
            throw Self.timedOut
        }
        let ours = all.first { manager in
            (manager.protocolConfiguration as? NETunnelProviderProtocol)?
                .providerBundleIdentifier == extensionIdentifier
        }
        manager = ours

        return ours
    }

    /// The extension embedded in this app, when its bundle versions can be read.
    private func bundledVersion() -> ExtensionVersion? {
        guard let info = Bundle(url: extensionBundleURL)?.infoDictionary else { return nil }

        return ExtensionVersion(info: info)
    }

    private func extensionProperties() async -> ObservedExtension {
        let request = OSSystemExtensionRequest.propertiesRequest(
            forExtensionWithIdentifier: extensionIdentifier,
            queue: .main
        )

        return await withCheckedContinuation { continuation in
            let delegate = ExtensionRequest(properties: { observed in
                continuation.resume(returning: observed)
            })
            requests.removeAll(where: \.isAnswered)
            requests.append(delegate)
            request.delegate = delegate
            OSSystemExtensionManager.shared.submitRequest(request)
            Task { @MainActor in
                try? await Task.sleep(for: Self.propertiesBound)
                delegate.expireProperties(ObservedExtension(state: .unknown, version: nil))
            }
        }
    }

    private static let timedOut = TransparentProxyFailure(
        message: "macOS did not answer in time. Try again."
    )

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

/// What a properties request said, including the version macOS has installed.
private struct ObservedExtension: Equatable {
    var state: ExtensionState
    var version: ExtensionVersion?
}

/// One request's delegate, turning the framework's callbacks into a result.
///
/// Every callback is delivered on the main queue — the queue each request
/// is made with — so everything here is main-actor state.
private final class ExtensionRequest: NSObject, OSSystemExtensionRequestDelegate, @unchecked Sendable {
    private var activation: (@MainActor (Result<ActivationOutcome, Error>) -> Void)?
    private var properties: (@MainActor (ObservedExtension) -> Void)?
    private let finishedLater: (@MainActor () -> Void)?
    /// Set when this request was asked to replace an extension at the same
    /// version and declined. The cancellation that follows means the
    /// installed copy stays.
    private var keptInstalled = false

    init(
        activation: @escaping @MainActor (Result<ActivationOutcome, Error>) -> Void,
        finishedLater: @escaping @MainActor () -> Void
    ) {
        self.activation = activation
        self.properties = nil
        self.finishedLater = finishedLater
    }

    init(properties: @escaping @MainActor (ObservedExtension) -> Void) {
        self.activation = nil
        self.properties = properties
        self.finishedLater = nil
    }

    func request(
        _ request: OSSystemExtensionRequest,
        actionForReplacingExtension existing: OSSystemExtensionProperties,
        withExtension ext: OSSystemExtensionProperties
    ) -> OSSystemExtensionRequest.ReplacementAction {
        let decision = ExtensionActivation.replacement(
            existing: ExtensionVersion(
                shortVersion: existing.bundleShortVersion,
                bundleVersion: existing.bundleVersion
            ),
            proposed: ExtensionVersion(
                shortVersion: ext.bundleShortVersion,
                bundleVersion: ext.bundleVersion
            )
        )

        switch decision {
        case .replace:
            // A different version. The request macOS already has continues,
            // and macOS still validates the replacement.
            return .replace
        case .keepInstalled:
            // Same version. Asking macOS to replace it fails validation,
            // which is a different error from a rejected signature. The
            // installed extension stays, and the cancellation is completed
            // as "already installed" so the configuration can start.
            keptInstalled = true
            return .cancel
        }
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
                properties(ObservedExtension(state: .notInstalled, version: nil))
                return
            }
            finishedLater?()
        }
    }

    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        MainActor.assumeIsolated {
            let nsError = error as NSError
            if keptInstalled,
               ExtensionActivation.useInstalledAfterCancellation(domain: nsError.domain, code: nsError.code),
               answerActivation(.success(.installed))
            {
                return
            }
            if answerActivation(.failure(TransparentProxyFailure.from(error))) { return }
            if let properties {
                self.properties = nil
                properties(ObservedExtension(state: .unknown, version: nil))
            }
        }
    }

    func request(
        _ request: OSSystemExtensionRequest,
        foundProperties found: [OSSystemExtensionProperties]
    ) {
        // Read here, on the queue the framework called on, so what crosses
        // to the main actor is a value and not the framework's objects.
        let observed: ObservedExtension
        if let enabled = found.first(where: { $0.isEnabled && !$0.isUninstalling }) {
            observed = ObservedExtension(state: .installed, version: Self.version(enabled))
        } else if let waiting = found.first(where: \.isAwaitingUserApproval) {
            observed = ObservedExtension(state: .awaitingApproval, version: Self.version(waiting))
        } else {
            observed = ObservedExtension(state: .notInstalled, version: nil)
        }

        MainActor.assumeIsolated {
            guard let properties else { return }
            self.properties = nil
            properties(observed)
        }
    }

    private static func version(_ properties: OSSystemExtensionProperties) -> ExtensionVersion {
        ExtensionVersion(
            shortVersion: properties.bundleShortVersion,
            bundleVersion: properties.bundleVersion
        )
    }

    /// The activation bound passed. A callback that already answered is left alone.
    @MainActor
    func expireActivation(_ result: Result<ActivationOutcome, Error>) {
        _ = answerActivation(result)
    }

    /// The properties bound passed. A callback that already answered is left alone.
    @MainActor
    func expireProperties(_ observed: ObservedExtension) {
        guard let properties else { return }
        self.properties = nil
        properties(observed)
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
