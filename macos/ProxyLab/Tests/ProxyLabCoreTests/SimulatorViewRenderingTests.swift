//  The iOS Simulator section, actually drawn.
//
//  A smoke test in the same spirit as `PinnedViewRenderingTests`: a body that
//  compiles is not one that lays out, and this one has a picker that is absent
//  when nothing is booted and a note that wraps to as many lines as it needs.
//  Deliberately two cases and no snapshot comparison.

import SimulatorTools
import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("The iOS Simulator section, drawn")
struct SimulatorViewRenderingTests {
    private func rendered(_ view: some View, width: CGFloat = 220) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    @Test("it draws with a simulator booted and connected")
    func drawsConnected() async {
        let model = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([("A", "iPhone 17")])),
            trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in true }),
            routes: FakeProxyRoute(route: ProxyRoute(
                http: CoreLocations.proxyEndpoint,
                https: CoreLocations.proxyEndpoint
            )),
            certificatePath: "/ca.pem"
        )
        await model.refresh()
        await model.connect(coreIsRunning: true)

        let size = rendered(SimulatorSectionView(model: model, coreConnected: true))

        #expect(size != nil)
        #expect((size?.height ?? 0) > 0)
    }

    @Test("it draws with nothing booted, which is the empty state")
    func drawsEmpty() async {
        let model = SimulatorSessionModel(
            catalog: SimulatorCatalog(runner: FakeSimctl.listing([])),
            trust: SimulatorTrustStore(runner: FakeSimctl(), certificateExists: { _ in true }),
            routes: FakeProxyRoute(route: .direct),
            certificatePath: "/ca.pem"
        )
        await model.refresh()

        let size = rendered(SimulatorSectionView(model: model, coreConnected: false))

        #expect(size != nil)
        #expect((size?.height ?? 0) > 0)
    }
}
