//  Trusting ProxyLab's CA in a simulator.
//
//  What is asserted is the command: `simctl keychain <udid> add-root-cert
//  <path>`, Apple's own, with nothing else beside it. A test cannot show that
//  a simulator trusts a certificate afterwards — that is what the manual
//  verification is for — but it can show that this is the only thing ProxyLab
//  asks for, and that a missing certificate is caught before `simctl` is
//  troubled with it.

import Foundation
import Testing

@testable import SimulatorTools

@Suite("Trusting the CA")
struct SimulatorTrustStoreTests {
    private func store(
        runner: FakeRunner,
        certificateExists: Bool = true
    ) -> SimulatorTrustStore {
        SimulatorTrustStore(runner: runner, certificateExists: { _ in certificateExists })
    }

    @Test("it runs simctl keychain add-root-cert, and nothing else")
    func runsTheDocumentedCommand() async throws {
        let runner = FakeRunner()

        try await store(runner: runner)
            .trustRootCertificate(at: "/home/dev/.proxylab/ca/ca-cert.pem", on: "UDID-1")

        #expect(runner.invocations == [[
            "keychain", "UDID-1", "add-root-cert", "/home/dev/.proxylab/ca/ca-cert.pem",
        ]])
    }

    @Test("running it twice is two of the same command and no complaint")
    func isIdempotent() async throws {
        // `simctl` exits zero for a certificate the simulator already trusts,
        // which is what makes Connect safe to press again.
        let runner = FakeRunner()
        let store = store(runner: runner)

        try await store.trustRootCertificate(at: "/ca.pem", on: "UDID-1")
        try await store.trustRootCertificate(at: "/ca.pem", on: "UDID-1")

        #expect(runner.invocations.count == 2)
        #expect(runner.invocations[0] == runner.invocations[1])
    }

    @Test("a certificate that is not there is caught before simctl is run")
    func missingCertificate() async {
        let runner = FakeRunner()

        await #expect(throws: SimulatorToolsError.certificateMissing(path: "/gone/ca-cert.pem")) {
            try await store(runner: runner, certificateExists: false)
                .trustRootCertificate(at: "/gone/ca-cert.pem", on: "UDID-1")
        }
        #expect(runner.invocations.isEmpty)
    }

    @Test("simctl's own failure is carried through with its message")
    func commandFailure() async {
        let runner = FakeRunner(failingWith: 148, saying: "Invalid device: UDID-1")

        await #expect(throws: SimulatorToolsError.commandFailed(
            command: "keychain add-root-cert",
            status: 148,
            reason: "Invalid device: UDID-1"
        )) {
            try await store(runner: runner).trustRootCertificate(at: "/ca.pem", on: "UDID-1")
        }
    }

    @Test("every message names what to do next")
    func messagesAreActionable() {
        #expect(SimulatorToolsError.noBootedSimulator.message.contains("Boot one"))
        #expect(
            SimulatorToolsError.certificateMissing(path: "/ca.pem").message.contains("/ca.pem")
        )
        #expect(
            SimulatorToolsError.commandLineToolsUnavailable("no such file")
                .message.contains("xcode-select")
        )
    }
}
