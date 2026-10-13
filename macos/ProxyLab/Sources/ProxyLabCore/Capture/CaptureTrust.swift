//  Whether this Mac trusts the CA the core is using, before the system proxy
//  is set.
//
//  The system HTTP and HTTPS proxy is not pointed at 127.0.0.1:9090 until
//  this reports ready. Ready means the certificate file the core stored was
//  read again after any trust change and Security.framework evaluates that
//  exact certificate as a trusted anchor. A certificate that merely has the
//  same name is not that certificate.
//
//  A test supplies its own store. The production app supplies the Security
//  framework store. Neither the decision nor a test writes this Mac's
//  keychain.

import Foundation

/// Whether certificate trust allows the system proxy to be enabled.
public enum CaptureTrustReadiness: Equatable, Sendable {
    /// The system proxy may be pointed at the explicit listener.
    case ready
    /// The system proxy must not be changed. `message` says why.
    case notReady(String)
}

/// The capture lifecycle asks this once the explicit listener is ready, and
/// does not claim the system proxy unless the answer is ready.
@MainActor
public protocol CaptureTrustChecking: AnyObject {
    func readiness() async -> CaptureTrustReadiness
}

/// What a trust check last learned, for the diagnostics sheet.
public struct CaptureTrustDiagnostic: Equatable, Sendable {
    /// SHA-256 of the current CA, or nil when that file cannot be read.
    public var sha256: String?
    /// Whether a certificate with those exact bytes is in a keychain or a
    /// trust record. Nil when that has not been asked, or cannot be asked.
    public var exactMatchInstalled: Bool?
    /// Whether evaluation trusts that exact certificate. Nil when it has not
    /// been asked.
    public var trusted: Bool?

    public init(sha256: String?, exactMatchInstalled: Bool?, trusted: Bool?) {
        self.sha256 = sha256
        self.exactMatchInstalled = exactMatchInstalled
        self.trusted = trusted
    }

    public static let unknown = CaptureTrustDiagnostic(
        sha256: nil,
        exactMatchInstalled: nil,
        trusted: nil
    )
}

/// The diagnostics sheet reads this. Refresh looks again and does not prompt.
@MainActor
public protocol CaptureTrustReporting: AnyObject {
    var diagnostic: CaptureTrustDiagnostic { get }
    func refreshDiagnostic()
}

/// How the exact current certificate stands in this user's trust.
public enum CaptureTrustMatch: Equatable, Sendable {
    /// No certificate with these bytes is installed.
    case absent
    /// These bytes are installed, and evaluation does not trust them.
    case presentUntrusted
    /// Evaluation trusts these bytes as an anchor.
    case trusted
}

/// What an install attempt did. Success here is not trust: the caller reads
/// the certificate again and evaluates it.
public enum CaptureTrustInstallResult: Equatable, Sendable {
    /// The trust operation returned success. Trust is not yet confirmed.
    case installed
    /// The user dismissed the authorization panel.
    case cancelled
    /// The operation failed.
    case failed
    /// The panel was not shown, because it must not be shown now.
    case notAsked
}

/// The keychain and trust settings, behind a protocol so a test never writes
/// the ones on this Mac.
@MainActor
public protocol CaptureTrustStoring: AnyObject {
    /// How `exactDER` stands. Comparison is the bytes, not the subject.
    func match(exactDER: Data) -> CaptureTrustMatch

    /// Installs and trusts `exactDER` for this user. Does not delete any
    /// other certificate.
    func installTrust(exactDER: Data) -> CaptureTrustInstallResult
}

/// The words the toolbar shows while trust is being decided.
enum CaptureTrustWording {
    static let checking = "Checking CA trust…"
    static let needsTrust = "ProxyLab CA needs trust"
    static let installing = "Installing ProxyLab CA…"
    static let cancelled = "CA trust cancelled"
    static let failed = "CA trust failed"

    static let missingDetail =
        "ProxyLab's certificate is not there yet. The system HTTP and HTTPS proxy was not changed."
    static let malformedDetail =
        "ProxyLab's certificate could not be read. The system HTTP and HTTPS proxy was not changed."
    static let cancelledDetail =
        "Certificate trust was cancelled. The system HTTP and HTTPS proxy was not changed."
    static let failedDetail =
        "ProxyLab could not trust its certificate. The system HTTP and HTTPS proxy was not changed."
    static let unverifiedDetail =
        "ProxyLab could not confirm that its certificate is trusted. The system HTTP and HTTPS proxy was not changed."
    static let needsTrustDetail =
        "ProxyLab's certificate still needs trust. The system HTTP and HTTPS proxy was not changed."
}

/// Decides whether the current CA may be used, and asks the store to trust it
/// when it is missing or untrusted.
///
/// The certificate is read from `certificateLocator`, which is the core's own
/// rule for where `ca-cert.pem` lives. After a trust operation the file is
/// read again. Ready is returned only when that second read is the same
/// certificate and the store's evaluation trusts it.
@MainActor
public final class MacOSCaptureTrust: CaptureTrustChecking, CaptureTrustReporting {
    public private(set) var diagnostic = CaptureTrustDiagnostic.unknown

    /// Told as the check moves. Nil clears the override and the capture
    /// phase's own title shows again.
    public var onActivity: (@MainActor (String?) -> Void)?

    /// Whether a credential panel may be presented. False while the app is
    /// quitting, and until the app says otherwise: a panel is not shown on
    /// a guess.
    public var authorizationPermitted: @MainActor () -> Bool

    private let certificateLocator: @MainActor () -> String?
    private let store: any CaptureTrustStoring

    public init(
        certificateLocator: @escaping @MainActor () -> String?,
        store: any CaptureTrustStoring,
        authorizationPermitted: @escaping @MainActor () -> Bool = { false }
    ) {
        self.certificateLocator = certificateLocator
        self.store = store
        self.authorizationPermitted = authorizationPermitted
    }

    /// Reads the CA again and records what evaluation says. Does not install.
    public func refreshDiagnostic() {
        _ = assess()
    }

    public func readiness() async -> CaptureTrustReadiness {
        onActivity?(CaptureTrustWording.checking)
        await Task.yield()

        switch assess() {
        case .missing:
            onActivity?(CaptureTrustWording.failed)
            return .notReady(CaptureTrustWording.missingDetail)
        case .malformed:
            onActivity?(CaptureTrustWording.failed)
            return .notReady(CaptureTrustWording.malformedDetail)
        case let .found(der, match):
            if match == .trusted {
                onActivity?(nil)
                return .ready
            }

            onActivity?(CaptureTrustWording.needsTrust)
            await Task.yield()

            guard authorizationPermitted() else {
                return .notReady(CaptureTrustWording.needsTrustDetail)
            }

            onActivity?(CaptureTrustWording.installing)
            await Task.yield()

            switch store.installTrust(exactDER: der) {
            case .cancelled:
                onActivity?(CaptureTrustWording.cancelled)
                return .notReady(CaptureTrustWording.cancelledDetail)
            case .notAsked:
                onActivity?(CaptureTrustWording.needsTrust)
                return .notReady(CaptureTrustWording.needsTrustDetail)
            case .failed:
                onActivity?(CaptureTrustWording.failed)
                return .notReady(CaptureTrustWording.failedDetail)
            case .installed:
                return confirm(original: der)
            }
        }
    }

    // MARK: - The file and the evaluation

    private enum Assessment {
        case missing
        case malformed
        case found(Data, CaptureTrustMatch)
    }

    private func assess() -> Assessment {
        let loaded = CaptureCertificateReader.load(at: certificateLocator())
        switch loaded {
        case .missing:
            diagnostic = CaptureTrustDiagnostic(sha256: nil, exactMatchInstalled: nil, trusted: false)
            return .missing
        case .malformed:
            diagnostic = CaptureTrustDiagnostic(sha256: nil, exactMatchInstalled: nil, trusted: false)
            return .malformed
        case let .certificate(der):
            let match = store.match(exactDER: der)
            diagnostic = CaptureTrustDiagnostic(
                sha256: CaptureCertificateReader.fingerprint(of: der),
                exactMatchInstalled: match != .absent,
                trusted: match == .trusted
            )
            return .found(der, match)
        }
    }

    /// The file is read again, and the certificate that was trusted has to
    /// still be the one there. Evaluation is asked a second time. The install
    /// result is not the answer.
    private func confirm(original: Data) -> CaptureTrustReadiness {
        guard case let .certificate(reloaded) = CaptureCertificateReader.load(at: certificateLocator()),
              reloaded == original
        else {
            _ = assess()
            onActivity?(CaptureTrustWording.failed)
            return .notReady(CaptureTrustWording.unverifiedDetail)
        }

        let match = store.match(exactDER: reloaded)
        diagnostic = CaptureTrustDiagnostic(
            sha256: CaptureCertificateReader.fingerprint(of: reloaded),
            exactMatchInstalled: match != .absent,
            trusted: match == .trusted
        )
        guard match == .trusted else {
            onActivity?(CaptureTrustWording.failed)
            return .notReady(CaptureTrustWording.unverifiedDetail)
        }

        onActivity?(nil)
        return .ready
    }
}
