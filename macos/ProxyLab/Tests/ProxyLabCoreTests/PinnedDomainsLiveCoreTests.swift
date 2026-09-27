//  Pinned domains against the real core.
//
//  The rest of the pinned-domain tests run with nothing installed. This one
//  drives the objects the window drives — a real `IPCClient`, a real
//  `TrafficInspectorModel`, a real `PinnedDomainsStore` — against a running
//  `proxy-engine` over its actual socket, and is skipped unless one is pointed
//  at:
//
//      PROXYLAB_IPC_SOCKET=~/.proxylab/run/proxylab.sock swift test
//
//  It is read-only against the developer's traffic. New traffic is made by
//  asking the core to replay a request, which is the one way to produce a
//  `traffic_added` event without a browser; nothing is deleted, and the one
//  test that clears the store runs only when a second variable says the core
//  being pointed at is a scratch one:
//
//      PROXYLAB_TEST_ALLOW_CLEAR=1
//
//  The host it exercises is whichever host the core has the most traffic for,
//  so the suite adapts to whatever the core in front of it has carried. A core
//  that has carried nothing yet leaves these tests with nothing to assert, and
//  they return rather than fail: an empty proxy is not a broken client, which
//  is the same line `CoreIntegrationTests` takes.

import Foundation
import Testing

@testable import ProxyLabCore

/// The socket a running core is listening on, if a test run named one.
private let liveCoreSocket: String? = {
    guard
        let path = ProcessInfo.processInfo.environment[IPCSocketLocation.environmentVariable],
        !path.isEmpty,
        FileManager.default.fileExists(atPath: (path as NSString).expandingTildeInPath)
    else {
        return nil
    }

    return (path as NSString).expandingTildeInPath
}()

/// Whether the core being pointed at is one a test may empty.
private let clearingIsAllowed =
    ProcessInfo.processInfo.environment["PROXYLAB_TEST_ALLOW_CLEAR"] == "1"

@Suite(
    "Pinned domains against a running core",
    .enabled(if: liveCoreSocket != nil, "set PROXYLAB_IPC_SOCKET to a running core's socket"),
    .serialized
)
@MainActor
struct PinnedDomainsLiveCoreTests {
    /// A model reading a real core, with no reconnection and no search timer.
    private func liveModel() async throws -> TrafficInspectorModel {
        let path = try #require(liveCoreSocket)
        let model = TrafficInspectorModel(
            session: IPCClient.overSocket(at: path),
            socketPath: path,
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        try #require(model.connection == .connected, "the core did not answer")

        return model
    }

    private func store(_ domains: [String] = []) -> PinnedDomainsStore {
        let pinned = PinnedDomainsStore(storage: FakePinnedDomainsStorage())
        for domain in domains {
            pinned.pin(domain)
        }

        return pinned
    }

    /// The host the core has carried the most traffic for, and how much.
    private func busiestHost(_ model: TrafficInspectorModel) -> (host: String, rows: Int)? {
        var counts: [String: Int] = [:]
        for summary in model.exchanges {
            guard let host = TrafficDisplay.filterHost(summary) else { continue }

            counts[host, default: 0] += 1
        }

        return counts.max { $0.value < $1.value }.map { ($0.key, $0.value) }
    }

    // MARK: - Test 1 and 2: pin, then navigate

    @Test("a host pinned from a real row shows only that host's real traffic")
    func pinAndNavigate() async throws {
        let model = try await liveModel()
        let pinned = store()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }

        // Test 1: pin the host off a row, exactly as the context menu does.
        pinned.pin(host)
        #expect(pinned.domains == [host])
        #expect(pinned.isPinned(host))

        // Test 2: selecting it narrows the list to that host.
        await model.setHost(host)

        #expect(model.listState == .loaded)
        #expect(!model.exchanges.isEmpty, "the busiest host came back with nothing")
        for summary in model.exchanges {
            #expect(
                TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host) == .orderedSame,
                "\(summary.url) is not \(host)"
            )
        }
    }

    // MARK: - Test 3: several hosts

    @Test("switching between pinned hosts shows each one's own traffic")
    func severalHosts() async throws {
        let model = try await liveModel()
        var hosts = Set(model.exchanges.compactMap(TrafficDisplay.filterHost))
        guard hosts.count >= 2 else { return nothingCarried("two hosts") }
        // Deterministic order, and at most three: the point is switching.
        let chosen = Array(hosts.sorted().prefix(3))
        hosts.removeAll()

        let pinned = store(chosen)
        #expect(pinned.domains == chosen)

        for host in chosen {
            await model.setHost(host)

            #expect(!model.exchanges.isEmpty, "\(host) came back with nothing")
            for summary in model.exchanges {
                #expect(
                    TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host)
                        == .orderedSame
                )
            }
        }

        // And back to everything, which must be at least as much as any one
        // host had.
        let narrowed = model.totalExchanges
        await model.setHost(nil)
        #expect(model.totalExchanges >= narrowed)
    }

    // MARK: - Tests 4 and 5: unpin, and persistence

    @Test("unpinning removes one host and a relaunch keeps the rest")
    func unpinAndPersist() async throws {
        let storage = FakePinnedDomainsStorage()
        let first = PinnedDomainsStore(storage: storage)
        first.pin("api.example.com")
        first.pin("auth.example.com")
        first.pin("cdn.example.com")

        first.unpin("auth.example.com")
        #expect(first.domains == ["api.example.com", "cdn.example.com"])

        // A second store over the same preferences is what a relaunch is.
        let second = PinnedDomainsStore(storage: storage)
        #expect(second.domains == ["api.example.com", "cdn.example.com"])
        #expect(!second.isPinned("auth.example.com"))
    }

    // MARK: - Tests 6 and 7: search, method, status

    @Test("a pinned host and a search are both applied by the core")
    func hostAndSearch() async throws {
        let model = try await liveModel()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }
        await model.setHost(host)
        // A fragment of a path the core really has, taken from a real row.
        let terms = model.exchanges.compactMap { summary -> String? in
            let path = TrafficDisplay.path(summary.url)
            guard path.count > 4 else { return nil }

            return String(path.dropFirst().prefix(4))
        }
        guard let term = terms.first else { return nothingCarried("a path to search for") }

        model.setSearch(term)
        await model.applySearchNow()

        #expect(model.listState == .loaded)
        for summary in model.exchanges {
            #expect(TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host) == .orderedSame)
            let fields = [summary.url, summary.method, summary.status.map(String.init) ?? ""]
            #expect(
                fields.contains { $0.containsASCIICaseInsensitive(term) },
                "\(summary.url) does not contain \(term)"
            )
        }
    }

    @Test("a pinned host and a method are both applied by the core")
    func hostAndMethod() async throws {
        let model = try await liveModel()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }
        await model.setHost(host)
        guard let method = model.exchanges.first?.method else { return nothingCarried() }

        await model.setMethod(method)

        #expect(model.listState == .loaded)
        for summary in model.exchanges {
            #expect(TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host) == .orderedSame)
            #expect(summary.method == method)
        }
    }

    @Test("a pinned host and a status class are both applied by the core")
    func hostAndStatus() async throws {
        let model = try await liveModel()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }
        await model.setHost(host)
        let groups = model.exchanges.compactMap { $0.status }
            .compactMap { StatusClass(rawValue: Int($0) / 100) }
        guard let group = groups.first else { return nothingCarried("an answered request") }

        await model.setStatus(group)

        #expect(model.listState == .loaded)
        for summary in model.exchanges {
            #expect(TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host) == .orderedSame)
            let status = try #require(summary.status)
            #expect(group.contains(status))
        }
    }

    // MARK: - Test 8: live traffic

    @Test("new traffic for the pinned host appears, and traffic for another does not")
    func liveTraffic() async throws {
        let path = try #require(liveCoreSocket)
        let client = IPCClient.overSocket(at: path)
        let model = TrafficInspectorModel(
            session: client,
            socketPath: path,
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        try #require(model.connection == .connected)

        // Two hosts the core demonstrably reached: a row with a status is one
        // the origin answered, which an unreachable address never has. Picking
        // blindly would mean replaying to something that is not listening and
        // waiting out a TCP timeout.
        let urls = Self.reachablePlaintextURLs(model)
        guard urls.count >= 2 else {
            await model.stop()

            return nothingCarried("two plaintext hosts it reached")
        }

        let hosts = urls.keys.sorted()
        let pinnedHost = hosts[0]
        let otherHost = hosts[1]
        await model.setHost(pinnedHost)
        let before = model.exchanges.map(\.id)

        // Asking the core to send a request is the one way to make traffic
        // without a browser. It goes through the proxy's own pipeline and is
        // published on the subscription like any other exchange.
        _ = try await client.replay(
            ReplayRequest(method: "GET", url: urls[pinnedHost]!)
        )
        _ = try await client.replay(
            ReplayRequest(method: "GET", url: urls[otherHost]!)
        )

        // The events arrive on the stream the model is already consuming.
        try await waitUntil("the matching row arrives") {
            model.exchanges.map(\.id) != before
        }

        let arrived = model.exchanges.filter { !before.contains($0.id) }
        #expect(!arrived.isEmpty)
        for summary in arrived {
            #expect(
                TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(pinnedHost)
                    == .orderedSame,
                "\(summary.url) should not be in a list narrowed to \(pinnedHost)"
            )
        }
        // And no row appeared twice.
        #expect(Set(model.exchanges.map(\.id)).count == model.exchanges.count)

        await model.stop()
    }

    // MARK: - Test 10: the selection stays put

    @Test("traffic arriving under a pinned host does not move the selection")
    func selectionSurvivesLiveTraffic() async throws {
        let path = try #require(liveCoreSocket)
        let client = IPCClient.overSocket(at: path)
        let model = TrafficInspectorModel(
            session: client,
            socketPath: path,
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        guard let (host, _) = busiestHost(model), let url = Self.reachablePlaintextURLs(model)[host]
        else {
            await model.stop()

            return nothingCarried("a plaintext host it reached")
        }
        await model.setHost(host)
        guard let chosen = model.exchanges.first else {
            await model.stop()

            return nothingCarried()
        }
        await model.select(chosen.id)
        try #require(model.selectedID == chosen.id)
        let before = model.exchanges.count

        _ = try await client.replay(ReplayRequest(method: "GET", url: url))
        try await waitUntil("the new row arrives") { model.exchanges.count > before }

        #expect(model.selectedID == chosen.id, "the selection moved")
        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the detail pane stopped showing the selected exchange")
            await model.stop()

            return
        }
        #expect(exchange.id == chosen.id)

        await model.stop()
    }

    // MARK: - Test 11: pagination

    @Test("a pinned host pages correctly, in order and without duplicates")
    func pagination() async throws {
        let model = try await liveModel()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }
        await model.setHost(host)

        guard model.hasMoreTraffic else {
            // Not enough traffic for this host to span a page; there is
            // nothing to page through and that is not a failure.
            return
        }

        let firstPage = model.exchanges.map(\.id)
        #expect(firstPage.count == TrafficInspectorModel.pageSize)

        await model.loadNextPage()

        let all = model.exchanges
        #expect(all.count > firstPage.count, "the next page added nothing")
        #expect(Set(all.map(\.id)).count == all.count, "a row appeared twice")
        // Every row is still the pinned host's.
        for summary in all {
            #expect(TrafficDisplay.filterHost(summary)?.caseInsensitiveCompare(host) == .orderedSame)
        }
        // And still newest first, which is the order that was asked for.
        let moments = all.map(\.startedAt.milliseconds)
        #expect(moments == moments.sorted(by: >), "the pages are out of order")

        // Oldest first must be the same rows from the other end.
        await model.setSort(.oldestFirst)
        let oldest = model.exchanges.map(\.startedAt.milliseconds)
        #expect(oldest == oldest.sorted(), "the reversed order is not sorted")
        #expect(model.filters.host == host)
    }

    // MARK: - Test 9: clearing

    @Test(
        "clearing the core's traffic empties the list and leaves the pins alone",
        .enabled(if: clearingIsAllowed, "set PROXYLAB_TEST_ALLOW_CLEAR=1 for a scratch core")
    )
    func clearing() async throws {
        let model = try await liveModel()
        guard let (host, _) = busiestHost(model) else { return nothingCarried() }
        let pinned = store([host, "kept.example.com"])
        await model.setHost(host)

        await model.clearAll()

        #expect(model.exchanges.isEmpty)
        #expect(model.totalExchanges == 0)
        // The pins are this application's preferences; clearing is the core's.
        #expect(pinned.domains == [host, "kept.example.com"])
        #expect(pinned.isPinned(host))
        // And the quiet domain says so rather than looking like a failure.
        #expect(model.placeholder == .noHostTraffic(host))
    }

    /// Says why a test had nothing to assert, without failing it.
    ///
    /// A core that has carried nothing, or nothing of the shape a test needs,
    /// is a proxy nobody has used yet — not a broken client. The note goes in
    /// the run's output so a skipped assertion is visible rather than silent.
    private func nothingCarried(_ what: String = "any traffic") {
        print("skipped: the core has not carried \(what) to assert on")
    }

    /// One plaintext URL per host the core actually reached.
    ///
    /// A row with a status is a row an origin answered. A host whose rows are
    /// all errors is one nothing is listening on, and replaying to it would
    /// mean waiting out a connection timeout rather than testing anything.
    private static func reachablePlaintextURLs(_ model: TrafficInspectorModel) -> [String: String] {
        Dictionary(
            model.exchanges.compactMap { summary -> (String, String)? in
                guard summary.status != nil,
                    summary.url.hasPrefix("http://"),
                    let host = TrafficDisplay.filterHost(summary)
                else {
                    return nil
                }

                return (host, summary.url)
            },
            uniquingKeysWith: { first, _ in first }
        )
    }

    /// Polls `condition` until it holds, or fails the test.
    ///
    /// A real core answers on its own schedule, so this waits for the thing
    /// that must happen rather than for a length of time — and gives up
    /// rather than hanging the suite.
    private func waitUntil(
        _ what: String,
        within: Duration = .seconds(10),
        _ condition: () -> Bool
    ) async throws {
        let deadline = ContinuousClock.now.advanced(by: within)

        while ContinuousClock.now < deadline {
            if condition() { return }

            try await Task.sleep(for: .milliseconds(25))
        }

        Issue.record("timed out waiting for \(what)")
    }
}
