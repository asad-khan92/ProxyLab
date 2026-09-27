//  Making a simulator trust ProxyLab's CA.
//
//      xcrun simctl keychain <udid> add-root-cert <path>
//
//  This is the whole of it, and it is Apple's own documented command — `simctl
//  help keychain` lists `add-root-cert`, `add-cert`, and `reset`. Nothing here
//  writes into the simulator's filesystem, edits a trust store by hand, or
//  touches the macOS Keychain.
//
//  # It can be run again
//
//  `add-root-cert` exits zero for a certificate the simulator already trusts,
//  so connecting a simulator twice is not an error and does not add the
//  certificate twice. That is what makes Connect safe to press repeatedly, and
//  it is why there is no "is it already installed?" query here — `simctl` has
//  no such query, and guessing at one would mean reading the simulator's trust
//  store behind Apple's back.
//
//  # Nothing here removes it
//
//  `simctl keychain` can add a certificate or `reset` the entire keychain.
//  There is no remove-one-certificate, and resetting would throw away every
//  other credential in that simulator — which is not undoing what ProxyLab
//  did, it is undoing rather more. So disconnecting leaves the certificate
//  trusted and says so.

import Foundation

/// Installs ProxyLab's root certificate into a simulator's trusted roots.
public struct SimulatorTrustStore: Sendable {
    private let runner: SimulatorCommandRunner
    private let certificateExists: @Sendable (String) -> Bool

    public init(
        runner: SimulatorCommandRunner = SimctlRunner(),
        certificateExists: @escaping @Sendable (String) -> Bool = { path in
            FileManager.default.fileExists(atPath: path)
        }
    ) {
        self.runner = runner
        self.certificateExists = certificateExists
    }

    /// Trusts the certificate at `path` in the simulator with `udid`.
    ///
    /// Checked before it is run rather than after it has failed: `simctl`'s
    /// own message for a missing file names the file and not what the file was
    /// for, and "ProxyLab's CA certificate is not there yet" is a different
    /// problem with a different fix.
    public func trustRootCertificate(at path: String, on udid: String) async throws {
        guard certificateExists(path) else {
            throw SimulatorToolsError.certificateMissing(path: path)
        }

        let outcome = try await runner.run(["keychain", udid, "add-root-cert", path])

        guard outcome.succeeded else {
            throw SimulatorToolsError.commandFailed(
                command: "keychain add-root-cert",
                status: outcome.status,
                reason: outcome.standardError
            )
        }
    }
}
