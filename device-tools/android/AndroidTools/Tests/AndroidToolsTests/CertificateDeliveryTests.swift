//  Getting the CA onto the emulator.

import Foundation
import Testing

@testable import AndroidTools

@Suite("Delivering the CA")
struct CertificateDeliveryTests {
    @Test("it pushes the certificate to Downloads, as a .crt")
    func pushes() async throws {
        let adb = FakeAdb()
        let delivery = CertificateDelivery(runner: adb, certificateExists: { _ in true })

        let landed = try await delivery.deliver(
            certificateAt: "/home/dev/.proxylab/ca/ca-cert.pem",
            to: "emulator-5554"
        )

        #expect(landed == "/sdcard/Download/proxylab-ca.crt")
        #expect(adb.invocations == [[
            "-s", "emulator-5554", "push",
            "/home/dev/.proxylab/ca/ca-cert.pem", "/sdcard/Download/proxylab-ca.crt",
        ]])
    }

    @Test("a CA that has not been generated is caught before adb is run")
    func missingCertificate() async {
        let adb = FakeAdb()
        let delivery = CertificateDelivery(runner: adb, certificateExists: { _ in false })

        await #expect(throws: AndroidToolsError.certificateMissing(path: "/gone/ca-cert.pem")) {
            try await delivery.deliver(certificateAt: "/gone/ca-cert.pem", to: "emulator-5554")
        }
        #expect(adb.invocations.isEmpty)
    }

    @Test("nothing here installs or trusts anything, and the wording says so")
    func saysWhatItCannotDo() {
        // The one claim this step must not make.
        #expect(CertificateInstructions.limitation.contains("HTTP is captured either way"))
        #expect(CertificateInstructions.limitation.contains("pinned"))
        #expect(CertificateInstructions.steps.contains { $0.contains("CA certificate") })
    }
}
