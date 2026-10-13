//  The reach the Network Extension section has into macOS.
//
//  One protocol, so the model can be tested against a stand-in that answers
//  what a test wrote: the real one needs a signed, approved extension in
//  /Applications, which a test run does not have and must not install.
//
//  Nothing here talks to the ProxyLab core. The core is reached over its IPC
//  socket by the rest of the app; the extension reaches the core over its own
//  loopback bridge. What this protocol reaches is the system — and, through
//  Apple's provider-message channel, the provider's own account of itself.

import Foundation
import TransparentProxyKit

/// Installing, switching, and asking after the transparent proxy.
@MainActor
public protocol TransparentProxyControl: AnyObject {
    /// Whether this copy of ProxyLab can install an extension at all.
    var readiness: TransparentProxyReadiness { get }

    /// Called when the system says something changed — the configuration
    /// connected or disconnected, or somebody approved the extension.
    var onChange: (@MainActor () -> Void)? { get set }

    /// What the system says now, without changing anything.
    func snapshot() async -> TransparentProxySnapshot

    /// Asks for the extension to be installed, or confirms it is.
    func activate() async throws -> ActivationOutcome

    /// A generation for the Turn On that is about to start the tunnel.
    ///
    /// Stop bumps the generation. A Turn On that is still inside a system
    /// call sees the new value and does not start the tunnel afterwards.
    func beginTurnOn() -> UInt64

    /// The in-flight Turn On must not start the tunnel.
    func abandonInFlightWork()

    /// Saves the configuration, switched on, and connects it.
    ///
    /// `generation` is the value ``beginTurnOn()`` returned for this attempt.
    func turnOn(_ configuration: ProviderConfiguration, generation: UInt64) async throws

    /// Disconnects the configuration and switches it off.
    func turnOff() async throws

    /// The provider's own account of itself, while it is running.
    func providerStatus() async -> ProviderStatus?
}
