//  The pinned-domain views, actually drawn.
//
//  Same reasoning as `RuleViewRenderingTests`: a SwiftUI view that compiles is
//  not one that lays out. A body that traps, a binding read while it is being
//  written, or a frame that resolves to nothing all happen when the view is
//  rendered. `ImageRenderer` runs a body with no window and nobody watching,
//  which is what makes that testable here.
//
//  Smoke tests, deliberately not snapshot comparisons: what is asserted is
//  that each view draws something at a sensible size, not that it draws the
//  same pixels as last time.
//
//  `NavigationSplitView` is not something `ImageRenderer` can draw, so the
//  sidebar is rendered as the list it is built from.

import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Pinned domain views")
struct PinnedViewRenderingTests {
    /// Lays `view` out offscreen and gives back the size it came out at.
    private func rendered(_ view: some View, width: CGFloat = 220) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    private func store(_ domains: [String]) -> PinnedDomainsStore {
        let pinned = PinnedDomainsStore(storage: FakePinnedDomainsStorage())
        for domain in domains {
            pinned.pin(domain)
        }

        return pinned
    }

    private func model(host: String? = nil) async -> TrafficInspectorModel {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(TrafficListResult(exchanges: [], nextCursor: nil, total: 0))]
        )
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await model.start()
        if let host {
            await model.setHost(host)
        }

        return model
    }

    @Test("the sidebar draws captured domains under the pinned list")
    func sidebarWithCapturedDomains() async throws {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(
                TrafficListResult(
                    exchanges: [
                        ExchangeSummary(
                            id: "a",
                            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                            client: "127.0.0.1:1",
                            scheme: .https,
                            destination: Destination(host: "api.example.com", port: 443),
                            method: "GET",
                            url: "https://api.example.com/v1",
                            version: .http11,
                            completed: true
                        ),
                        ExchangeSummary(
                            id: "b",
                            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
                            client: "127.0.0.1:1",
                            scheme: .https,
                            destination: Destination(host: "cdn.example.com", port: 443),
                            method: "GET",
                            url: "https://cdn.example.com/a.js",
                            version: .http11,
                            completed: true
                        ),
                    ],
                    nextCursor: nil,
                    total: 2
                )
            )
        ])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await model.start()

        let view = PinnedDomainsView(model: model, pinned: store(["api.example.com"]))
        let size = try #require(rendered(view.frame(height: 420)))
        #expect(size.height > 0)
        #expect(model.knownHosts == ["api.example.com", "cdn.example.com"])
    }

    @Test("the sidebar draws with domains pinned")
    func sidebarWithDomains() async throws {
        let view = await PinnedDomainsView(
            model: model(host: "api.example.com"),
            pinned: store(["api.example.com", "auth.example.com", "cdn.example.com"])
        )

        let size = try #require(rendered(view.frame(height: 320)))
        #expect(size.width > 0)
        #expect(size.height > 0)
    }

    @Test("the sidebar draws its empty state when nothing is pinned")
    func sidebarEmpty() async throws {
        let view = await PinnedDomainsView(model: model(), pinned: store([]))

        let size = try #require(rendered(view.frame(height: 320)))
        #expect(size.width > 0)
        #expect(size.height > 0)
    }

    @Test("the sidebar draws the host it is filtering on when nobody pinned it")
    func sidebarWithUnpinnedFilter() async throws {
        let view = await PinnedDomainsView(
            model: model(host: "staging.example.com"),
            pinned: store(["api.example.com"])
        )

        let size = try #require(rendered(view.frame(height: 320)))
        #expect(size.height > 0)
    }

    @Test("a pinned row draws, however long the domain is")
    func rowDraws() throws {
        let pinned = store(["api.example.com"])

        for domain in ["api.example.com", "a.very.long.subdomain.chain.example.com", "::1"] {
            let size = try #require(rendered(PinnedDomainRow(domain: domain, pinned: pinned)))
            #expect(size.width > 0, "\(domain) drew nothing")
            #expect(size.height > 0, "\(domain) drew nothing")
        }
    }

    @Test("the unpinned host row draws")
    func unpinnedRowDraws() throws {
        let size = try #require(
            rendered(UnpinnedHostRow(host: "staging.example.com", pinned: store([])))
        )

        #expect(size.width > 0)
        #expect(size.height > 0)
    }

    @Test("the traffic list draws its quiet-domain empty state")
    func quietDomainPlaceholder() async throws {
        let model = await model(host: "api.example.com")
        #expect(model.placeholder == .noHostTraffic("api.example.com"))

        let view = FocusedTrafficList(
            model: model,
            breakpoints: BreakpointsModel(session: FakeBreakpointsSession()),
            saved: SavedRequestsModel(session: FakeSavedRequestsSession(), traffic: FakeSession()),
            pinned: store(["api.example.com"])
        )

        let size = try #require(rendered(view.frame(height: 320), width: 360))
        #expect(size.height > 0)
    }
}

/// The traffic list, with the search focus the filter bar now takes.
private struct FocusedTrafficList: View {
    let model: TrafficInspectorModel
    let breakpoints: BreakpointsModel
    let saved: SavedRequestsModel
    let pinned: PinnedDomainsStore
    @FocusState private var searchFocused: Bool

    var body: some View {
        TrafficListView(
            model: model,
            breakpoints: breakpoints,
            saved: saved,
            pinned: pinned,
            selection: .constant(nil),
            searchFocused: $searchFocused
        )
    }
}
