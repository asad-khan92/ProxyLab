//  Getting ProxyLab's CA onto the emulator, and being honest about what that
//  buys.
//
//      adb -s <serial> push ~/.proxylab/ca/ca-cert.pem /sdcard/Download/proxylab-ca.crt
//
//  That is the whole of what ProxyLab does. The install itself is six taps in
//  Settings, by the developer, on purpose — there is no adb command that adds
//  a user CA, and the alternatives are all the ones this step rules out:
//  rooting the emulator, remounting `/system`, or writing into the system
//  certificate store. ProxyLab does none of them, so it copies the file where
//  the certificate installer can find it and says what to tap.
//
//  # A user CA is not trusted by every app, and saying otherwise would be a lie
//
//  Since Android 7 (API 24), an app that targets API 24 or later does **not**
//  trust user-installed CAs unless it opts in with a network security
//  configuration. So:
//
//      Chrome, and other browsers            usually works
//      a debug build with a network config   works, if it opts in
//      an ordinary release app               will not trust it
//      an app that pins its certificates     will not trust it, ever
//
//  HTTP is captured regardless — the proxy setting is all that needs. HTTPS
//  interception depends on the app on the other end, and ProxyLab reports that
//  rather than implying a CA install makes every app readable. Pinning and
//  apps that deliberately refuse user CAs are not something this step tries to
//  defeat.

import Foundation

/// Copies the CA where Android's certificate installer can reach it.
public struct CertificateDelivery: Sendable {
    /// Where the certificate is put on the device.
    ///
    /// `Downloads` is where the certificate installer looks first, and
    /// `.crt` is the extension it recognises — the same PEM bytes under a
    /// name the installer will offer to open.
    public static let remotePath = "/sdcard/Download/proxylab-ca.crt"

    private let runner: any AdbCommandRunner
    private let certificateExists: @Sendable (String) -> Bool

    public init(
        runner: any AdbCommandRunner,
        certificateExists: @escaping @Sendable (String) -> Bool = { path in
            FileManager.default.fileExists(atPath: path)
        }
    ) {
        self.runner = runner
        self.certificateExists = certificateExists
    }

    /// Pushes the certificate at `path` to the emulator, and says where it
    /// landed.
    ///
    /// Checked before adb is troubled with it: adb's message for a missing
    /// file names the file and not what the file was for, and "the core has
    /// not made a CA yet" is a different problem with a different fix.
    @discardableResult
    public func deliver(certificateAt path: String, to serial: String) async throws -> String {
        guard certificateExists(path) else {
            throw AndroidToolsError.certificateMissing(path: path)
        }

        let outcome = try await runner.run(["-s", serial, "push", path, Self.remotePath])

        guard outcome.succeeded else {
            throw AndroidToolsError.commandFailed(
                command: "push",
                status: outcome.status,
                reason: outcome.failureReason
            )
        }

        return Self.remotePath
    }
}

/// What to tap, once the certificate is on the device.
public enum CertificateInstructions {
    /// The steps, shortest useful form.
    ///
    /// The exact wording of the Settings path moves between Android versions,
    /// which is why the search term is given as well: it is the part that does
    /// not go stale.
    public static let steps = [
        "Settings ▸ Security ▸ Encryption & credentials ▸ Install a certificate ▸ CA certificate",
        "(or search Settings for \"CA certificate\")",
        "Choose proxylab-ca.crt from Downloads, and accept the warning.",
    ]

    /// The part that has to be said out loud.
    public static let limitation =
        "HTTP is captured either way. For HTTPS, only apps that trust user-installed CAs will "
            + "be readable — browsers usually will; apps targeting Android 7 or later will not "
            + "unless they opt in, and a pinned app never will."
}
