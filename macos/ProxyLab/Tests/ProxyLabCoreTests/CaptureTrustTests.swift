//  Whether the current CA is trusted, without touching this Mac's keychains.
//
//  The certificates are generated into a temporary directory and deleted
//  afterwards. The trust store is a stand-in. `SecurityInterceptionTrustStore`
//  is the production store and is not constructed here.

import Foundation
import SimulatorTools
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Certificate trust")
struct CaptureTrustTests {
    @Test("the Mac trust check and a simulator are handed the same CA file")
    func sameCertificateFile() {
        let path = CoreLocations.caCertificate(environment: ["HOME": "/Users/dev"])

        #expect(path == "/Users/dev/.proxylab/ca/ca-cert.pem")
    }

    @Test("an exact CA that is already trusted is ready, and nothing is installed")
    func alreadyTrusted() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.table[fixture.der] = .trusted
        var asked = false
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: {
                asked = true
                return true
            }
        )
        var activities: [String?] = []
        trust.onActivity = { activities.append($0) }

        let readiness = await trust.readiness()

        #expect(readiness == .ready)
        #expect(store.installs.isEmpty)
        #expect(asked == false)
        #expect(activities == [CaptureTrustWording.checking, nil])
        #expect(trust.diagnostic.trusted == true)
        #expect(trust.diagnostic.exactMatchInstalled == true)
        #expect(trust.diagnostic.sha256 == CaptureCertificateReader.fingerprint(of: fixture.der))
    }

    @Test("an absent CA is installed and is ready only after evaluation trusts it")
    func absentThenTrusted() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.trustAfterInstall = .trusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .ready)
        #expect(store.installs == [fixture.der])
        #expect(store.lookups.last == fixture.der)
        #expect(trust.diagnostic.trusted == true)
    }

    @Test("a certificate that is present and untrusted still needs a trust operation")
    func presentUntrusted() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.table[fixture.der] = .presentUntrusted
        store.trustAfterInstall = .trusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .ready)
        #expect(store.installs == [fixture.der])
    }

    @Test("a different certificate with the same name does not satisfy the current CA")
    func staleCertificate() async throws {
        let current = try TrustTestCertificate.issue(commonName: "ProxyLab Development CA")
        let stale = try TrustTestCertificate.issue(commonName: "ProxyLab Development CA")
        defer {
            current.cleanup()
            stale.cleanup()
        }
        #expect(current.der != stale.der)
        #expect(
            CaptureCertificateReader.fingerprint(of: current.der)
                != CaptureCertificateReader.fingerprint(of: stale.der)
        )

        let store = RecordingTrustStore()
        store.table[stale.der] = .trusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { current.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .ready)
        #expect(store.lookups.allSatisfy { $0 == current.der })
        #expect(!store.lookups.contains(stale.der))
        #expect(store.installs == [current.der])
    }

    @Test("a cancelled trust operation is not ready")
    func cancelled() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.result = .cancelled
        var activities: [String?] = []
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )
        trust.onActivity = { activities.append($0) }

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.cancelledDetail))
        #expect(store.installs == [fixture.der])
        #expect(activities.contains(CaptureTrustWording.needsTrust))
        #expect(activities.contains(CaptureTrustWording.installing))
        #expect(activities.last == CaptureTrustWording.cancelled)
    }

    @Test("a failed trust operation is not ready")
    func failed() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.result = .failed
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.failedDetail))
        #expect(store.installs == [fixture.der])
    }

    @Test("an install that claims success is not ready when evaluation still does not trust it")
    func unverified() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.trustAfterInstall = .presentUntrusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.unverifiedDetail))
        #expect(store.installs == [fixture.der])
        #expect(trust.diagnostic.trusted == false)
    }

    @Test("a malformed CA is not ready and is not installed")
    func malformed() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("proxylab-trust-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let pem = directory.appendingPathComponent("ca-cert.pem")
        try Data("not a certificate".utf8).write(to: pem)
        let key = directory.appendingPathComponent("ca-key.pem")
        try Data("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n".utf8).write(to: key)

        let store = RecordingTrustStore()
        let trust = MacOSCaptureTrust(
            certificateLocator: { pem.path },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.malformedDetail))
        #expect(store.installs.isEmpty)
        #expect(store.lookups.isEmpty)
        #expect(CaptureCertificateReader.load(at: key.path) == .malformed)
    }

    @Test("a missing CA is not ready and is not installed")
    func missing() async {
        let store = RecordingTrustStore()
        let missing = MacOSCaptureTrust(
            certificateLocator: { "/tmp/proxylab-trust-no-such-ca.pem" },
            store: store,
            authorizationPermitted: { true }
        )
        let unknown = MacOSCaptureTrust(
            certificateLocator: { nil },
            store: store,
            authorizationPermitted: { true }
        )

        #expect(await missing.readiness() == .notReady(CaptureTrustWording.missingDetail))
        #expect(await unknown.readiness() == .notReady(CaptureTrustWording.missingDetail))
        #expect(store.installs.isEmpty)
    }

    @Test("a changed CA fingerprint has to be trusted again")
    func fingerprintChanges() async throws {
        let first = try TrustTestCertificate.issue()
        let second = try TrustTestCertificate.issue()
        defer {
            first.cleanup()
            second.cleanup()
        }
        let location = CertificateLocation(first.pemPath)
        let store = RecordingTrustStore()
        store.table[first.der] = .trusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { location.path },
            store: store,
            authorizationPermitted: { true }
        )

        #expect(await trust.readiness() == .ready)
        #expect(store.installs.isEmpty)

        location.path = second.pemPath
        store.trustAfterInstall = .trusted
        #expect(await trust.readiness() == .ready)
        #expect(store.installs == [second.der])
        #expect(trust.diagnostic.sha256 == CaptureCertificateReader.fingerprint(of: second.der))
    }

    @Test("a certificate that changes during installation is not treated as trusted")
    func replacedDuringInstall() async throws {
        let first = try TrustTestCertificate.issue()
        let second = try TrustTestCertificate.issue()
        defer {
            first.cleanup()
            second.cleanup()
        }
        var paths = [first.pemPath, second.pemPath]
        let store = RecordingTrustStore()
        store.trustAfterInstall = .trusted
        let trust = MacOSCaptureTrust(
            certificateLocator: {
                guard !paths.isEmpty else { return second.pemPath }
                return paths.removeFirst()
            },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.unverifiedDetail))
        #expect(store.installs == [first.der])
    }

    @Test("authorization is not requested when a prompt is not allowed")
    func promptRefused() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { false }
        )

        let readiness = await trust.readiness()

        #expect(readiness == .notReady(CaptureTrustWording.needsTrustDetail))
        #expect(store.installs.isEmpty)
    }

    @Test("trusting the CA in a simulator does not make the Mac trust check ready")
    func simulatorIsIndependent() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let runner = FakeSimctl()
        let simulator = SimulatorTrustStore(runner: runner, certificateExists: { _ in true })
        try await simulator.trustRootCertificate(at: fixture.pemPath, on: "UDID-1")

        let store = RecordingTrustStore()
        store.result = .cancelled
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        let readiness = await trust.readiness()

        #expect(runner.invocations == [["keychain", "UDID-1", "add-root-cert", fixture.pemPath]])
        #expect(readiness == .notReady(CaptureTrustWording.cancelledDetail))
        #expect(store.installs == [fixture.der])
    }

    @Test("refreshing the diagnostic reads the CA and does not install it")
    func refreshDoesNotInstall() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let store = RecordingTrustStore()
        store.table[fixture.der] = .presentUntrusted
        let trust = MacOSCaptureTrust(
            certificateLocator: { fixture.pemPath },
            store: store,
            authorizationPermitted: { true }
        )

        trust.refreshDiagnostic()

        #expect(store.installs.isEmpty)
        #expect(trust.diagnostic.exactMatchInstalled == true)
        #expect(trust.diagnostic.trusted == false)
        #expect(trust.diagnostic.sha256 == CaptureCertificateReader.fingerprint(of: fixture.der))
    }

    @Test("text around a PEM certificate does not change its identity")
    func pemFraming() async throws {
        let fixture = try TrustTestCertificate.issue()
        defer { fixture.cleanup() }
        let pem = try String(contentsOfFile: fixture.pemPath, encoding: .utf8)
        let wrapped = fixture.directory.appendingPathComponent("wrapped.pem")
        try Data("ProxyLab CA\n\(pem)\n".utf8).write(to: wrapped)

        #expect(CaptureCertificateReader.load(at: wrapped.path) == .certificate(fixture.der))
    }
}

/// The path a trust check reads. A class so a later change is the next
/// read, without mutating a value the closure captured.
@MainActor
final class CertificateLocation {
    var path: String

    init(_ path: String) {
        self.path = path
    }
}

@MainActor
final class RecordingTrustStore: CaptureTrustStoring {
    var table: [Data: CaptureTrustMatch] = [:]
    var result: CaptureTrustInstallResult = .installed
    var trustAfterInstall: CaptureTrustMatch = .trusted
    private(set) var installs: [Data] = []
    private(set) var lookups: [Data] = []

    func match(exactDER: Data) -> CaptureTrustMatch {
        lookups.append(exactDER)
        return table[exactDER] ?? .absent
    }

    func installTrust(exactDER: Data) -> CaptureTrustInstallResult {
        installs.append(exactDER)
        if result == .installed {
            table[exactDER] = trustAfterInstall
        }
        return result
    }
}

enum TrustTestCertificate {
    struct Issued {
        let directory: URL
        let pemPath: String
        let der: Data

        func cleanup() {
            try? FileManager.default.removeItem(at: directory)
        }
    }

    static func issue(commonName: String = "ProxyLab Development CA") throws -> Issued {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("proxylab-trust-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let pem = directory.appendingPathComponent("ca-cert.pem")
        let key = directory.appendingPathComponent("ca-key.pem")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/openssl")
        process.arguments = [
            "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "30", "-nodes",
            "-keyout", key.path,
            "-out", pem.path,
            "-subj", "/CN=\(commonName)/O=ProxyLab",
        ]
        let errors = Pipe()
        process.standardOutput = Pipe()
        process.standardError = errors
        do {
            try process.run()
        } catch {
            try? FileManager.default.removeItem(at: directory)
            throw error
        }
        process.waitUntilExit()
        let err = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        guard process.terminationStatus == 0 else {
            try? FileManager.default.removeItem(at: directory)
            throw TrustFixtureError.openssl(status: process.terminationStatus, detail: err)
        }
        guard case let .certificate(der) = CaptureCertificateReader.load(at: pem.path) else {
            try? FileManager.default.removeItem(at: directory)
            throw TrustFixtureError.unreadable
        }
        return Issued(directory: directory, pemPath: pem.path, der: der)
    }
}

private enum TrustFixtureError: Error, CustomStringConvertible {
    case openssl(status: Int32, detail: String)
    case unreadable

    var description: String {
        switch self {
        case let .openssl(status, detail):
            "openssl exited \(status): \(detail)"
        case .unreadable:
            "the generated certificate could not be parsed"
        }
    }
}
