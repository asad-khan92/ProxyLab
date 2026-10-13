//  The Network Extension section, drawn offscreen in the states it has to
//  survive. Smoke tests, like the other panels': a body that traps or lays out
//  to nothing fails here.

import AppKit
import SwiftUI
import Testing
import TransparentProxyKit

@testable import ProxyLabCore

@Suite("Drawing the Network Extension section")
@MainActor
struct TransparentProxyViewRenderingTests {
    private func size(_ model: TransparentProxyModel, coreConnected: Bool) -> CGSize? {
        let view = TransparentProxySectionView(model: model, coreConnected: coreConnected)
        let renderer = ImageRenderer(content: view.frame(width: 230))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    @Test("it draws off, not yet installed, on, on with no core, awaiting approval, and unavailable")
    func draws() async {
        let off = TransparentProxyModel(control: FakeTransparentProxyControl(extensionState: .installed, tunnel: .off))
        await off.refresh()

        let notInstalled = TransparentProxyModel(
            control: FakeTransparentProxyControl(extensionState: .notInstalled, tunnel: .off)
        )
        await notInstalled.refresh()

        let on = TransparentProxyModel(control: FakeTransparentProxyControl(extensionState: .installed, tunnel: .on))
        await on.refresh()

        let approving = FakeTransparentProxyControl()
        approving.activation = .success(.awaitingApproval)
        let awaiting = TransparentProxyModel(control: approving)
        await awaiting.turnOn()

        let unavailable = TransparentProxyModel(control: FakeTransparentProxyControl(readiness: .unsigned))

        for (model, core) in [
            (off, true), (notInstalled, true), (on, true), (on, false), (awaiting, true), (unavailable, false),
        ] {
            let drawn = size(model, coreConnected: core)
            #expect((drawn?.height ?? 0) > 0)
        }
    }
}
