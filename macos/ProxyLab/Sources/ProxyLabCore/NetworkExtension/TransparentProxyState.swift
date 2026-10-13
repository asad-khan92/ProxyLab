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
    /// Whether Turn On has to submit an activation request before it can
    /// start the configuration. An enabled extension at this build's version
    /// does not: the configuration is started on the copy macOS already has.
    public var requiresActivation: Bool

    public init(
        extensionState: ExtensionState,
        tunnel: TunnelState,
        requiresActivation: Bool? = nil
    ) {
        self.extensionState = extensionState
        self.tunnel = tunnel
        self.requiresActivation = requiresActivation ?? (extensionState != .installed)
    }
}

/// `CFBundleShortVersionString` and `CFBundleVersion`, the two identifiers
/// macOS compares before it will replace an installed system extension.
struct ExtensionVersion: Equatable, Sendable {
    var shortVersion: String
    var bundleVersion: String

    init(shortVersion: String, bundleVersion: String) {
        self.shortVersion = shortVersion
        self.bundleVersion = bundleVersion
    }

    init?(info: [String: Any]) {
        guard let shortVersion = info["CFBundleShortVersionString"] as? String,
              let bundleVersion = info["CFBundleVersion"] as? String,
              !shortVersion.isEmpty, !bundleVersion.isEmpty
        else { return nil }

        self.shortVersion = shortVersion
        self.bundleVersion = bundleVersion
    }
}

/// Whether an activation request is warranted, and what to tell macOS when
/// it asks about replacing one that is already there.
enum ExtensionActivation {
    /// Submit `OSSystemExtensionRequest.activationRequest`, or start the
    /// configuration that is already installed.
    enum Plan: Equatable, Sendable {
        case activate
        case useInstalled
    }

    /// The delegate's answer when macOS asks about an existing extension.
    enum Replacement: Equatable, Sendable {
        /// The bundled extension's version differs. Let the existing request
        /// replace it; macOS still validates that replacement.
        case replace
        /// Same version. Replacing it is what macOS rejects during validation.
        case keepInstalled
    }

    static func plan(
        state: ExtensionState,
        installed: ExtensionVersion?,
        bundled: ExtensionVersion?,
        configurationSaved: Bool = false
    ) -> Plan {
        switch state {
        case .installed:
            if let installed, let bundled, installed != bundled {
                return .activate
            }
            return .useInstalled
        case .unknown where configurationSaved:
            // A properties request that does not answer is not evidence the
            // extension is missing. The saved configuration is the one to
            // start; submitting activation for that unknown state only waits
            // out another system call.
            return .useInstalled
        case .unknown, .notInstalled, .awaitingApproval, .willCompleteAfterReboot:
            return .activate
        }
    }

    static func replacement(existing: ExtensionVersion, proposed: ExtensionVersion) -> Replacement {
        existing == proposed ? .keepInstalled : .replace
    }

    /// A same-version replacement that this process declined comes back as
    /// `OSSystemExtensionError.requestCanceled` (code 11). The installed
    /// extension is still the one to start.
    static func useInstalledAfterCancellation(domain: String, code: Int) -> Bool {
        domain == TransparentProxyFailure.systemExtensionDomain && code == 11
    }
}

/// What asking for the extension to be installed came to.
public enum ActivationOutcome: Equatable, Sendable {
    case installed
    case awaitingApproval
    case willCompleteAfterReboot
}

/// Something the system refused, as a sentence someone can act on.
///
/// `message` is what the panel shows. `domain`, `code`,
/// `systemDescription`, and `underlying` are the `NSError` that sentence
/// came from, kept so a friendly line cannot hide which failure it was.
/// `systemDescription` is that error's `localizedDescription`. It is not
/// named that, because `Error.localizedDescription` would otherwise hide it.
public struct TransparentProxyFailure: Error, Equatable, Sendable {
    public let message: String
    public let domain: String?
    public let code: Int?
    public let systemDescription: String?
    public let underlying: String?

    public init(
        message: String,
        domain: String? = nil,
        code: Int? = nil,
        systemDescription: String? = nil,
        underlying: String? = nil
    ) {
        self.message = message
        self.domain = domain
        self.code = code
        self.systemDescription = systemDescription
        self.underlying = underlying
    }

    /// Domain, code, the system's own description, and one underlying error.
    public var diagnosticRecord: String? {
        guard let domain, let code else { return nil }

        var record = "\(domain) \(code)"
        if let systemDescription, !systemDescription.isEmpty {
            record += ": \(systemDescription)"
        }
        if let underlying, !underlying.isEmpty {
            record += " underlying \(underlying)"
        }
        return record
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
        let underlying = (error.userInfo[NSUnderlyingErrorKey] as? NSError).map { nested in
            "\(nested.domain) \(nested.code): \(nested.localizedDescription)"
        }

        return Self(
            message: message(domain: error.domain, code: error.code),
            domain: error.domain,
            code: error.code,
            systemDescription: error.localizedDescription,
            underlying: underlying
        )
    }

    /// A sentence for one published code.
    ///
    /// `OSSystemExtensionError.Code` in the SystemExtensions header:
    /// 8 is `codeSignatureInvalid` and 9 is `validationFailed`. They are
    /// different failures. Only 8 is a rejected signature.
    private static func message(domain: String, code: Int) -> String {
        switch (domain, code) {
        case (systemExtensionDomain, 2):
            return TransparentProxyReadiness.unsigned.message ?? ""
        case (systemExtensionDomain, 3):
            return TransparentProxyReadiness.notInApplications.message ?? ""
        case (systemExtensionDomain, 4), (systemExtensionDomain, 5), (systemExtensionDomain, 7):
            return TransparentProxyReadiness.extensionMissing.message ?? ""
        case (systemExtensionDomain, 8):
            return "macOS rejected the extension's signature. Rebuild with the "
                + "same team and identity for the app and the extension."
        case (systemExtensionDomain, 9):
            return "macOS could not validate the extension. If it is already "
                + "installed, press Turn On again. Replacing an installed "
                + "extension needs a newer version."
        case (systemExtensionDomain, 10):
            return "This Mac's policy does not allow the extension, "
                + "which usually means device management blocks it."
        case (systemExtensionDomain, 11), (systemExtensionDomain, 12):
            return "The request was cancelled. Press Turn On to try again."
        case (systemExtensionDomain, 13):
            return "macOS needs an administrator to allow the extension. "
                + "Press Turn On and follow the prompt."
        case (networkExtensionDomain, 5):
            return "ProxyLab could not save its proxy configuration. If macOS "
                + "asked to add proxy configurations, choose Allow, then press Turn On again."
        case (networkExtensionDomain, 1), (networkExtensionDomain, 6):
            return "macOS says the proxy configuration is not valid. "
                + "Turn it off, then on again."
        case (networkExtensionDomain, 2):
            return "The proxy configuration is switched off in System Settings "
                + "› Network. Turn it on there, or press Turn On again."
        case (networkExtensionDomain, 3):
            return "The extension did not start. Check that it is allowed in "
                + "System Settings › General › Login Items & Extensions."
        case (networkExtensionDomain, 4):
            return "The proxy configuration changed underneath ProxyLab. Press Turn On again."
        default:
            return "macOS refused (\(domain) \(code))."
        }
    }
}
