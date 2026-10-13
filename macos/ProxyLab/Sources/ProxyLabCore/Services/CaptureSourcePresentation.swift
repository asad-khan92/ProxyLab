//  What a capture-source row says, and what its detail is allowed to add.
//
//  The session models stay the source of truth: a simulator's route, an
//  emulator's proxy, the LAN listener, and capture's phase. This file only
//  chooses the words. A row is a name and one short state. Ports, tool
//  names, and the sentences those models keep for Diagnostics stay off the
//  row, and off the detail unless a failure has to be read there.
//
//  Selecting a row does not filter traffic. Nothing here is a host.

import AndroidTools
import Foundation

/// Which capture source a row is.
enum CaptureSourceKind: String, CaseIterable, Identifiable {
    case mac
    case simulator
    case android
    case iphone

    var id: String { rawValue }
}

/// How loud the row's marker is.
enum CaptureSourceTone: Equatable, Sendable {
    case ready
    case idle
    case attention
    case fault
}

/// One compact row. The sidebar draws this and nothing longer.
struct CaptureSourceState: Equatable, Identifiable, Sendable {
    var source: CaptureSourceKind
    var title: String
    var subtitle: String
    var tone: CaptureSourceTone
    /// A filled dot. An idle source stays an empty one.
    var filled: Bool

    var id: CaptureSourceKind { source }
}

/// A label and a value in the detail, already in user-facing words.
struct CaptureSourceFact: Equatable, Identifiable, Sendable {
    var label: String
    var value: String

    var id: String { label }
}

/// A button the detail may show. Start and Stop are not among them.
enum CaptureSourceControl: Equatable, Sendable {
    case connect
    case disconnect
    case sendCertificate
    case enableLocalNetwork
    case disableLocalNetwork
    case certificateInstructions

    var title: String {
        switch self {
        case .connect: "Connect"
        case .disconnect: "Disconnect"
        case .sendCertificate: "Send CA"
        case .enableLocalNetwork: "Enable Local Network"
        case .disableLocalNetwork: "Disable Local Network"
        case .certificateInstructions: "CA Setup…"
        }
    }
}

/// One of those buttons, and whether it can be pressed.
struct CaptureSourceControlItem: Equatable, Identifiable, Sendable {
    var control: CaptureSourceControl
    var enabled: Bool

    var id: CaptureSourceControl { control }
}

/// The row plus the detail that opens from it.
struct CaptureSourcePresentation: Equatable, Sendable {
    var state: CaptureSourceState
    var facts: [CaptureSourceFact]
    /// One sentence under the facts. Nil when the subtitle already said it.
    var message: String?
    var messageIsWarning: Bool
    /// A failure note kept behind Details…, when the message would be the
    /// wrong place for it.
    var technicalDetail: String?
    var controls: [CaptureSourceControlItem]
    var showsSimulatorPicker: Bool
    var showsAndroidPicker: Bool
    var showsAddressPicker: Bool
    /// Install steps, after the CA file has been copied to an emulator.
    var certificateSteps: [String]

    /// The capture lifecycle, the transparent-capture model, and LAN, read
    /// at one moment.
    struct ThisMacInput: Equatable, Sendable {
        var phase: CapturePhase
        var trustActivity: String?
        var statusDetail: String?
        var proxyNotice: String?
        var coreRunning: Bool
        var transparentOn: Bool
        var transparentAvailable: Bool
        var awaitingApproval: Bool
        var passingThrough: Bool
        var lanEndpoint: String?
        var lanWaiting: Bool
    }
}

extension CaptureSourcePresentation {
    /// This Mac, from the capture lifecycle. There is no Start or Stop here.
    @MainActor
    static func thisMac(
        lifecycle: CaptureLifecycle,
        lan: IPhoneSetupModel?,
        coreConnected: Bool
    ) -> CaptureSourcePresentation {
        let transparent = lifecycle.transparent
        let lanEndpoint: String?
        let lanWaiting: Bool
        switch lan?.state {
        case let .on(endpoint):
            lanEndpoint = endpoint
            lanWaiting = false
        case .waitingForNetwork:
            lanEndpoint = nil
            lanWaiting = true
        default:
            lanEndpoint = nil
            lanWaiting = false
        }

        return thisMac(
            ThisMacInput(
                phase: lifecycle.phase,
                trustActivity: lifecycle.trustActivity,
                statusDetail: lifecycle.statusDetail,
                proxyNotice: lifecycle.proxyNotice,
                coreRunning: lifecycle.coreIsRunning,
                transparentOn: transparent.isOn,
                transparentAvailable: transparent.readiness == .ready,
                awaitingApproval: lifecycle.phase == .needsApproval
                    || transparent.snapshot?.extensionState == .awaitingApproval,
                passingThrough: transparent.isPassingThrough(coreConnected: coreConnected),
                lanEndpoint: lanEndpoint,
                lanWaiting: lanWaiting
            )
        )
    }

    /// This Mac from the values the lifecycle is already showing.
    static func thisMac(_ input: ThisMacInput) -> CaptureSourcePresentation {
        let (subtitle, tone) = macSubtitle(input)
        var facts = [
            CaptureSourceFact(label: "System proxy", value: systemProxyValue(input)),
            CaptureSourceFact(label: "Transparent capture", value: transparentValue(input)),
            CaptureSourceFact(label: "CA trust", value: caTrustValue(input)),
            CaptureSourceFact(label: "Core", value: input.coreRunning ? "Running" : "Stopped"),
        ]
        if let endpoint = input.lanEndpoint {
            facts.append(CaptureSourceFact(label: "Local network", value: endpoint))
        } else if input.lanWaiting {
            facts.append(CaptureSourceFact(label: "Local network", value: "Waiting for a network"))
        }

        let notice = input.proxyNotice.map(CaptureSourceWording.detail(of:))
        let failure = input.statusDetail.map(CaptureSourceWording.detail(of:))
        let message = notice?.message ?? failure?.message
        let technical = notice?.technical ?? failure?.technical

        return make(
            source: .mac,
            title: "This Mac",
            subtitle: subtitle,
            tone: tone,
            filled: tone != .idle,
            facts: facts,
            message: message,
            messageIsWarning: notice != nil && failure == nil,
            technicalDetail: technical,
            controls: []
        )
    }

    /// The booted simulator, from the session model and the route it reads.
    @MainActor
    static func simulator(
        model: SimulatorSessionModel,
        transparentCaptureListening: Bool,
        coreConnected: Bool
    ) -> CaptureSourcePresentation {
        let presented = model.presentation(transparentCaptureListening: transparentCaptureListening)
        let device = model.selectedSimulator
            ?? model.simulators.first { $0.udid == presented.connection.udid }
        let title = device?.name ?? "Simulator"
        var facts: [CaptureSourceFact] = []
        if let runtime = device?.runtime, !runtime.isEmpty {
            facts.append(CaptureSourceFact(label: "System", value: runtime))
        }

        switch presented.connection {
        case .connected:
            let path = presented.trafficPath == .transparentCapture
                ? "Transparent Capture"
                : "Explicit Proxy"
            let through = presented.trafficPath == .transparentCapture
                ? "Captured through transparent capture."
                : "Captured through This Mac."
            facts.append(CaptureSourceFact(label: "Routing", value: path))
            facts.append(CaptureSourceFact(label: "CA", value: "Trusted"))
            return make(
                source: .simulator,
                title: title,
                subtitle: "Connected · CA trusted",
                tone: .ready,
                filled: true,
                facts: facts,
                message: through,
                controls: [item(.disconnect, enabled: model.canDisconnect)]
            )

        case .certificateTrusted:
            facts.append(CaptureSourceFact(label: "Routing", value: "Not Routed"))
            facts.append(CaptureSourceFact(label: "CA", value: "Trusted"))
            let unread = presented.note?.text.contains("could not be read") == true
            return make(
                source: .simulator,
                title: title,
                subtitle: "CA trusted · Not routed",
                tone: .attention,
                filled: true,
                facts: facts,
                message: unread
                    ? "The certificate is trusted. The route could not be read."
                    : "This simulator trusts the certificate. Start capture so its traffic is routed through This Mac.",
                technicalDetail: presented.note.map { CaptureSourceWording.detail(of: $0.text).technical ?? "" }
                    .flatMap { $0.isEmpty ? nil : $0 },
                controls: [item(.disconnect, enabled: model.canDisconnect)]
            )

        case .connecting:
            return make(
                source: .simulator,
                title: title,
                subtitle: "Connecting…",
                tone: .attention,
                filled: true,
                facts: facts,
                controls: []
            )

        case .notConnected:
            break
        }

        if model.simulators.isEmpty {
            if let note = model.note, note.isFailure, !CaptureSourceWording.isAbsence(note.text) {
                let detail = CaptureSourceWording.detail(of: note.text)
                return make(
                    source: .simulator,
                    title: "Simulator",
                    subtitle: CaptureSourceWording.rowFailure(note.text),
                    tone: .fault,
                    filled: true,
                    facts: [],
                    message: detail.message,
                    technicalDetail: detail.technical,
                    controls: []
                )
            }
            let checking = !model.hasRefreshed
            return make(
                source: .simulator,
                title: "Simulator",
                subtitle: checking ? "Checking…" : "No simulator",
                tone: checking ? .attention : .idle,
                filled: checking,
                facts: [],
                controls: []
            )
        }

        if let note = model.note, note.isFailure {
            let detail = CaptureSourceWording.detail(of: note.text)
            return make(
                source: .simulator,
                title: title,
                subtitle: CaptureSourceWording.rowFailure(note.text),
                tone: .fault,
                filled: true,
                facts: facts,
                message: detail.message == CaptureSourceWording.rowFailure(note.text) ? nil : detail.message,
                technicalDetail: detail.technical,
                controls: [item(.connect, enabled: model.canConnect && coreConnected)]
            )
        }

        return make(
            source: .simulator,
            title: title,
            subtitle: "Not connected",
            tone: .idle,
            filled: false,
            facts: facts,
            message: "Install the certificate to capture this simulator.",
            showsSimulatorPicker: model.simulators.count > 1,
            controls: [item(.connect, enabled: model.canConnect)]
        )
    }

    /// The running emulator. The proxy address it is given stays in the model.
    @MainActor
    static func android(
        model: AndroidSessionModel,
        coreConnected: Bool
    ) -> CaptureSourcePresentation {
        let device = model.selectedEmulator
        let title = emulatorTitle(device)

        switch model.state {
        case .connecting:
            return make(
                source: .android,
                title: title,
                subtitle: "Connecting…",
                tone: .attention,
                filled: true,
                controls: []
            )

        case .connected:
            if let note = model.note, note.isFailure {
                let detail = CaptureSourceWording.detail(of: note.text)
                return make(
                    source: .android,
                    title: title,
                    subtitle: CaptureSourceWording.rowFailure(note.text),
                    tone: .fault,
                    filled: true,
                    message: detail.message,
                    technicalDetail: detail.technical,
                    controls: [
                        item(.sendCertificate, enabled: model.canDeliverCertificate),
                        item(.disconnect, enabled: model.canDisconnect),
                    ]
                )
            }
            if model.certificateDelivered {
                return make(
                    source: .android,
                    title: title,
                    subtitle: "Connected",
                    tone: .ready,
                    filled: true,
                    facts: [CaptureSourceFact(label: "CA", value: "Copied to the emulator")],
                    message: "Install the certificate from Downloads. HTTPS is readable only in apps that trust a user certificate.",
                    controls: [
                        item(.disconnect, enabled: model.canDisconnect),
                    ],
                    certificateSteps: model.certificateSteps
                )
            }
            return make(
                source: .android,
                title: title,
                subtitle: "Needs CA setup",
                tone: .attention,
                filled: true,
                facts: [CaptureSourceFact(label: "CA", value: "Not installed")],
                message: "The emulator is connected. Send the certificate, then install it there.",
                controls: [
                    item(.sendCertificate, enabled: model.canDeliverCertificate),
                    item(.disconnect, enabled: model.canDisconnect),
                ]
            )

        case .notConnected:
            break
        }

        if model.emulators.isEmpty {
            if let note = model.note, note.isFailure, !CaptureSourceWording.isAbsence(note.text) {
                let detail = CaptureSourceWording.detail(of: note.text)
                return make(
                    source: .android,
                    title: "Android",
                    subtitle: CaptureSourceWording.rowFailure(note.text),
                    tone: .fault,
                    filled: true,
                    message: detail.message,
                    technicalDetail: detail.technical,
                    controls: []
                )
            }
            let checking = !model.hasRefreshed
            return make(
                source: .android,
                title: "Android",
                subtitle: checking ? "Checking…" : "No emulator",
                tone: checking ? .attention : .idle,
                filled: checking,
                controls: []
            )
        }

        if let note = model.note, note.isFailure {
            let detail = CaptureSourceWording.detail(of: note.text)
            return make(
                source: .android,
                title: title,
                subtitle: CaptureSourceWording.rowFailure(note.text),
                tone: .fault,
                filled: true,
                message: detail.message,
                technicalDetail: detail.technical,
                showsAndroidPicker: model.emulators.count > 1,
                controls: [item(.connect, enabled: model.canConnect && coreConnected)]
            )
        }

        let usable = device?.isUsable ?? false
        return make(
            source: .android,
            title: title,
            subtitle: usable ? "Not connected" : "Not ready",
            tone: usable ? .idle : .attention,
            filled: !usable,
            message: usable ? "Point the emulator at ProxyLab." : "Wait until the emulator has finished starting.",
            showsAndroidPicker: model.emulators.count > 1,
            controls: [item(.connect, enabled: model.canConnect && coreConnected)]
        )
    }

    /// The physical iPhone, which is the LAN listener and nothing else.
    @MainActor
    static func iPhone(
        model: IPhoneSetupModel,
        coreConnected: Bool
    ) -> CaptureSourcePresentation {
        switch model.state {
        case let .on(endpoint):
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: "Ready on \(endpoint)",
                tone: .ready,
                filled: true,
                facts: [CaptureSourceFact(label: "Address", value: endpoint)],
                message: "Any device on this Wi-Fi can use this proxy. There is no password.",
                messageIsWarning: true,
                controls: [
                    item(.disableLocalNetwork, enabled: model.canDisable && coreConnected),
                    item(.certificateInstructions, enabled: true),
                ]
            )

        case .waitingForNetwork:
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: "Waiting for a local network",
                tone: .attention,
                filled: true,
                message: "Local network access is on. This Mac has no address yet.",
                controls: [item(.disableLocalNetwork, enabled: model.canDisable && coreConnected)]
            )

        case .loading:
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: "Checking…",
                tone: .attention,
                filled: true,
                controls: []
            )

        case .working:
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: "Working…",
                tone: .attention,
                filled: true,
                controls: []
            )

        case let .failed(reason):
            let detail = CaptureSourceWording.detail(of: reason)
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: CaptureSourceWording.rowFailure(reason),
                tone: .fault,
                filled: true,
                message: detail.message,
                technicalDetail: detail.technical,
                controls: []
            )

        case .off:
            break
        }

        if model.addresses.isEmpty {
            let checking = model.status == nil && model.lastError == nil
            return make(
                source: .iphone,
                title: "Physical iPhone",
                subtitle: checking ? "Checking…" : "Waiting for a local network",
                tone: checking ? .attention : .idle,
                filled: checking,
                message: checking ? nil : "This Mac has no local network address yet.",
                controls: []
            )
        }

        var facts: [CaptureSourceFact] = []
        if model.addresses.count == 1, let address = model.selected ?? model.addresses.first?.address {
            facts.append(CaptureSourceFact(label: "This Mac", value: address))
        }

        return make(
            source: .iphone,
            title: "Physical iPhone",
            subtitle: "Local network off",
            tone: .idle,
            filled: false,
            facts: facts,
            message: "Enable local network access, then point the iPhone at this Mac.",
            showsAddressPicker: model.addresses.count > 1,
            controls: [item(.enableLocalNetwork, enabled: model.canEnable && coreConnected)]
        )
    }
}

extension CaptureSourcePresentation {
    fileprivate static func make(
        source: CaptureSourceKind,
        title: String,
        subtitle: String,
        tone: CaptureSourceTone,
        filled: Bool,
        facts: [CaptureSourceFact] = [],
        message: String? = nil,
        messageIsWarning: Bool = false,
        technicalDetail: String? = nil,
        showsSimulatorPicker: Bool = false,
        showsAndroidPicker: Bool = false,
        showsAddressPicker: Bool = false,
        controls: [CaptureSourceControlItem],
        certificateSteps: [String] = []
    ) -> CaptureSourcePresentation {
        let technical = technicalDetail?.isEmpty == true ? nil : technicalDetail
        return CaptureSourcePresentation(
            state: CaptureSourceState(
                source: source,
                title: title,
                subtitle: subtitle,
                tone: tone,
                filled: filled
            ),
            facts: facts,
            message: message,
            messageIsWarning: messageIsWarning,
            technicalDetail: technical,
            controls: controls,
            showsSimulatorPicker: showsSimulatorPicker,
            showsAndroidPicker: showsAndroidPicker,
            showsAddressPicker: showsAddressPicker,
            certificateSteps: certificateSteps
        )
    }

    fileprivate static func item(
        _ control: CaptureSourceControl,
        enabled: Bool
    ) -> CaptureSourceControlItem {
        CaptureSourceControlItem(control: control, enabled: enabled)
    }

    fileprivate static func emulatorTitle(_ emulator: AndroidEmulator?) -> String {
        guard let raw = emulator?.avdName ?? emulator?.model, !raw.isEmpty else { return "Android" }

        return raw.replacingOccurrences(of: "_", with: " ")
    }

    private static func macSubtitle(_ input: ThisMacInput) -> (String, CaptureSourceTone) {
        if input.proxyNotice != nil, input.phase == .listening || input.phase == .needsApproval {
            return ("Needs attention", .attention)
        }
        if let activity = input.trustActivity {
            switch activity {
            case CaptureTrustWording.checking: return ("Checking CA trust", .attention)
            case CaptureTrustWording.needsTrust: return ("Needs CA Trust", .attention)
            case CaptureTrustWording.installing: return ("Installing CA", .attention)
            case CaptureTrustWording.cancelled: return ("CA trust cancelled", .fault)
            case CaptureTrustWording.failed: return ("CA trust failed", .fault)
            default: return (CaptureSourceWording.rowFailure(activity), .fault)
            }
        }
        switch input.phase {
        case .stopped: return ("Stopped", .idle)
        case .starting: return ("Starting…", .attention)
        case .listening: return ("Listening", .ready)
        case .stopping: return ("Stopping…", .attention)
        case .needsApproval: return ("Needs Approval", .attention)
        case .needsTrust: return ("Needs CA Trust", .attention)
        case let .failed(message): return (CaptureSourceWording.rowFailure(message), .fault)
        }
    }

    private static func systemProxyValue(_ input: ThisMacInput) -> String {
        if input.proxyNotice != nil { return "Changed" }
        if let detail = input.statusDetail, CaptureSourceWording.proxyLeftInPlace(detail) {
            return "Still set"
        }
        switch input.phase {
        case .listening, .needsApproval: return "On"
        case .stopping: return "Restoring"
        case .starting, .needsTrust: return "Waiting"
        case .stopped, .failed: return "Off"
        }
    }

    private static func transparentValue(_ input: ThisMacInput) -> String {
        if !input.transparentAvailable { return "Unavailable" }
        if input.awaitingApproval { return "Needs approval" }
        if input.passingThrough { return "Not capturing" }
        return input.transparentOn ? "On" : "Off"
    }

    private static func caTrustValue(_ input: ThisMacInput) -> String {
        if let activity = input.trustActivity {
            switch activity {
            case CaptureTrustWording.checking: return "Checking"
            case CaptureTrustWording.needsTrust: return "Needs trust"
            case CaptureTrustWording.installing: return "Installing"
            case CaptureTrustWording.cancelled, CaptureTrustWording.failed: return "Not trusted"
            default: return "Not trusted"
            }
        }
        switch input.phase {
        case .listening, .needsApproval: return "Trusted"
        case .needsTrust: return "Needs trust"
        case .stopped: return "Not checked"
        case .starting, .stopping: return "Checking"
        case .failed: return "Not trusted"
        }
    }
}

/// Turns a model sentence into a row and, when the sentence is the wrong
/// length or names an address, a detail.
enum CaptureSourceWording {
    struct Split: Equatable {
        var message: String?
        var technical: String?
    }

    static func rowFailure(_ text: String) -> String {
        let folded = text.lowercased()
        if isInternal(text) { return "Error" }
        if folded.contains("cancel") && folded.contains("trust") { return "CA trust cancelled" }
        if folded.contains("ca trust failed")
            || folded.contains("could not trust")
            || folded.contains("keychain")
            || (folded.contains("certificate") && folded.contains("not"))
        {
            return "CA trust failed"
        }
        if folded.contains("needs trust") { return "Needs CA trust" }
        if folded.contains("already in use") { return "Port in use" }
        if folded.contains("no longer booted") { return "Simulator shut down" }
        if folded.contains("no longer running") { return "Emulator shut down" }
        if folded.contains("not running") || folded.contains("could not reach the proxylab core")
            || folded.contains("not connected to the proxylab core")
        {
            return "Capture is stopped"
        }
        if isAbsence(text) { return text.contains("Android") ? "No emulator" : "No simulator" }
        if folded.contains("adb could not be found") || folded.contains("android_home") {
            return "Android tools missing"
        }
        let sentence = text.split(whereSeparator: { $0 == "." || $0 == "\n" }).first.map(String.init) ?? text
        let trimmed = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.count <= 32, !mentionsHiddenAddress(trimmed) { return trimmed }
        return "Error"
    }

    /// The empty-device sentences. They are a state, not a failure.
    static func isAbsence(_ text: String) -> Bool {
        let folded = text.lowercased()
        return folded.contains("no ios simulator is booted")
            || folded.contains("no android emulator is running")
    }

    static func proxyLeftInPlace(_ text: String) -> Bool {
        let folded = text.lowercased()
        return folded.contains("left running") || folded.contains("still sending")
    }

    /// What the detail shows. A sentence that names the loopback or emulator
    /// route is kept behind Details….
    static func detail(of text: String) -> Split {
        if isInternal(text) {
            return Split(message: "Capture did not start.", technical: text)
        }
        let row = rowFailure(text)
        if text.lowercased().contains("already in use") {
            return Split(
                message: "Another process is using the proxy port. ProxyLab left it running.",
                technical: text
            )
        }
        if mentionsHiddenAddress(text) || text.count > 180 {
            return Split(
                message: row == "Error" ? "See Details for what happened." : nil,
                technical: text
            )
        }
        if text == row { return Split(message: nil, technical: nil) }
        return Split(message: text, technical: nil)
    }

    static func mentionsHiddenAddress(_ text: String) -> Bool {
        text.contains("127.0.0.1") || text.contains("10.0.2.2") || text.contains("9091")
    }

    /// Core and extension wording that belongs in Diagnostics, not on a row.
    static func isInternal(_ text: String) -> Bool {
        let folded = text.lowercased()
        return folded.contains("bridge") || folded.contains("ipc disconnected")
    }
}
