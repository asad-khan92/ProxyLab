//  What the system says about transparent capture, as values.
//
//  Three separate questions, answered by three separate parts of macOS, and
//  kept apart here because each fails in its own way and each needs its own
//  sentence when it does:
//
//      can this copy of ProxyLab install an extension at all?   Readiness
//      is the extension installed and approved?                 ExtensionState
//      is the proxy configuration switched on?                  TunnelState
//
//  None of these types does anything. `TransparentProxyControl` is what asks
//  the system; these are what it answers with, so the model and the view can
//  be tested with answers a test wrote.

import Foundation

/// Whether this copy of ProxyLab can install a system extension at all.
///
/// Every case but ``ready`` is something about how the app was built or where
/// it is, and none of them can be fixed by pressing a button in the app — so
/// each says what to do instead.
public enum TransparentProxyReadiness: Equatable, Sendable {
    case ready
    /// Run with `swift run`, so there is no bundle to carry an extension.
    case notAnAppBundle
    /// A bundle with no extension in it.
    case extensionMissing
    /// Signed without the entitlement that lets an app install one.
    case unsigned
    /// Anywhere but `/Applications`, which macOS requires.
    case notInApplications

    /// Works out which of these applies, most fundamental first.
    public static func evaluate(
        bundlePath: String,
        isAppBundle: Bool,
        extensionPresent: Bool,
        hasInstallEntitlement: Bool
    ) -> Self {
        guard isAppBundle else { return .notAnAppBundle }
        guard extensionPresent else { return .extensionMissing }
        guard hasInstallEntitlement else { return .unsigned }
        guard bundlePath.hasPrefix("/Applications/") else { return .notInApplications }

        return .ready
    }

    /// What to do about it, or `nil` when there is nothing to do.
    public var message: String? {
        switch self {
        case .ready:
            nil
        case .notAnAppBundle:
            "Transparent capture needs ProxyLab.app. Build it with Scripts/make-app-bundle.sh."
        case .extensionMissing:
            "This ProxyLab.app has no Network Extension in it. Rebuild it with "
                + "Scripts/make-app-bundle.sh."
        case .unsigned:
            "This build is not signed to install a Network Extension. Build with "
                + "PROXYLAB_TEAM_ID and the two provisioning profiles set — see the README."
        case .notInApplications:
            "macOS only installs Network Extensions from apps in /Applications. "
                + "Move ProxyLab.app there and open it from there."
        }
    }
}

/// Whether the system extension is installed.
public enum ExtensionState: Equatable, Sendable {
    /// Not asked yet, or the system would not say.
    case unknown
    case notInstalled
    /// Waiting for someone to allow it in System Settings.
    case awaitingApproval
    case installed
    /// Replacing an older copy, which takes a restart.
    case willCompleteAfterReboot
}

/// Whether the proxy configuration is connected.
public enum TunnelState: Equatable, Sendable {
    /// No configuration has been saved yet.
    case notConfigured
    case off
    case starting
    case on
    case stopping
    /// Saved, and the system will not use it.
    case invalid
}

/// Both halves, read at one moment.
public struct TransparentProxySnapshot: Equatable, Sendable {
    public var extensionState: ExtensionState
    public var tunnel: TunnelState

    public init(extensionState: ExtensionState, tunnel: TunnelState) {
        self.extensionState = extensionState
        self.tunnel = tunnel
    }
}

/// What asking for the extension to be installed came to.
public enum ActivationOutcome: Equatable, Sendable {
    case installed
    case awaitingApproval
    case willCompleteAfterReboot
}

/// Something the system refused, as a sentence someone can act on.
public struct TransparentProxyFailure: Error, Equatable, Sendable {
    public let message: String

    public init(message: String) {
        self.message = message
    }

    /// The system extension framework's error domain.
    static let systemExtensionDomain = "OSSystemExtensionErrorDomain"
    /// The Network Extension configuration error domain.
    static let networkExtensionDomain = "NEVPNErrorDomain"

    /// Reads an error from either framework.
    ///
    /// Matched on domain and code, which are the frameworks' published API,
    /// rather than on the frameworks' own descriptions, which are written for
    /// a different reader and are not stable.
    public static func from(_ error: Error) -> Self {
        if let failure = error as? Self { return failure }

        let error = error as NSError
        switch (error.domain, error.code) {
        case (systemExtensionDomain, 2):
            return .init(message: TransparentProxyReadiness.unsigned.message ?? "")
        case (systemExtensionDomain, 3):
            return .init(message: TransparentProxyReadiness.notInApplications.message ?? "")
        case (systemExtensionDomain, 4), (systemExtensionDomain, 5), (systemExtensionDomain, 7):
            return .init(message: TransparentProxyReadiness.extensionMissing.message ?? "")
        case (systemExtensionDomain, 8), (systemExtensionDomain, 9):
            return .init(message: "macOS rejected the extension's signature. Rebuild with the "
                + "same team and identity for the app and the extension.")
        case (systemExtensionDomain, 10):
            return .init(message: "This Mac's policy does not allow the extension, "
                + "which usually means device management blocks it.")
        case (systemExtensionDomain, 11), (systemExtensionDomain, 12):
            return .init(message: "The request was cancelled. Press Turn On to try again.")
        case (systemExtensionDomain, 13):
            return .init(message: "macOS needs an administrator to allow the extension. "
                + "Press Turn On and follow the prompt.")
        case (networkExtensionDomain, 5):
            return .init(message: "ProxyLab could not save its proxy configuration. If macOS "
                + "asked to add proxy configurations, choose Allow, then press Turn On again.")
        case (networkExtensionDomain, 1), (networkExtensionDomain, 6):
            return .init(message: "macOS says the proxy configuration is not valid. "
                + "Turn it off, then on again.")
        case (networkExtensionDomain, 2):
            return .init(message: "The proxy configuration is switched off in System Settings "
                + "› Network. Turn it on there, or press Turn On again.")
        case (networkExtensionDomain, 3):
            return .init(message: "The extension did not start. Check that it is allowed in "
                + "System Settings › General › Login Items & Extensions.")
        case (networkExtensionDomain, 4):
            return .init(message: "The proxy configuration changed underneath ProxyLab. Press Turn On again.")
        default:
            return .init(message: "macOS refused (\(error.domain) \(error.code)).")
        }
    }
}
