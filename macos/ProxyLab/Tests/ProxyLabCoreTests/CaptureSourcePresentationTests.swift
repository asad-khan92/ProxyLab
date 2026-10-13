//  The capture-source rows: a name and one state, drawn from the models
//  that already exist.
//
//  What is asserted is the wording. A connected simulator does not mention
//  the loopback port, an emulator does not mention its alias for that port,
//  and a physical iPhone's row follows the LAN address the model was given.
//  The sidebar that holds the rows is drawn offscreen, short and tall.

import AndroidTools
import AppKit
import SimulatorTools
import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Capture source presentation")
struct CaptureSourcePresentationTests {
    @Test("This Mac follows capture: listening, stopped, trust, and approval")
    func thisMacStates() {
        let listening = CaptureSourcePresentation.thisMac(mac(phase: .listening, coreRunning: true, transparentOn: true))
        #expect(listening.state.title == "This Mac")
        #expect(listening.state.subtitle == "Listening")
        #expect(listening.state.tone == .ready)
        #expect(listening.facts.map(\.value).contains("On"))
        #expect(listening.facts.first { $0.label == "CA trust" }?.value == "Trusted")
        #expect(listening.controls.isEmpty)

        let stopped = CaptureSourcePresentation.thisMac(mac(phase: .stopped))
        #expect(stopped.state.subtitle == "Stopped")
        #expect(stopped.state.filled == false)
        #expect(stopped.facts.first { $0.label == "System proxy" }?.value == "Off")
        #expect(stopped.controls.isEmpty)

        let trust = CaptureSourcePresentation.thisMac(
            mac(phase: .starting, trustActivity: CaptureTrustWording.needsTrust)
        )
        #expect(trust.state.subtitle == "Needs CA Trust")
        #expect(trust.state.tone == .attention)
        #expect(trust.facts.first { $0.label == "CA trust" }?.value == "Needs trust")

        let approval = CaptureSourcePresentation.thisMac(
            mac(phase: .needsApproval, coreRunning: true, transparentAvailable: true, awaitingApproval: true)
        )
        #expect(approval.state.subtitle == "Needs Approval")
        #expect(approval.facts.first { $0.label == "Transparent capture" }?.value == "Needs approval")
        #expect(approval.controls.isEmpty)
    }

    @Test("a trust failure is one short line, and the longer sentence stays in the detail")
    func conciseTrustFailure() {
        let failed = CaptureSourcePresentation.thisMac(
            mac(
                phase: .failed(CaptureTrustWording.needsTrustDetail),
                trustActivity: CaptureTrustWording.failed,
                statusDetail: CaptureTrustWording.needsTrustDetail
            )
        )

        #expect(failed.state.subtitle == "CA trust failed")
        #expect(failed.state.tone == .fault)
        #expect(failed.message == CaptureTrustWording.needsTrustDetail)
        #expect(failed.controls.isEmpty)
        #expect(primaryText(failed).contains("127.0.0.1") == false)
    }

    @Test("an internal capture failure is not named on the row")
    func internalFailureStaysOffTheRow() {
        let text = "The transparent bridge is not listening. Capture was not started."
        let failed = CaptureSourcePresentation.thisMac(
            mac(phase: .failed(text), statusDetail: text)
        )

        #expect(failed.state.subtitle == "Error")
        #expect(failed.message == "Capture did not start.")
        #expect(failed.technicalDetail == text)
        #expect(primaryText(failed).contains("bridge") == false)
    }

    @Test("a simulator on the system proxy is connected and does not name the port")
    func simulatorExplicitProxy() async {
        let model = await connectedSimulator(route: .init(
            http: ProxyEndpoint(host: "127.0.0.1", port: 9_090),
            https: ProxyEndpoint(host: "127.0.0.1", port: 9_090)
        ))

        let presented = CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: true,
            coreConnected: true
        )

        #expect(presented.state.title == "iPhone 17 Pro Max")
        #expect(presented.state.subtitle == "Connected · CA trusted")
        #expect(presented.facts.first { $0.label == "Routing" }?.value == "Explicit Proxy")
        #expect(presented.message == "Captured through This Mac.")
        #expect(presented.technicalDetail == nil)
        #expect(primaryText(presented).contains("127.0.0.1") == false)
        #expect(presented.controls.map(\.control) == [.disconnect])
    }

    @Test("transparent capture is the route only when the system proxy is not")
    func simulatorTransparent() async {
        let model = await connectedSimulator(route: .direct)

        let presented = CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: true,
            coreConnected: true
        )

        #expect(presented.state.subtitle == "Connected · CA trusted")
        #expect(presented.facts.first { $0.label == "Routing" }?.value == "Transparent Capture")
        #expect(presented.message == "Captured through transparent capture.")
        #expect(primaryText(presented).contains("9091") == false)
    }

    @Test("a trusted simulator with no route says so, and stopping capture changes that row")
    func simulatorNotRoutedAfterStop() async {
        let routes = AdjustableRoute(route: ProxyRoute(
            http: ProxyEndpoint(host: "127.0.0.1", port: 9_090),
            https: ProxyEndpoint(host: "127.0.0.1", port: 9_090)
        ))
        let model = simulator(routes: routes)
        await model.refresh()
        await model.connect(coreIsRunning: true, transparentCaptureListening: true)

        let listening = CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: true,
            coreConnected: true
        )
        #expect(listening.state.subtitle == "Connected · CA trusted")

        routes.route = .direct
        model.noteRouteMayHaveChanged()
        let stopped = CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: false,
            coreConnected: true
        )

        #expect(stopped.state.subtitle == "CA trusted · Not routed")
        #expect(stopped.facts.first { $0.label == "Routing" }?.value == "Not Routed")
        #expect(primaryText(stopped).contains("127.0.0.1") == false)
        #expect(stopped.state.subtitle != listening.state.subtitle)
    }

    @Test("a missing simulator certificate is a CA failure, not a paragraph")
    func simulatorTrustFailure() async {
        let model = simulator(certificateExists: false)
        await model.refresh()
        await model.connect(coreIsRunning: true)

        let presented = CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: false,
            coreConnected: true
        )

        #expect(presented.state.subtitle == "CA trust failed")
        #expect(presented.state.tone == .fault)
        #expect(presented.state.subtitle.contains("\n") == false)
    }

    @Test("an emulator is connected, or still needs the CA, without its proxy address")
    func androidStates() async {
        let adb = FakeAdb()
        let model = AndroidSessionModel(
            runner: adb,
            port: 9_090,
            certificatePath: "/ca.pem",
            certificateExists: { _ in true }
        )
        await model.refresh()

        let idle = CaptureSourcePresentation.android(model: model, coreConnected: true)
        #expect(idle.state.title == "Pixel 10 Pro XL")
        #expect(idle.state.subtitle == "Not connected")
        #expect(primaryText(idle).contains("10.0.2.2") == false)

        await model.connect(coreIsRunning: true)
        let needsCA = CaptureSourcePresentation.android(model: model, coreConnected: true)
        #expect(needsCA.state.subtitle == "Needs CA setup")
        #expect(needsCA.controls.map(\.control) == [.sendCertificate, .disconnect])
        #expect(primaryText(needsCA).contains("10.0.2.2") == false)
        #expect(primaryText(needsCA).contains("adb") == false)

        await model.deliverCertificate()
        let connected = CaptureSourcePresentation.android(model: model, coreConnected: true)
        #expect(connected.state.subtitle == "Connected")
        #expect(connected.state.tone == .ready)
        #expect(connected.certificateSteps.isEmpty == false)
        #expect(primaryText(connected).contains("10.0.2.2") == false)
    }

    @Test("no running emulator is an idle row, and missing tools are an error")
    func androidAbsence() async {
        let empty = AndroidSessionModel(
            runner: FakeAdb(present: false),
            port: 9_090,
            certificatePath: "/ca.pem",
            certificateExists: { _ in true }
        )
        await empty.refresh()
        let none = CaptureSourcePresentation.android(model: empty, coreConnected: true)
        #expect(none.state.subtitle == "No emulator")
        #expect(none.state.tone == .idle)

        let missing = AndroidSessionModel(runner: nil, port: 9_090, certificatePath: "/ca.pem")
        await missing.refresh()
        let tools = CaptureSourcePresentation.android(model: missing, coreConnected: true)
        #expect(tools.state.subtitle == "Android tools missing")
        #expect(tools.state.tone == .fault)
        #expect(tools.state.subtitle.contains("ANDROID_HOME") == false)
    }

    @Test("a physical iPhone row names the LAN address and follows a network change")
    func iPhoneFollowsTheLanAddress() async {
        let session = FakeLanSession()
        let first = LanStatusResult(
            enabled: true,
            address: "192.168.110.134",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.110.134")]
        )
        let gone = LanStatusResult(
            enabled: true,
            address: "192.168.110.134",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        let moved = LanStatusResult(
            enabled: true,
            address: "192.168.1.25",
            port: 9_090,
            available: [LanAddress(interface: "en0", address: "192.168.1.25")]
        )
        await session.answerStatuses(with: [.success(first), .success(gone)])
        await session.answerEnable(with: .success(moved))
        let model = IPhoneSetupModel(session: session)
        await model.refresh()

        let ready = CaptureSourcePresentation.iPhone(model: model, coreConnected: true)
        #expect(ready.state.title == "Physical iPhone")
        #expect(ready.state.subtitle == "Ready on 192.168.110.134:9090")
        #expect(ready.facts.first { $0.label == "Address" }?.value == "192.168.110.134:9090")

        await model.networkDidChange(primaryInterface: "en0")

        let updated = CaptureSourcePresentation.iPhone(model: model, coreConnected: true)
        #expect(updated.state.subtitle == "Ready on 192.168.1.25:9090")
        #expect(updated.state.subtitle.contains("192.168.110.134") == false)
    }

    @Test("a physical iPhone with no network waits, and does not borrow the simulator route")
    func iPhoneWaitsForNetwork() async {
        let session = FakeLanSession()
        await session.answerStatus(with: LanFixtures.noAddresses)
        let model = IPhoneSetupModel(session: session)
        await model.refresh()

        let waiting = CaptureSourcePresentation.iPhone(model: model, coreConnected: true)
        #expect(waiting.state.subtitle == "Waiting for a local network")
        #expect(waiting.state.subtitle.contains("127.0.0.1") == false)
        #expect(waiting.controls.isEmpty)
    }

    @Test("rows stay one line, and the source list is not a traffic filter")
    func rowsStayCompact() {
        let samples = [
            CaptureSourcePresentation.thisMac(mac(phase: .listening, coreRunning: true, transparentOn: true)).state,
            CaptureSourceState(source: .simulator, title: "iPhone 17 Pro Max", subtitle: "Connected · CA trusted", tone: .ready, filled: true),
            CaptureSourceState(source: .android, title: "Pixel 10 Pro XL", subtitle: "Needs CA setup", tone: .attention, filled: true),
            CaptureSourceState(source: .iphone, title: "Physical iPhone", subtitle: "Ready on 192.168.110.134:9090", tone: .ready, filled: true),
        ]

        #expect(samples.map(\.source) == [.mac, .simulator, .android, .iphone])
        for row in samples {
            #expect(row.subtitle.contains("\n") == false)
            #expect(row.subtitle.count < 48)
            #expect(row.title.contains("127.0.0.1") == false)
        }
    }
}

@MainActor
@Suite("Capture sources, drawn")
struct CaptureSourcesRenderingTests {
    @Test("the source list stays short, in a short window and a tall one, in either appearance")
    func shortWindow() {
        let rows = VStack(alignment: .leading, spacing: 2) {
            Text("CAPTURE SOURCES")
            CaptureSourceRowLabel(
                state: CaptureSourceState(
                    source: .mac, title: "This Mac", subtitle: "Listening", tone: .ready, filled: true
                ),
                selected: false
            )
            CaptureSourceRowLabel(
                state: CaptureSourceState(
                    source: .simulator, title: "iPhone 17 Pro Max", subtitle: "Connected · CA trusted",
                    tone: .ready, filled: true
                ),
                selected: false
            )
            CaptureSourceRowLabel(
                state: CaptureSourceState(
                    source: .android, title: "Pixel 10 Pro XL", subtitle: "Not connected", tone: .idle, filled: false
                ),
                selected: false
            )
            CaptureSourceRowLabel(
                state: CaptureSourceState(
                    source: .iphone, title: "Physical iPhone", subtitle: "Waiting for a local network",
                    tone: .idle, filled: false
                ),
                selected: false
            )
        }
        .frame(width: 230)

        let ideal = measured(rows)
        #expect(ideal.height > 80)
        #expect(ideal.height < 220)

        let placed = SidebarColumnPlacement.heights(
            available: 320,
            footerIdeal: ideal.height,
            minimumContent: 120
        )
        #expect(placed.footer == ideal.height)
        #expect(abs((placed.content + placed.footer) - 320) < 0.5)

        for height in [200.0, 760.0] {
            for scheme in [ColorScheme.light, ColorScheme.dark] {
                let column = BottomPinnedColumn(minimumContentHeight: 120) {
                    Text("All Traffic")
                } footer: {
                    rows
                }
                .frame(width: 230, height: height)
                .environment(\.colorScheme, scheme)

                #expect(bitmap(column) != nil)
            }
        }
    }

    @Test("a source detail lays out without a second Start")
    func detailLaysOut() {
        let presentation = CaptureSourcePresentation.thisMac(
            mac(phase: .listening, coreRunning: true, transparentOn: true)
        )
        let detail = CaptureSourceDetailView(
            presentation: presentation,
            refreshDisabled: false,
            onRefresh: {},
            perform: { _ in }
        ) {
            EmptyView()
        }
        .frame(width: 300)

        let size = measured(detail)
        #expect(size.height > 40)
        #expect(presentation.controls.contains { $0.control.title == "Start" || $0.control.title == "Stop" } == false)
    }

    private func measured(_ view: some View) -> CGSize {
        let renderer = ImageRenderer(content: view)
        renderer.scale = 1
        return renderer.nsImage?.size ?? .zero
    }

    private func bitmap(_ view: some View) -> NSBitmapImageRep? {
        let renderer = ImageRenderer(content: view)
        renderer.scale = 1
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)
    }
}

@MainActor
private func mac(
    phase: CapturePhase,
    trustActivity: String? = nil,
    statusDetail: String? = nil,
    proxyNotice: String? = nil,
    coreRunning: Bool = false,
    transparentOn: Bool = false,
    transparentAvailable: Bool = true,
    awaitingApproval: Bool = false,
    passingThrough: Bool = false,
    lanEndpoint: String? = nil,
    lanWaiting: Bool = false
) -> CaptureSourcePresentation.ThisMacInput {
    CaptureSourcePresentation.ThisMacInput(
        phase: phase,
        trustActivity: trustActivity,
        statusDetail: statusDetail,
        proxyNotice: proxyNotice,
        coreRunning: coreRunning,
        transparentOn: transparentOn,
        transparentAvailable: transparentAvailable,
        awaitingApproval: awaitingApproval,
        passingThrough: passingThrough,
        lanEndpoint: lanEndpoint,
        lanWaiting: lanWaiting
    )
}

@MainActor
private func simulator(
    routes: any ProxyRouteReading,
    certificateExists: Bool = true
) -> SimulatorSessionModel {
    SimulatorSessionModel(
        catalog: SimulatorCatalog(runner: FakeSimctl.listing([("A", "iPhone 17 Pro Max")])),
        trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in certificateExists }),
        routes: routes,
        endpoint: ProxyEndpoint(host: "127.0.0.1", port: 9_090),
        certificatePath: "/ca.pem"
    )
}

@MainActor
private func simulator(certificateExists: Bool) -> SimulatorSessionModel {
    simulator(routes: FakeProxyRoute(route: .direct), certificateExists: certificateExists)
}

@MainActor
private func connectedSimulator(route: ProxyRoute) async -> SimulatorSessionModel {
    let model = simulator(routes: FakeProxyRoute(route: route))
    await model.refresh()
    await model.connect(coreIsRunning: true, transparentCaptureListening: route == .direct)
    return model
}

private func primaryText(_ presentation: CaptureSourcePresentation) -> String {
    (
        [presentation.state.title, presentation.state.subtitle, presentation.message ?? ""]
            + presentation.facts.map(\.value)
            + presentation.certificateSteps
    ).joined(separator: "\n")
}

private final class AdjustableRoute: ProxyRouteReading, @unchecked Sendable {
    var route: ProxyRoute

    init(route: ProxyRoute) {
        self.route = route
    }

    func currentRoute() throws -> ProxyRoute { route }
}
