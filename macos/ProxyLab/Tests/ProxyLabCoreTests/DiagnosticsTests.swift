//  Sanitizing a diagnostic, aggregating the state the app already has, and
//  keeping only the recent problems.

import Foundation
import SimulatorTools
import SwiftUI
import Testing
import TransparentProxyKit

@testable import ProxyLabCore

private let identity = DiagnosticsIdentity(
    appVersion: "test",
    macosVersion: "macOS test",
    architecture: "arm64"
)

@Suite("Diagnostic text")
struct DiagnosticsSanitizerTests {
    @Test("a URL's credentials and query do not survive")
    func url() {
        let line = DiagnosticsSanitizer.sanitize(
            "upstream failed https://alice:hunter2@example.com/a?token=secret"
        )

        #expect(!line.contains("alice"))
        #expect(!line.contains("hunter2"))
        #expect(!line.contains("token"))
        #expect(!line.contains("secret"))
        #expect(line.contains("[redacted]@example.com"))
        #expect(!line.contains("?"))
    }

    @Test("an authorization header and a cookie do not survive")
    func headers() {
        let authorization = DiagnosticsSanitizer.sanitize(
            "saw Authorization: Bearer abc.def.ghi in a message"
        )
        let cookie = DiagnosticsSanitizer.sanitize("Cookie: session=abc123")

        #expect(!authorization.contains("abc.def"))
        #expect(authorization.contains("[redacted]"))
        #expect(!cookie.contains("abc123"))
    }

    @Test("a password assignment, a private key, and a home path do not survive")
    func secrets() {
        let password = DiagnosticsSanitizer.sanitize("login password=hunter2 failed")
        let key = DiagnosticsSanitizer.sanitize(
            "before\n-----BEGIN PRIVATE KEY-----\nMIIEsecret\n-----END PRIVATE KEY-----\nafter"
        )
        let path = DiagnosticsSanitizer.sanitize(
            "could not open /Users/ada/.proxylab/traffic/traffic.db: permission denied"
        )
        let caKey = DiagnosticsSanitizer.sanitize("read /Users/ada/.proxylab/ca/ca-key.pem")

        #expect(password.contains("password=[redacted]"))
        #expect(!password.contains("hunter2"))
        #expect(!key.contains("MIIE"))
        #expect(!key.contains("PRIVATE KEY"))
        #expect(key.contains("before"))
        #expect(key.contains("after"))
        #expect(!path.contains("ada"))
        #expect(path.contains("[path]"))
        #expect(path.contains("permission denied"))
        #expect(caKey == "read [redacted]")
    }

    @Test("an ordinary failure is kept, and sanitizing twice changes nothing")
    func ordinary() {
        let line = DiagnosticsSanitizer.sanitize("connect failed: connection refused")
        #expect(line == "connect failed: connection refused")

        let once = DiagnosticsSanitizer.sanitize(
            "http://alice:pw@example.com/a?token=1 Cookie: a=b /home/ada/x"
        )
        #expect(DiagnosticsSanitizer.sanitize(once) == once)
    }

    @Test("a long message is cut off")
    func longMessage() {
        let line = DiagnosticsSanitizer.sanitize(String(repeating: "a", count: 500))
        #expect(line.count <= DiagnosticsSanitizer.maxMessageChars + 1)
        #expect(line.hasSuffix("…"))
    }
}

@Suite("Recent diagnostic problems")
struct RecentDiagnosticsTests {
    @Test("the newest are kept and a repeat is not stored twice")
    func bound() {
        var recent = RecentDiagnostics()
        for number in 0..<40 {
            recent.record(
                level: "error",
                component: "upstream",
                operation: "connect",
                message: "failure \(number)",
                atMs: UInt64(number)
            )
        }

        #expect(recent.entries.count == RecentDiagnostics.limit)
        #expect(recent.entries.first?.message == "failure 39")
        #expect(!recent.entries.contains { $0.message == "failure 7" })
        #expect(recent.entries.contains { $0.message == "failure 8" })

        recent.record(
            level: "error",
            component: "upstream",
            operation: "connect",
            message: "failure 39",
            atMs: 100
        )
        #expect(recent.entries.count == RecentDiagnostics.limit)
        #expect(recent.entries.first?.atMs == 39)
    }

    @Test("a secret is not what gets kept")
    func secret() {
        var recent = RecentDiagnostics()
        recent.record(
            level: "error",
            component: "storage",
            operation: "open",
            message: "open /Users/ada/.proxylab/traffic.db password=hunter2",
            atMs: 1
        )

        let message = recent.entries[0].message
        #expect(!message.contains("ada"))
        #expect(!message.contains("hunter2"))
        #expect(message.contains("[path]"))
        #expect(message.contains("[redacted]"))
    }
}

@Suite("Diagnostic status")
struct DiagnosticsStatusTests {
    @Test("a healthy core is one report, without the LAN interface list")
    func healthy() {
        let snapshot = DiagnosticsPresentation.make(
            identity: identity,
            connection: .connected,
            core: CoreDiagnostics(
                version: "0.1.0",
                core: "running",
                localhost: DiagnosticEndpoint(state: "listening", address: "127.0.0.1:9090"),
                transparent: DiagnosticEndpoint(state: "listening", address: "127.0.0.1:9091"),
                lan: LanStatusResult(
                    enabled: true,
                    address: "192.168.1.42",
                    port: 9090,
                    available: [LanAddress(interface: "secret-iface", address: "192.168.1.42")]
                ),
                storage: DiagnosticComponent(state: "open"),
                ipc: DiagnosticComponent(state: "serving"),
                recent: []
            ),
            networkExtension: NetworkExtensionDiagnosticInput(readiness: .unsigned),
            simulator: SimulatorDiagnosticInput(
                hasRefreshed: false,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            ),
            android: AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: false,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            ),
            localProblems: []
        )
        let report = snapshot.report

        #expect(report.contains("App version: test"))
        #expect(report.contains("Core: running"))
        #expect(report.contains("listening 127.0.0.1:9090"))
        #expect(report.contains("listening 127.0.0.1:9091"))
        #expect(report.contains("on 192.168.1.42:9090"))
        #expect(!report.contains("secret-iface"))
        #expect(report.contains("connected (core serving)"))
        #expect(report.contains("Storage: open"))
        #expect(report.contains("Status: unsupported"))
        #expect(report.contains("Tool: not checked"))
        #expect(report.contains("ADB: available"))
        #expect(report.contains("Discovery: not checked"))
        #expect(!report.contains("Current CA SHA-256"))
    }

    @Test("the current CA is reported by its fingerprint, not by a path")
    func certificateTrust() {
        let snapshot = DiagnosticsPresentation.make(
            identity: identity,
            connection: .connected,
            core: nil,
            networkExtension: NetworkExtensionDiagnosticInput(readiness: .unsigned),
            simulator: SimulatorDiagnosticInput(
                hasRefreshed: false,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            ),
            android: AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: false,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            ),
            trust: CaptureTrustDiagnostic(
                sha256: "AA:BB:CC",
                exactMatchInstalled: false,
                trusted: false
            ),
            localProblems: []
        )

        #expect(snapshot.report.contains("Current CA SHA-256: AA:BB:CC"))
        #expect(snapshot.report.contains("Installed exact match: no"))
        #expect(snapshot.report.contains("Trusted: no"))
        #expect(!snapshot.report.contains("/Users"))
        #expect(!snapshot.report.contains("ca-key"))
    }

    @Test("a secret in a core message does not reach the report")
    func secret() {
        let snapshot = DiagnosticsPresentation.make(
            identity: identity,
            connection: .connected,
            core: CoreDiagnostics(
                version: "0.1.0",
                core: "running",
                localhost: DiagnosticEndpoint(state: "listening", address: "127.0.0.1:9090"),
                transparent: DiagnosticEndpoint(
                    state: "unavailable",
                    message: "could not bind /Users/ada/.proxylab/bridge"
                ),
                storage: DiagnosticComponent(state: "open"),
                ipc: DiagnosticComponent(state: "serving"),
                recent: [
                    DiagnosticEvent(
                        level: "error",
                        component: "upstream",
                        operation: "connect",
                        category: "connection",
                        message: "failed https://alice:hunter2@example.com/a?token=secret",
                        atMs: 2
                    )
                ]
            ),
            networkExtension: NetworkExtensionDiagnosticInput(
                readiness: .ready,
                lastError: "Authorization: Bearer abc.def"
            ),
            simulator: SimulatorDiagnosticInput(
                hasRefreshed: true,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            ),
            android: AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: true,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            ),
            localProblems: []
        )

        let report = snapshot.report
        #expect(!report.contains("hunter2"))
        #expect(!report.contains("alice"))
        #expect(!report.contains("token"))
        #expect(!report.contains("secret"))
        #expect(!report.contains("ada"))
        #expect(!report.contains("abc.def"))
        #expect(report.contains("[path]"))
        #expect(report.contains("[redacted]"))
        #expect(report.contains("upstream/connect"))
    }

    @Test("network extension states stay the ones the system already reports")
    func networkExtension() {
        #expect(
            DiagnosticsPresentation.networkExtension(
                NetworkExtensionDiagnosticInput(readiness: .unsigned)
            ).title == "unsupported"
        )
        #expect(
            DiagnosticsPresentation.networkExtension(
                NetworkExtensionDiagnosticInput(readiness: .notAnAppBundle)
            ).title == "unsupported"
        )
        #expect(
            DiagnosticsPresentation.networkExtension(
                NetworkExtensionDiagnosticInput(
                    readiness: .ready,
                    snapshot: TransparentProxySnapshot(
                        extensionState: .notInstalled,
                        tunnel: .notConfigured
                    )
                )
            ).title == "not installed"
        )
        #expect(
            DiagnosticsPresentation.networkExtension(
                NetworkExtensionDiagnosticInput(
                    readiness: .ready,
                    snapshot: TransparentProxySnapshot(extensionState: .installed, tunnel: .off)
                )
            ).title == "not enabled"
        )
        #expect(
            DiagnosticsPresentation.networkExtension(
                NetworkExtensionDiagnosticInput(
                    readiness: .ready,
                    snapshot: TransparentProxySnapshot(extensionState: .installed, tunnel: .on),
                    bridge: .reachable
                )
            ).title == "ready"
        )
        let failed = DiagnosticsPresentation.networkExtension(
            NetworkExtensionDiagnosticInput(
                readiness: .ready,
                snapshot: TransparentProxySnapshot(extensionState: .installed, tunnel: .on),
                bridge: .unreachable,
                bridgeFailure: "connection refused"
            )
        )
        #expect(failed.title == "error")
        #expect(failed.detail.contains("connection refused"))

        let report = DiagnosticsPresentation.make(
            identity: identity,
            connection: .connected,
            core: nil,
            networkExtension: NetworkExtensionDiagnosticInput(
                readiness: .ready,
                snapshot: TransparentProxySnapshot(extensionState: .installed, tunnel: .on),
                bridge: .reachable,
                flows: .init(
                    claimedTcp80: 2,
                    claimedTcp443: 4,
                    bypassedPort: 0,
                    bypassedUdp: 9,
                    bypassedLoopback: 1,
                    bypassedProxyLab: 1,
                    bridgeFallback: 0
                )
            ),
            simulator: SimulatorDiagnosticInput(
                hasRefreshed: false,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            ),
            android: AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: false,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            ),
            localProblems: []
        )
        #expect(report.report.contains("Claimed TCP :443: 4"))
        #expect(report.report.contains("Claimed TCP :80: 2"))
        #expect(report.report.contains("Bypassed UDP/QUIC: 9"))
        #expect(report.report.contains("Bypassed loopback: 1"))
        #expect(report.report.contains("Bypassed core process: 1"))
        #expect(report.report.contains("Bridge fallback: 0"))
        #expect(report.report.contains("UDP, including QUIC on port 443, is not delivered"))
    }

    @Test("a system extension failure keeps its domain and code in the report")
    func systemExtensionDetail() {
        let snapshot = DiagnosticsPresentation.make(
            identity: identity,
            connection: .connected,
            core: nil,
            networkExtension: NetworkExtensionDiagnosticInput(
                readiness: .ready,
                snapshot: TransparentProxySnapshot(extensionState: .installed, tunnel: .off),
                lastError: "macOS could not validate the extension. If it is already installed, press Turn On again.",
                systemDetail: "OSSystemExtensionErrorDomain 9: Extension validation failed underlying NSOSStatusErrorDomain 4: validation status"
            ),
            simulator: SimulatorDiagnosticInput(
                hasRefreshed: false,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            ),
            android: AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: false,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            ),
            localProblems: []
        )

        #expect(snapshot.report.contains("could not validate"))
        #expect(!snapshot.report.contains("rejected the extension's signature"))
        #expect(snapshot.report.contains("OSSystemExtensionErrorDomain 9"))
        #expect(snapshot.report.contains("Extension validation failed"))
        #expect(snapshot.report.contains("NSOSStatusErrorDomain 4"))
    }

    @Test("simulator and emulator lines come from the session state")
    func devices() {
        let connected = DiagnosticsPresentation.simulator(
            SimulatorDiagnosticInput(
                hasRefreshed: true,
                bootedCount: 1,
                selectedLabel: "iPhone 17",
                state: .connected(udid: "UDID-SHOULD-NOT-BE-REQUIRED"),
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            )
        )
        #expect(connected.lines.contains { $0.title == "Certificate" && $0.value == "trusted" })
        #expect(connected.lines.contains { $0.title == "Proxy route" && $0.value.contains("127.0.0.1:9090") })

        let transparent = DiagnosticsPresentation.simulator(
            SimulatorDiagnosticInput(
                hasRefreshed: true,
                bootedCount: 1,
                selectedLabel: "iPhone 17 Pro Max",
                state: .connected(udid: "A"),
                note: SimulatorNote(
                    text: "iPhone 17 Pro Max trusts ProxyLab's CA and is captured through This Mac / Transparent Capture.",
                    isFailure: false
                ),
                proxyAddress: "127.0.0.1:9090",
                trafficPath: .transparentCapture
            )
        )
        #expect(transparent.lines.contains { $0.title == "Certificate" && $0.value == "trusted" })
        #expect(transparent.lines.contains {
            $0.title == "Proxy route" && $0.value == "transparent capture"
        })
        #expect(!transparent.lines.contains { $0.value.contains("127.0.0.1:9090") })
        #expect(connected.lines.contains { $0.title == "Selected" && $0.value == "iPhone 17" })
        #expect(connected.problem == nil)

        let missingTools = DiagnosticsPresentation.simulator(
            SimulatorDiagnosticInput(
                hasRefreshed: true,
                bootedCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: SimulatorNote(
                    text: SimulatorToolsError.commandLineToolsUnavailable("xcrun").message,
                    isFailure: true
                ),
                proxyAddress: "127.0.0.1:9090"
            )
        )
        #expect(missingTools.lines.contains { $0.title == "Tool" && $0.value == "unavailable" })
        #expect(missingTools.problem?.contains("command line tools") == true)

        let trusted = DiagnosticsPresentation.simulator(
            SimulatorDiagnosticInput(
                hasRefreshed: true,
                bootedCount: 1,
                selectedLabel: "iPhone 17",
                state: .certificateTrusted(udid: "abc"),
                note: nil,
                proxyAddress: "127.0.0.1:9090"
            )
        )
        #expect(trusted.lines.contains { $0.title == "Proxy route" && $0.value == "not routed to ProxyLab" })

        let applied = DiagnosticsPresentation.android(
            AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: true,
                runningCount: 1,
                selectedLabel: "Pixel — emulator-5554",
                state: .connected(serial: "emulator-5554"),
                note: nil,
                proxyAddress: "10.0.2.2:9090"
            )
        )
        #expect(applied.lines.contains { $0.title == "Proxy" && $0.value.contains("applied") })
        #expect(applied.lines.contains { $0.title == "Discovery" && $0.value == "1 running" })

        let missingAdb = DiagnosticsPresentation.android(
            AndroidDiagnosticInput(
                adbAvailable: false,
                hasRefreshed: true,
                runningCount: 0,
                selectedLabel: nil,
                state: .notConnected,
                note: nil,
                proxyAddress: "10.0.2.2:9090",
                adbMissingMessage: "adb could not be found"
            )
        )
        #expect(missingAdb.lines.contains { $0.title == "ADB" && $0.value == "unavailable" })
        #expect(missingAdb.lines.contains { $0.title == "Discovery" && $0.value == "none running" })
        #expect(missingAdb.problem == "adb could not be found")

        let restore = DiagnosticsPresentation.android(
            AndroidDiagnosticInput(
                adbAvailable: true,
                hasRefreshed: true,
                runningCount: 1,
                selectedLabel: "Pixel",
                state: .notConnected,
                note: AndroidNote(
                    text: "Pixel was disconnected, and its previous proxy setting could not be put back: offline",
                    isFailure: true
                ),
                proxyAddress: "10.0.2.2:9090"
            )
        )
        #expect(restore.lines.contains { $0.title == "Proxy" && $0.value == "restore failed" })
        #expect(restore.problem?.contains("could not be put back") == true)
    }

    @Test("a diagnostics response decodes")
    func decodes() throws {
        let json = """
        {"version":1,"request_id":"d1","ok":true,"type":"diagnostics","result":{
          "version":"0.1.0","core":"running",
          "localhost":{"state":"listening","address":"127.0.0.1:9090"},
          "transparent":{"state":"unavailable","message":"the transparent capture bridge is not listening"},
          "lan":{"enabled":false,"port":9090,"available":[]},
          "storage":{"state":"open"},
          "ipc":{"state":"serving"},
          "recent":[{
            "level":"error","component":"upstream","operation":"connect","category":"connection",
            "message":"connection refused","endpoint":"example.com:443","at_ms":9
          }]
        }}
        """
        let message = try JSONDecoder().decode(IPCMessage.self, from: Data(json.utf8))
        guard case let .response(response) = message,
            case let .diagnostics(core) = response.result
        else {
            Issue.record("expected a diagnostics result")
            return
        }

        #expect(core.version == "0.1.0")
        #expect(core.localhost.address == "127.0.0.1:9090")
        #expect(core.transparent.state == "unavailable")
        #expect(core.lan?.enabled == false)
        #expect(core.storage.state == "open")
        #expect(core.ipc.state == "serving")
        #expect(core.recent.first?.endpoint == "example.com:443")
        #expect(core.recent.first?.atMs == 9)
    }
}

@MainActor
@Suite("The diagnostics sheet")
struct DiagnosticsModelTests {
    @Test("copying the report leaves out a secret the core repeated")
    func copy() async {
        let session = ScriptedDiagnostics(
            .success(
                CoreDiagnostics(
                    version: "0.1.0",
                    core: "running",
                    localhost: DiagnosticEndpoint(state: "listening", address: "127.0.0.1:9090"),
                    transparent: DiagnosticEndpoint(state: "listening", address: "127.0.0.1:9091"),
                    storage: DiagnosticComponent(state: "open"),
                    ipc: DiagnosticComponent(state: "serving"),
                    recent: [
                        DiagnosticEvent(
                            level: "error",
                            component: "upstream",
                            operation: "connect",
                            category: "connection",
                            message: "https://alice:hunter2@example.com/a?token=secret",
                            atMs: 1
                        )
                    ]
                )
            )
        )
        let traffic = TrafficInspectorModel(session: FakeSession(), socketPath: "/tmp/proxylab-test.sock")
        await traffic.start()
        let clipboard = FakeClipboard()
        let model = makeModel(session: session, traffic: traffic, clipboard: clipboard)

        await model.refresh(includingDevices: false)
        model.copyReport()

        let report = clipboard.last ?? ""
        #expect(report.contains("127.0.0.1:9090"))
        #expect(report.contains("App version: test"))
        #expect(!report.contains("hunter2"))
        #expect(!report.contains("alice"))
        #expect(!report.contains("token"))
        #expect(!report.contains("/tmp/proxylab-test.sock"))
        #expect(await session.calls == 1)
    }

    @Test("an idle connection is not asked, and a failed ask is kept without its path")
    func failures() async {
        let idle = ScriptedDiagnostics(.failure(.notConnected))
        let idleTraffic = TrafficInspectorModel(session: FakeSession(), socketPath: "/tmp/unused.sock")
        let idleModel = makeModel(session: idle, traffic: idleTraffic, clipboard: FakeClipboard())
        await idleModel.refresh(includingDevices: false)
        #expect(await idle.calls == 0)

        let failing = ScriptedDiagnostics(
            .failure(
                .server(
                    IPCErrorBody(
                        code: .internal,
                        message: "open /Users/ada/.proxylab/traffic.db password=hunter2"
                    )
                )
            )
        )
        let traffic = TrafficInspectorModel(session: FakeSession(), socketPath: "/tmp/unused.sock")
        await traffic.start()
        let clipboard = FakeClipboard()
        let model = makeModel(session: failing, traffic: traffic, clipboard: clipboard)
        await model.refresh(includingDevices: false)
        model.copyReport()

        let report = clipboard.last ?? ""
        #expect(!report.contains("ada"))
        #expect(!report.contains("hunter2"))
        #expect(report.contains("ipc/diagnostics"))
    }

    @Test("refreshing devices reports a missing adb and an unsigned extension")
    func devices() async {
        let session = ScriptedDiagnostics(.failure(.notConnected))
        let traffic = TrafficInspectorModel(session: FakeSession(), socketPath: "/tmp/unused.sock")
        let simulator = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([])),
            trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in true }),
            routes: FakeProxyRoute(route: .direct),
            certificatePath: "/ca.pem"
        )
        let android = AndroidSessionModel(runner: nil, certificatePath: "/ca.pem", certificateExists: { _ in true })
        let lan = FakeLanSession()
        await lan.answerStatus(with: LanFixtures.off)
        let model = DiagnosticsModel(
            session: session,
            traffic: traffic,
            simulator: simulator,
            android: android,
            iPhone: IPhoneSetupModel(session: lan),
            transparent: TransparentProxyModel(
                control: FakeTransparentProxyControl(readiness: .unsigned)
            ),
            clipboard: FakeClipboard(),
            identity: identity
        )

        await model.refresh(includingDevices: true)
        let report = model.snapshot.report

        #expect(report.contains("ADB: unavailable"))
        #expect(report.contains("none booted"))
        #expect(report.contains("unsupported"))
        #expect(!report.contains("/ca.pem"))
        #expect(await session.calls == 0)
    }

    @Test("the sheet lays out")
    func draws() {
        let model = makeModel(
            session: ScriptedDiagnostics(.failure(.notConnected)),
            traffic: TrafficInspectorModel(session: FakeSession(), socketPath: "/tmp/unused.sock"),
            clipboard: FakeClipboard()
        )
        let renderer = ImageRenderer(content: DiagnosticsView(model: model).frame(width: 520, height: 480))
        renderer.scale = 1
        #expect((renderer.nsImage?.size.height ?? 0) > 0)
    }

    private func makeModel(
        session: any DiagnosticsSession,
        traffic: TrafficInspectorModel,
        clipboard: FakeClipboard
    ) -> DiagnosticsModel {
        DiagnosticsModel(
            session: session,
            traffic: traffic,
            simulator: SimulatorSessionModel(
                catalog: SimulatorCatalog(runner: FakeSimctl.listing([])),
                trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in true }),
                routes: FakeProxyRoute(route: .direct),
                certificatePath: "/ca.pem"
            ),
            android: AndroidSessionModel(
                runner: nil,
                certificatePath: "/ca.pem",
                certificateExists: { _ in true }
            ),
            iPhone: IPhoneSetupModel(session: FakeLanSession()),
            transparent: TransparentProxyModel(
                control: FakeTransparentProxyControl(readiness: .unsigned)
            ),
            clipboard: clipboard,
            identity: identity
        )
    }
}

/// A diagnostics answer a test decided.
private actor ScriptedDiagnostics: DiagnosticsSession {
    private(set) var calls = 0
    private let result: Result<CoreDiagnostics, IPCError>

    init(_ result: Result<CoreDiagnostics, IPCError>) {
        self.result = result
    }

    func diagnostics() async throws -> CoreDiagnostics {
        calls += 1
        return try result.get()
    }
}
