//  This user's trust of one certificate: the CA the core is using now.
//
//  Identity is the DER encoding passed in. Nothing here looks at the subject,
//  deletes another certificate, or opens the System keychain for writing.
//  That keychain is root-owned. Putting a trusted root there from this
//  process would need a privileged helper or Apple's setuid authorization
//  trampoline. This app has neither. It does not run `security`, `sudo`, or
//  another tool.
//
//  What it does use is Security.framework. The certificate is added to the
//  login keychain, which this user can write, and
//  `SecTrustSettingsSetTrustSettings` records it in the user trust domain.
//  That call presents the panel that asks for this user's credentials. The
//  admin trust domain is only read, so a certificate this Mac already trusts
//  system-wide is recognized and not written again.
//
//  The record trusts that one root, rather than a single policy. Evaluating
//  this CA under the SSL policy fails as a leaf — it is a CA certificate,
//  with a lifetime a server certificate is not allowed — whether or not it
//  is trusted, so that result cannot tell the two apart. The basic policy
//  is the evaluation that reports "not trusted" until the root is trusted
//  and succeeds once it is. TLS clients then accept the server certificates
//  the core signs with it. The trust is this user's, for this certificate.
//
//  An install that returns success is not trusted until `match` evaluates
//  the same bytes again. The caller does that, and it does not enable the
//  system proxy on this type's word alone.

import Foundation
import os
import Security

/// The production trust store. A test does not construct it: constructing it
/// is what talks to this Mac's keychains.
@MainActor
public final class SecurityInterceptionTrustStore: CaptureTrustStoring {
    private static let log = Logger(subsystem: "com.proxylab.ProxyLab", category: "capture-trust")

    public init() {}

    public func match(exactDER: Data) -> CaptureTrustMatch {
        guard let certificate = SecCertificateCreateWithData(nil, exactDER as CFData) else {
            return .absent
        }

        if trustedAnchor(certificate) {
            return .trusted
        }

        let present = certificatesInSearchList().contains { sameDER($0, as: exactDER) }
            || trustCertificates(in: .user).contains { sameDER($0, as: exactDER) }
            || trustCertificates(in: .admin).contains { sameDER($0, as: exactDER) }
        return present ? .presentUntrusted : .absent
    }

    public func installTrust(exactDER: Data) -> CaptureTrustInstallResult {
        guard let certificate = SecCertificateCreateWithData(nil, exactDER as CFData) else {
            return .failed
        }

        switch addToLoginKeychain(certificate) {
        case .installed:
            break
        case let other:
            return other
        }

        // Nil settings are "trust this root", which is the record the basic
        // policy evaluation can observe. The domain is the user domain. The
        // call blocks on this user's credential panel.
        let status = SecTrustSettingsSetTrustSettings(certificate, .user, nil)
        return outcome(of: status)
    }

    // MARK: - Evaluation

    /// Whether `certificate` is a trusted anchor.
    ///
    /// The system anchors are left as they are. Passing this certificate in
    /// as an anchor would make the evaluation succeed for a certificate
    /// nobody trusts.
    private func trustedAnchor(_ certificate: SecCertificate) -> Bool {
        var trust: SecTrust?
        let created = SecTrustCreateWithCertificates(certificate, SecPolicyCreateBasicX509(), &trust)
        guard created == errSecSuccess, let trust else {
            Self.log.error("Trust evaluation could not be created (status \(created)).")
            return false
        }

        // A local CA has nothing to fetch. Leaving the network on would make
        // the check wait on a revocation lookup that cannot succeed.
        _ = SecTrustSetNetworkFetchAllowed(trust, false)

        var error: CFError?
        _ = SecTrustEvaluateWithError(trust, &error)

        var result = SecTrustResultType.invalid
        guard SecTrustGetTrustResult(trust, &result) == errSecSuccess else { return false }

        switch result {
        case .proceed, .unspecified:
            return true
        default:
            return false
        }
    }

    // MARK: - Presence

    private func certificatesInSearchList() -> [SecCertificate] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassCertificate,
            kSecMatchLimit as String: kSecMatchLimitAll,
            kSecReturnRef as String: true,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else {
            Self.log.error("Certificate search failed with status \(status).")
            return []
        }
        return result as? [SecCertificate] ?? []
    }

    private func trustCertificates(in domain: SecTrustSettingsDomain) -> [SecCertificate] {
        var result: CFArray?
        let status = SecTrustSettingsCopyCertificates(domain, &result)
        guard status == errSecSuccess, let result = result as? [SecCertificate] else { return [] }
        return result
    }

    private func sameDER(_ certificate: SecCertificate, as der: Data) -> Bool {
        SecCertificateCopyData(certificate) as Data == der
    }

    // MARK: - Writing the user record

    /// Puts `certificate` in the login keychain. A copy that is already
    /// there is success. Another certificate is left where it is.
    ///
    /// `SecItemAdd` is the current call. It answers `errSecParam` for some
    /// certificates; `SecCertificateAddToKeychain` with a nil keychain is
    /// then the same write into the default keychain, which is the login
    /// keychain. Neither call names the System keychain.
    private func addToLoginKeychain(_ certificate: SecCertificate) -> CaptureTrustInstallResult {
        let query: [String: Any] = [
            kSecClass as String: kSecClassCertificate,
            kSecValueRef as String: certificate,
        ]
        let added = SecItemAdd(query as CFDictionary, nil)
        if added == errSecSuccess || added == errSecDuplicateItem { return .installed }
        guard added == errSecParam else { return outcome(of: added) }

        let legacy = SecCertificateAddToKeychain(certificate, nil)
        if legacy == errSecSuccess || legacy == errSecDuplicateItem { return .installed }
        return outcome(of: legacy)
    }

    private func outcome(of status: OSStatus) -> CaptureTrustInstallResult {
        switch status {
        case errSecSuccess:
            return .installed
        case errSecUserCanceled, errAuthorizationCanceled:
            Self.log.notice("Certificate trust was cancelled.")
            return .cancelled
        case errSecInteractionNotAllowed, errAuthorizationInteractionNotAllowed:
            Self.log.error("Certificate trust was not asked because interaction is not allowed.")
            return .notAsked
        default:
            Self.log.error("Certificate trust was not changed (status \(status)).")
            return .failed
        }
    }
}
