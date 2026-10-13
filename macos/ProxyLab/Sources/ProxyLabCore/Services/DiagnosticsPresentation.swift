//  The diagnostics sheet, as values.
//
//  Everything the sheet shows is derived here from state that already exists:
//  the core's `diagnostics` answer, this app's IPC connection, and the device
//  and Network Extension models. Nothing here asks the system, opens a
//  socket, or reads captured traffic. The text a person copies is the same
//  lines the sheet shows, sanitized once more on the way out.

import Foundation
import TransparentProxyKit

/// Who is asking, without anything that identifies the person.
public struct DiagnosticsIdentity: Equatable, Sendable {
    public var appVersion: String
    public var macosVersion: String
    public var architecture: String

    public init(appVersion: String, macosVersion: String, architecture: String) {
        self.appVersion = appVersion
        self.macosVersion = macosVersion
        self.architecture = architecture
    }

    /// This process, as the system will say.
    public static var current: DiagnosticsIdentity {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        let app = (version?.isEmpty == false) ? version! : "development"

        return DiagnosticsIdentity(
            appVersion: app,
            macosVersion: ProcessInfo.processInfo.operatingSystemVersionString,
            architecture: architectureName
        )
    }

    private static var architectureName: String {
        #if arch(arm64)
        "arm64"
        #elseif arch(x86_64)
        "x86_64"
        #else
        "unknown"
        #endif
    }
}

/// One recent problem this process has kept.
public struct DiagnosticProblem: Equatable, Sendable, Identifiable {
    public var level: String
    public var component: String
    public var operation: String
    public var message: String
    public var atMs: UInt64

    public var id: String { "\(atMs)|\(component)|\(operation)|\(message)" }

    public init(level: String, component: String, operation: String, message: String, atMs: UInt64) {
        self.level = level
        self.component = component
        self.operation = operation
        self.message = message
        self.atMs = atMs
    }
}

/// A short list of recent problems. Newest is kept; nothing is written to disk.
public struct RecentDiagnostics: Equatable, Sendable {
    public static let limit = 32

    public private(set) var entries: [DiagnosticProblem] = []

    public init() {}

    /// Keeps `message` when it is not already here.
    ///
    /// The same failure reported on every refresh would otherwise push the
    /// one before it out of a list this small.
    public mutating func record(
        level: String,
        component: String,
        operation: String,
        message: String,
        atMs: UInt64
    ) {
        let message = DiagnosticsSanitizer.sanitize(message)
        guard !message.isEmpty else { return }
        if entries.contains(where: {
            $0.component == component && $0.operation == operation && $0.message == message
        }) {
            return
        }

        entries.insert(
            DiagnosticProblem(
                level: level,
                component: component,
                operation: operation,
                message: message,
                atMs: atMs
            ),
            at: 0
        )
        if entries.count > Self.limit {
            entries.removeLast(entries.count - Self.limit)
        }
    }
}

/// One line of the sheet.
public struct DiagnosticLine: Equatable, Identifiable, Sendable {
    public let id: String
    public let title: String
    public let value: String
    public let isProblem: Bool

    public init(id: String, title: String, value: String, isProblem: Bool) {
        self.id = id
        self.title = title
        self.value = value
        self.isProblem = isProblem
    }
}

/// One group of lines.
public struct DiagnosticSection: Equatable, Identifiable, Sendable {
    public let id: String
    public let title: String
    public let lines: [DiagnosticLine]

    public init(id: String, title: String, lines: [DiagnosticLine]) {
        self.id = id
        self.title = title
        self.lines = lines
    }
}

/// The sheet and the text a person copies, from one set of inputs.
public struct DiagnosticsSnapshot: Equatable, Sendable {
    public let sections: [DiagnosticSection]
    public let report: String

    public init(sections: [DiagnosticSection], report: String) {
        self.sections = sections
        self.report = report
    }
}

/// Where transparent capture stands, in the five states the sheet names.
public enum NetworkExtensionDiagnostic: Equatable, Sendable {
    case unsupported(String)
    case notInstalled(String)
    case notEnabled(String)
    case ready(String)
    case error(String)

    public var title: String {
        switch self {
        case .unsupported: "unsupported"
        case .notInstalled: "not installed"
        case .notEnabled: "not enabled"
        case .ready: "ready"
        case .error: "error"
        }
    }

    public var detail: String {
        switch self {
        case let .unsupported(message), let .notInstalled(message), let .notEnabled(message),
            let .ready(message), let .error(message):
            message
        }
    }

    /// A sentence worth keeping, when this is a failure rather than a status.
    public var problem: (level: String, message: String)? {
        switch self {
        case let .unsupported(message): ("warn", message)
        case let .error(message): ("error", message)
        case .notInstalled, .notEnabled, .ready: nil
        }
    }
}

/// What the Network Extension model already knows.
public struct NetworkExtensionDiagnosticInput: Equatable, Sendable {
    public var readiness: TransparentProxyReadiness
    public var snapshot: TransparentProxySnapshot?
    public var bridge: ProviderStatus.Health?
    public var bridgeFailure: String?
    public var lastError: String?
    /// Domain, code, description, and underlying error for ``lastError``.
    public var systemDetail: String?
    /// Flow decisions since the extension started, when it has reported them.
    public var flows: ProviderStatus.Counters?

    public init(
        readiness: TransparentProxyReadiness,
        snapshot: TransparentProxySnapshot? = nil,
        bridge: ProviderStatus.Health? = nil,
        bridgeFailure: String? = nil,
        lastError: String? = nil,
        systemDetail: String? = nil,
        flows: ProviderStatus.Counters? = nil
    ) {
        self.readiness = readiness
        self.snapshot = snapshot
        self.bridge = bridge
        self.bridgeFailure = bridgeFailure
        self.lastError = lastError
        self.systemDetail = systemDetail
        self.flows = flows
    }
}

/// What the simulator model already knows.
public struct SimulatorDiagnosticInput: Equatable, Sendable {
    public var hasRefreshed: Bool
    public var bootedCount: Int
    public var selectedLabel: String?
    public var state: SimulatorConnection
    public var note: SimulatorNote?
    public var proxyAddress: String
    /// How a connected simulator is captured. Nil when it is not connected
    /// through a route, including when the CA is trusted and nothing routes it.
    public var trafficPath: SimulatorTrafficPath?

    public init(
        hasRefreshed: Bool,
        bootedCount: Int,
        selectedLabel: String?,
        state: SimulatorConnection,
        note: SimulatorNote?,
        proxyAddress: String,
        trafficPath: SimulatorTrafficPath? = nil
    ) {
        self.hasRefreshed = hasRefreshed
        self.bootedCount = bootedCount
        self.selectedLabel = selectedLabel
        self.state = state
        self.note = note
        self.proxyAddress = proxyAddress
        self.trafficPath = trafficPath
    }
}

/// What the emulator model already knows.
public struct AndroidDiagnosticInput: Equatable, Sendable {
    public var adbAvailable: Bool
    public var hasRefreshed: Bool
    public var runningCount: Int
    public var selectedLabel: String?
    public var state: AndroidConnection
    public var note: AndroidNote?
    public var proxyAddress: String
    /// What to say when adb was not found, which is known before a refresh.
    public var adbMissingMessage: String?

    public init(
        adbAvailable: Bool,
        hasRefreshed: Bool,
        runningCount: Int,
        selectedLabel: String?,
        state: AndroidConnection,
        note: AndroidNote?,
        proxyAddress: String,
        adbMissingMessage: String? = nil
    ) {
        self.adbAvailable = adbAvailable
        self.hasRefreshed = hasRefreshed
        self.runningCount = runningCount
        self.selectedLabel = selectedLabel
        self.state = state
        self.note = note
        self.proxyAddress = proxyAddress
        self.adbMissingMessage = adbMissingMessage
    }
}

/// Turns the inputs into the sheet and the copied report.
public enum DiagnosticsPresentation {
    public static func make(
        identity: DiagnosticsIdentity,
        connection: ConnectionState,
        core: CoreDiagnostics?,
        networkExtension: NetworkExtensionDiagnosticInput,
        simulator: SimulatorDiagnosticInput,
        android: AndroidDiagnosticInput,
        trust: CaptureTrustDiagnostic? = nil,
        lifecycle: CaptureLifecycleTrace? = nil,
        localProblems: [DiagnosticProblem]
    ) -> DiagnosticsSnapshot {
        let network = Self.networkExtension(networkExtension)
        let ios = Self.simulator(simulator)
        let android = Self.android(android)
        let problems = merged(core: core, local: localProblems)
        var sections = [
            identitySection(identity),
            coreSection(connection: connection, core: core),
        ]
        if let trust {
            sections.append(trustSection(trust))
        }
        sections.append(contentsOf: [
            networkSection(
                network,
                systemDetail: networkExtension.systemDetail,
                flows: networkExtension.flows
            ),
            deviceSection(id: "ios", title: "iOS Simulator", lines: ios.lines),
            deviceSection(id: "android", title: "Android Emulator", lines: android.lines),
        ])
        if let lifecycle {
            sections.append(lifecycleSection(lifecycle))
        }
        sections.append(recentSection(problems))

        return DiagnosticsSnapshot(sections: sections, report: render(sections))
    }

    public static func networkExtension(
        _ input: NetworkExtensionDiagnosticInput
    ) -> NetworkExtensionDiagnostic {
        if let lastError = input.lastError, !lastError.isEmpty {
            return .error(lastError)
        }

        switch input.readiness {
        case .notAnAppBundle:
            return .unsupported(
                input.readiness.message
                    ?? "Transparent capture needs ProxyLab.app."
            )
        case .extensionMissing:
            return .unsupported(
                input.readiness.message
                    ?? "This ProxyLab.app has no Network Extension in it."
            )
        case .unsigned:
            return .unsupported(
                input.readiness.message
                    ?? "This build is not signed to install a Network Extension."
            )
        case .notInApplications:
            return .error(
                input.readiness.message
                    ?? "macOS only installs Network Extensions from apps in /Applications."
            )
        case .ready:
            break
        }

        guard let snapshot = input.snapshot else {
            return .notEnabled("Not checked yet. Press Refresh.")
        }

        switch snapshot.extensionState {
        case .notInstalled:
            return .notInstalled("Extension not installed. Turn On in This Mac installs it.")
        case .awaitingApproval:
            return .notEnabled("Extension waiting for approval in System Settings.")
        case .willCompleteAfterReboot:
            return .notEnabled("Extension finishes installing after a restart.")
        case .unknown:
            return .notEnabled("Extension state is not known yet.")
        case .installed:
            break
        }

        switch snapshot.tunnel {
        case .notConfigured, .off:
            return .notEnabled("Transparent capture is off.")
        case .starting:
            return .notEnabled("Transparent capture is starting.")
        case .stopping:
            return .notEnabled("Transparent capture is stopping.")
        case .invalid:
            return .error("The saved configuration is not usable. Turn it off, then on again.")
        case .on:
            switch input.bridge {
            case .reachable:
                return .ready("Transparent capture is on, and the core's bridge is reachable.")
            case .unreachable:
                let why = input.bridgeFailure.map { " \($0)" } ?? ""
                return .error("Transparent capture is on, but the core's bridge is not reachable.\(why)")
            case .unknown, .none:
                return .ready("Transparent capture is on. The extension has not reported the bridge yet.")
            }
        }
    }

    public static func simulator(_ input: SimulatorDiagnosticInput) -> DeviceLines {
        let failure = input.note?.isFailure == true ? input.note?.text : nil
        let tool: (String, Bool)
        if failure?.contains("command line tools could not be run") == true {
            tool = ("unavailable", true)
        } else if input.hasRefreshed || input.bootedCount > 0 {
            tool = ("available", false)
        } else {
            tool = ("not checked", false)
        }

        let discovery: (String, Bool)
        if failure?.contains("list of simulators could not be read") == true
            || failure?.contains("command line tools could not be run") == true
        {
            discovery = ("failed", true)
        } else if input.hasRefreshed {
            discovery = (input.bootedCount == 0 ? "none booted" : "\(input.bootedCount) booted", false)
        } else {
            discovery = ("not checked", false)
        }

        let certificate: (String, Bool)
        let route: (String, Bool)
        switch input.state {
        case .connected:
            certificate = ("trusted", false)
            switch input.trafficPath {
            case .transparentCapture:
                route = ("transparent capture", false)
            case .explicitProxy, nil:
                route = ("routed through \(input.proxyAddress)", false)
            }
        case .certificateTrusted:
            certificate = ("trusted", false)
            route = ("not routed to ProxyLab", true)
        case .notConnected, .connecting:
            if failure?.contains("CA certificate is not") == true {
                certificate = ("missing", true)
            } else {
                certificate = ("not checked", false)
            }
            if failure?.contains("proxy configuration could not be read") == true {
                route = ("unreadable", true)
            } else {
                route = ("not connected", false)
            }
        }

        var lines = [
            line("ios", "Tool", tool.0, problem: tool.1),
            line("ios", "Discovery", discovery.0, problem: discovery.1),
            line("ios", "Selected", input.selectedLabel ?? "none", problem: false),
            line("ios", "Certificate", certificate.0, problem: certificate.1),
            line("ios", "Proxy route", route.0, problem: route.1),
        ]
        if let failure {
            lines.append(line("ios", "Detail", failure, problem: true))
        }

        return DeviceLines(lines: lines, problem: failure)
    }

    public static func android(_ input: AndroidDiagnosticInput) -> DeviceLines {
        let failure = input.note?.isFailure == true ? input.note?.text : nil
        let tool = input.adbAvailable ? "available" : "unavailable"
        let discovery: String
        if input.hasRefreshed {
            discovery = input.runningCount == 0 ? "none running" : "\(input.runningCount) running"
        } else {
            discovery = "not checked"
        }

        let proxy: (String, Bool)
        if case .connected = input.state {
            proxy = ("applied — \(input.proxyAddress)", false)
        } else if failure?.contains("could not be put back") == true {
            proxy = ("restore failed", true)
        } else if input.note?.isFailure == false, input.note?.text.contains("disconnected") == true {
            proxy = ("restored", false)
        } else {
            proxy = ("not applied", false)
        }

        var lines = [
            line("android", "ADB", tool, problem: !input.adbAvailable),
            line("android", "Discovery", discovery, problem: false),
            line("android", "Selected", input.selectedLabel ?? "none", problem: false),
            line("android", "Proxy", proxy.0, problem: proxy.1),
        ]
        let problem = failure ?? (input.adbAvailable ? nil : input.adbMissingMessage)
        if let problem {
            lines.append(line("android", "Detail", problem, problem: true))
        }

        return DeviceLines(lines: lines, problem: problem)
    }

    // MARK: - Sections

    private static func identitySection(_ identity: DiagnosticsIdentity) -> DiagnosticSection {
        DiagnosticSection(
            id: "identity",
            title: "ProxyLab",
            lines: [
                line("identity", "App version", identity.appVersion, problem: false),
                line("identity", "macOS", identity.macosVersion, problem: false),
                line("identity", "Architecture", identity.architecture, problem: false),
            ]
        )
    }

    private static func coreSection(
        connection: ConnectionState,
        core: CoreDiagnostics?
    ) -> DiagnosticSection {
        let lines = [
            line("core", "Core version", core?.version ?? "unavailable", problem: core == nil),
            line("core", "Core", core?.core ?? "unavailable", problem: core == nil),
            endpointLine("Localhost", core?.localhost),
            endpointLine("Transparent bridge", core?.transparent),
            lanLine(core),
            connectionLine(connection, core: core),
            storageLine(core),
        ]
        return DiagnosticSection(id: "core", title: "Core", lines: lines)
    }

    private static func networkSection(
        _ diagnostic: NetworkExtensionDiagnostic,
        systemDetail: String?,
        flows: ProviderStatus.Counters?
    ) -> DiagnosticSection {
        let failed = diagnostic.title == "error"
        let notable = diagnostic.problem != nil
        var lines = [
            line("network", "Status", diagnostic.title, problem: failed),
            line("network", "Detail", diagnostic.detail, problem: notable),
        ]
        if let systemDetail, !systemDetail.isEmpty {
            lines.append(line("network", "System error", systemDetail, problem: true))
        }
        if let flows {
            lines.append(line("network", "Capture scope", FlowPolicy.captureScope, problem: false))
            lines.append(contentsOf: flowLines(flows))
        }
        return DiagnosticSection(
            id: "network",
            title: "Network Extension",
            lines: lines
        )
    }

    /// Counts of flows the extension saw. A bypass that the rules never
    /// deliver is not shown as zero: zero would read as "none happened".
    private static func flowLines(_ flows: ProviderStatus.Counters) -> [DiagnosticLine] {
        [
            line("network", "Claimed TCP :443", String(flows.claimedTcp443), problem: false),
            line("network", "Claimed TCP :80", String(flows.claimedTcp80), problem: false),
            line(
                "network",
                "Bypassed other TCP port",
                Self.unobserved(flows.bypassedPort, "other TCP ports are not delivered"),
                problem: false
            ),
            line(
                "network",
                "Bypassed UDP/QUIC",
                Self.unobserved(flows.bypassedUdp, "UDP is not delivered"),
                problem: false
            ),
            line("network", "Bypassed loopback", String(flows.bypassedLoopback), problem: false),
            line("network", "Bypassed core process", String(flows.bypassedProxyLab), problem: false),
            line("network", "Bridge fallback", String(flows.bridgeFallback), problem: flows.bridgeFallback > 0),
            line("network", "Bypassed unreadable", String(flows.bypassedUnreadable), problem: false),
        ]
    }

    /// A count of flows that were seen, or a sentence when none can be seen.
    private static func unobserved(_ count: Int, _ reason: String) -> String {
        count == 0 ? "not observed — \(reason)" : String(count)
    }

    private static func deviceSection(
        id: String,
        title: String,
        lines: [DiagnosticLine]
    ) -> DiagnosticSection {
        DiagnosticSection(id: id, title: title, lines: lines)
    }

    private static func trustSection(_ trust: CaptureTrustDiagnostic) -> DiagnosticSection {
        DiagnosticSection(
            id: "ca",
            title: "Certificate",
            lines: [
                line("ca", "Current CA SHA-256", trust.sha256 ?? "unavailable", problem: false),
                line(
                    "ca",
                    "Installed exact match",
                    yesNo(trust.exactMatchInstalled),
                    problem: trust.exactMatchInstalled == false
                ),
                line("ca", "Trusted", yesNo(trust.trusted), problem: trust.trusted == false),
            ]
        )
    }

    private static func yesNo(_ value: Bool?) -> String {
        switch value {
        case true: "yes"
        case false: "no"
        case nil: "unknown"
        }
    }

    /// The capture stages this process has kept. No bodies, paths, or secrets.
    private static func lifecycleSection(_ trace: CaptureLifecycleTrace) -> DiagnosticSection {
        var lines = [
            line(
                "lifecycle",
                "Stage",
                "\(trace.stage) (\(trace.stageMilliseconds)ms)",
                problem: false
            ),
        ]
        for event in trace.events.suffix(12) {
            lines.append(
                DiagnosticLine(
                    id: "lifecycle.\(event.id)",
                    title: event.stage,
                    value: DiagnosticsSanitizer.sanitize("\(event.milliseconds)ms"),
                    isProblem: false
                )
            )
        }
        return DiagnosticSection(id: "lifecycle", title: "Capture lifecycle", lines: lines)
    }

    private static func recentSection(_ problems: [DiagnosticProblem]) -> DiagnosticSection {
        let lines: [DiagnosticLine]
        if problems.isEmpty {
            lines = [line("recent", "Status", "none", problem: false)]
        } else {
            lines = problems.enumerated().map { index, problem in
                DiagnosticLine(
                    id: "recent.\(index).\(problem.id)",
                    title: problem.level,
                    value: DiagnosticsSanitizer.sanitize(
                        "\(problem.component)/\(problem.operation): \(problem.message)"
                    ),
                    isProblem: problem.level == "error"
                )
            }
        }

        return DiagnosticSection(id: "recent", title: "Recent", lines: lines)
    }

    private static func merged(
        core: CoreDiagnostics?,
        local: [DiagnosticProblem]
    ) -> [DiagnosticProblem] {
        var all = local
        if let core {
            all.append(contentsOf: core.recent.map { event in
                DiagnosticProblem(
                    level: event.level,
                    component: event.component,
                    operation: event.operation,
                    message: DiagnosticsSanitizer.sanitize(event.message),
                    atMs: event.atMs
                )
            })
        }

        all.sort { $0.atMs > $1.atMs }
        var seen = Set<String>()
        var kept: [DiagnosticProblem] = []
        for problem in all {
            let key = "\(problem.component)\u{0}\(problem.operation)\u{0}\(problem.message)"
            if seen.insert(key).inserted {
                kept.append(problem)
            }
            if kept.count == RecentDiagnostics.limit { break }
        }
        return kept
    }

    private static func endpointLine(_ title: String, _ endpoint: DiagnosticEndpoint?) -> DiagnosticLine {
        guard let endpoint else {
            return line("core", title, "unknown — the core is not connected", problem: false)
        }

        switch endpoint.state {
        case "listening":
            let address = endpoint.address ?? ""
            let value = address.isEmpty ? "listening" : "listening \(address)"
            return line("core", title, value, problem: false)
        case "unavailable":
            let why = endpoint.message.map { " — \($0)" } ?? ""
            return line("core", title, "unavailable\(why)", problem: true)
        default:
            return line("core", title, "unknown", problem: false)
        }
    }

    private static func lanLine(_ core: CoreDiagnostics?) -> DiagnosticLine {
        guard let core else {
            return line("core", "LAN", "unknown — the core is not connected", problem: false)
        }
        guard let lan = core.lan else {
            return line("core", "LAN", "not reported", problem: false)
        }

        if lan.enabled, let address = lan.address {
            return line("core", "LAN", "on \(address):\(lan.port)", problem: false)
        }
        if lan.enabled {
            return line("core", "LAN", "on, address unavailable (port \(lan.port))", problem: true)
        }
        return line("core", "LAN", "off (port \(lan.port))", problem: false)
    }

    private static func connectionLine(
        _ connection: ConnectionState,
        core: CoreDiagnostics?
    ) -> DiagnosticLine {
        switch connection {
        case .idle:
            return line("core", "IPC", "not connected yet", problem: false)
        case .connecting:
            return line("core", "IPC", "connecting", problem: false)
        case .connected:
            if let core {
                return line("core", "IPC", "connected (core \(core.ipc.state))", problem: false)
            }
            return line("core", "IPC", "connected, status unavailable", problem: true)
        case let .disconnected(message):
            if let message, !message.isEmpty {
                return line("core", "IPC", "disconnected — \(message)", problem: true)
            }
            return line("core", "IPC", "disconnected", problem: true)
        }
    }

    private static func storageLine(_ core: CoreDiagnostics?) -> DiagnosticLine {
        guard let storage = core?.storage else {
            return line("core", "Storage", "unknown — the core is not connected", problem: false)
        }

        let why = storage.message.map { " — \($0)" } ?? ""
        return line(
            "core",
            "Storage",
            "\(storage.state)\(why)",
            problem: storage.state == "unavailable"
        )
    }

    private static func line(
        _ section: String,
        _ title: String,
        _ value: String,
        problem: Bool
    ) -> DiagnosticLine {
        DiagnosticLine(
            id: "\(section).\(title)",
            title: title,
            value: DiagnosticsSanitizer.sanitize(value),
            isProblem: problem
        )
    }

    private static func render(_ sections: [DiagnosticSection]) -> String {
        // Each value was sanitized on its own. Sanitizing the joined report
        // would treat the whole document as one message and cut it off.
        var lines = ["ProxyLab diagnostics", ""]
        for section in sections {
            lines.append(section.title)
            for line in section.lines {
                lines.append("  \(line.title): \(line.value)")
            }
            lines.append("")
        }

        return lines.joined(separator: "\n")
    }
}

/// The lines for one device, and the failure worth keeping if there is one.
public struct DeviceLines: Equatable, Sendable {
    public var lines: [DiagnosticLine]
    public var problem: String?

    public init(lines: [DiagnosticLine], problem: String?) {
        self.lines = lines
        self.problem = problem
    }
}
