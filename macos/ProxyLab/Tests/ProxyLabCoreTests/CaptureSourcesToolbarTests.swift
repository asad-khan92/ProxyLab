//  The traffic toolbar's four source buttons, and the panels they open.
//
//  The words are the point: an icon with its title kept for a tooltip is how
//  Simulator, Android, iPhone, and This Mac were missed. This draws the
//  buttons and each panel, and checks the four titles stay the ones on screen.

import SimulatorTools
import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("The capture-source toolbar")
struct CaptureSourcesToolbarTests {
    @Test("the buttons are named Simulator, Android, iPhone, and This Mac")
    func titles() {
        #expect(CaptureSource.allCases.map(\.title) == [
            "Simulator", "Android", "iPhone", "This Mac",
        ])
    }

    @Test("the buttons and each panel lay out")
    func draws() {
        let toolbar = CaptureSourcesToolbar(
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
            coreConnected: true
        )

        let buttons = ImageRenderer(content: toolbar.frame(width: 520))
        buttons.scale = 1
        #expect((buttons.nsImage?.size.height ?? 0) > 0)

        for source in CaptureSource.allCases {
            let panel = ImageRenderer(content: toolbar.panel(for: source).frame(width: 280))
            panel.scale = 1
            #expect((panel.nsImage?.size.height ?? 0) > 0)
        }
    }
}
