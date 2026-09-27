//  What selecting a pinned domain actually does.
//
//  A pin is a preference; navigating to one is the existing host filter. So
//  what these tests assert is that the sidebar produces the same
//  `list_traffic` the Host menu does, that it composes with everything else
//  the query already carries, and that nothing about live traffic, paging, or
//  the selected exchange changes because a domain happens to be selected.
//
//  Nothing here sleeps. The settling time is long enough that the search timer
//  never fires by itself, and the one test about searching flushes it on
//  purpose.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Pinned domain navigation")
@MainActor
struct PinnedDomainNavigationTests {
    private func model(
        _ session: FakeSession,
        settling: Duration = .seconds(600)
    ) -> TrafficInspectorModel {
        TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual,
            searchSettlingTime: settling
        )
    }

    private func store(_ domains: [String] = []) -> PinnedDomainsStore {
        let storage = FakePinnedDomainsStorage()
        let pinned = PinnedDomainsStore(storage: storage)
        for domain in domains {
            pinned.pin(domain)
        }

        return pinned
    }

    /// A summary with the fields the filters look at.
    private func row(
        _ id: String,
        seconds: UInt64 = 1_700_000_000,
        method: String = "GET",
        host: String = "api.example.com",
        path: String = "/v1/users",
        status: UInt16? = 200
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: seconds, nanos: 0),
            durationMicros: 1_000,
            client: "127.0.0.1:54321",
            scheme: .https,
            destination: Destination(host: host, port: 443),
            method: method,
            url: "https://\(host)\(path)",
            version: .http11,
            status: status,
            completed: true
        )
    }

    private func page(
        _ rows: [ExchangeSummary],
        next: TrafficCursor? = nil,
        total: UInt64? = nil
    ) -> TrafficListResult {
        TrafficListResult(exchanges: rows, nextCursor: next, total: total ?? UInt64(rows.count))
    }

    /// The most recent `list_traffic` the model sent.
    private func lastRequest(_ session: FakeSession) async -> ListTrafficParameters {
        let requests = await session.listRequests
        guard let last = requests.last else {
            Issue.record("the core was never asked for a list")

            return ListTrafficParameters()
        }

        return last
    }

    // MARK: - Navigating to a domain

    @Test("selecting a pinned domain asks the core for that host")
    func selectingADomain() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        await model.setHost("api.example.com")

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(model.filters.host == "api.example.com")
        // The query it came from, not a second one built beside it.
        #expect(request.limit == TrafficInspectorModel.pageSize)
        #expect(request.cursor == nil)
    }

    @Test("selecting the domain already selected asks the core for nothing")
    func reselectingCostsNothing() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")
        let calls = await session.listCount

        await model.setHost("api.example.com")

        #expect(await session.listCount == calls)
    }

    @Test("all traffic removes the host and keeps every other filter")
    func allTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setMethod("POST")
        await model.setStatus(.clientError)
        await model.setSort(.oldestFirst)
        await model.setHost("api.example.com")

        await model.setHost(nil)

        let request = await lastRequest(session)
        #expect(request.host == nil)
        // The domain went; nothing else did.
        #expect(request.method == "POST")
        #expect(request.status == .class(4))
        #expect(request.order == .oldest)
        #expect(model.filters.method == "POST")
        #expect(model.filters.status == .clientError)
    }

    @Test("switching between pinned domains narrows to each in turn")
    func switchingDomains() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        await model.setHost("api.example.com")
        #expect(await lastRequest(session).host == "api.example.com")

        await model.setHost("auth.example.com")
        #expect(await lastRequest(session).host == "auth.example.com")

        await model.setHost("cdn.example.com")
        #expect(await lastRequest(session).host == "cdn.example.com")
    }

    // MARK: - Combining with everything else

    @Test("a pinned domain and a search are both sent")
    func domainAndSearch() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        model.setSearch("login")
        await model.applySearchNow()

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.search == "login")
    }

    @Test("a search does not lose the pinned domain")
    func searchKeepsTheDomain() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        model.setSearch("login")
        await model.applySearchNow()

        await model.setHost("api.example.com")

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.search == "login")
    }

    @Test("a pinned domain and a method are both sent")
    func domainAndMethod() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.setMethod("POST")

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.method == "POST")
    }

    @Test("a pinned domain and a status class are both sent")
    func domainAndStatus() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.setStatus(.clientError)

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.status == .class(4))
    }

    @Test("a domain, a search, a method, and a status all travel together")
    func everythingAtOnce() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        await model.setHost("api.example.com")
        await model.setMethod("POST")
        await model.setStatus(.clientError)
        model.setSearch("login")
        await model.applySearchNow()

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.method == "POST")
        #expect(request.status == .class(4))
        #expect(request.search == "login")
        #expect(model.filters.activeCount == 4)
    }

    @Test("clearing the filters clears the domain with them")
    func clearingFilters() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")
        await model.setMethod("POST")

        await model.clearFilters()

        let request = await lastRequest(session)
        #expect(request.host == nil)
        #expect(request.method == nil)
        #expect(model.filters.host == nil)
    }

    // MARK: - Sorting and paging

    @Test("a pinned domain keeps its host when the sort is turned around")
    func domainAndSort() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.setSort(.oldestFirst)
        var request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.order == .oldest)

        await model.setSort(.newestFirst)
        request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.order == .newest)
    }

    @Test("the next page of a pinned domain is still that domain's page")
    func domainAndPaging() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 1_700_000_000_000, id: "a")
        await session.answerLists(
            with: [
                // The page `start()` loads, then the domain's first page, then
                // the page after it.
                .success(page([])),
                .success(page([row("a", seconds: 1_700_000_002)], next: cursor, total: 2)),
                .success(page([row("b", seconds: 1_700_000_001)], total: 2)),
            ]
        )
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.loadNextPage()

        let request = await lastRequest(session)
        #expect(request.host == "api.example.com")
        #expect(request.cursor == cursor)
        // One row per exchange, in order, and neither of them twice.
        #expect(model.exchanges.map(\.id) == ["a", "b"])
    }

    @Test("changing the domain starts the paging again rather than carrying on")
    func changingDomainRestartsPaging() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 1_700_000_000_000, id: "a")
        await session.answerLists(
            with: [
                .success(page([row("a")], next: cursor, total: 9)),
                .success(page([row("z", host: "auth.example.com")])),
            ]
        )
        let model = model(session)
        await model.start()

        await model.setHost("auth.example.com")

        let request = await lastRequest(session)
        // A cursor names a place in the order it was produced in, and this is
        // a different order.
        #expect(request.cursor == nil)
        #expect(model.exchanges.map(\.id) == ["z"])
    }

    // MARK: - Live traffic

    @Test("live traffic for the pinned domain appears")
    func liveMatchingTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.handle(
            .event(.trafficAdded(row("new", seconds: 1_700_000_100, path: "/v1/orders")))
        )

        #expect(model.exchanges.map(\.id).contains("new"))
        #expect(model.totalExchanges == 2)
    }

    @Test("live traffic for another domain does not appear")
    func liveNonMatchingTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.handle(
            .event(.trafficAdded(row("other", seconds: 1_700_000_100, host: "other.example.com")))
        )

        #expect(!model.exchanges.map(\.id).contains("other"))
        // Not counted either: the core would not have listed it.
        #expect(model.totalExchanges == 1)
    }

    @Test("a domain is matched the way the core matches it, whatever the case")
    func liveTrafficFoldsCase() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()
        await model.setHost("API.Example.com")

        await model.handle(.event(.trafficAdded(row("a", host: "api.example.com"))))

        #expect(model.exchanges.map(\.id) == ["a"])
    }

    @Test("an arriving row is not added twice")
    func liveTrafficIsNotDuplicated() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.handle(.event(.trafficAdded(row("a"))))
        await model.handle(.event(.trafficAdded(row("a"))))

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.totalExchanges == 1)
    }

    @Test("live traffic under a pinned domain does not move the selection")
    func selectionSurvivesLiveTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a"), row("b", seconds: 1_699_999_999)]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")
        await model.select("a")

        await model.handle(.event(.trafficAdded(row("new", seconds: 1_700_000_100))))
        await model.handle(
            .event(.trafficAdded(row("ignored", seconds: 1_700_000_101, host: "other.example.com")))
        )

        #expect(model.selectedID == "a")
        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the detail pane stopped showing the selected exchange")

            return
        }
        #expect(exchange.id == "a")
    }

    // MARK: - Several URLs, one domain

    @Test("every path on one host is one domain")
    func onePinPerHost() async throws {
        let pinned = store()
        let rows = [
            row("a", path: "/v1/users"),
            row("b", path: "/v1/orders?active=true"),
            row("c", path: "/v2/users"),
        ]

        for summary in rows {
            if let host = TrafficDisplay.filterHost(summary) {
                pinned.pin(host)
            }
        }

        #expect(pinned.domains == ["api.example.com"])
    }

    @Test("the host that is pinned is the one the core filters on")
    func pinsTheFilterableHost() async throws {
        // A URL whose host is not the destination the proxy connected to.
        // The core filters on the destination, so that is what is pinned.
        let summary = ExchangeSummary(
            id: "a",
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:54321",
            scheme: .http,
            destination: Destination(host: "origin.internal", port: 8_080),
            method: "GET",
            url: "http://vhost.example.com/x",
            version: .http11,
            status: 200,
            completed: true
        )

        #expect(TrafficDisplay.filterHost(summary) == "origin.internal")
        // What a row shows is still the URL's host, which is what a developer
        // recognises. The two are separate on purpose.
        #expect(TrafficDisplay.host(summary) == "vhost.example.com")
    }

    @Test("an exchange with no destination has no domain to pin")
    func noDestination() async throws {
        let summary = ExchangeSummary(
            id: "a",
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:54321",
            scheme: .http,
            destination: nil,
            method: "GET",
            url: "/not-absolute",
            version: .http11,
            completed: true
        )

        #expect(TrafficDisplay.filterHost(summary) == nil)
    }

    @Test("unpinning the domain in view leaves the list where it was")
    func unpinningWhileFiltered() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let pinned = store(["api.example.com"])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")
        let calls = await session.listCount

        pinned.unpin("api.example.com")

        // The pin went; the filter did not. Unpinning is about the sidebar,
        // and silently widening the list under someone would be a surprise.
        #expect(!pinned.isPinned("api.example.com"))
        #expect(model.filters.host == "api.example.com")
        #expect(model.exchanges.map(\.id) == ["a"])
        // And it cost no request: a pin is a preference, not a query.
        #expect(await session.listCount == calls)
    }

    @Test("pinning the domain in view costs no request either")
    func pinningWhileFiltered() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let pinned = store()
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")
        let calls = await session.listCount

        pinned.pin("api.example.com")

        #expect(pinned.domains == ["api.example.com"])
        #expect(model.filters.host == "api.example.com")
        #expect(await session.listCount == calls)
    }

    // MARK: - Clearing traffic

    @Test("clearing the traffic empties the list and keeps the pins")
    func clearingTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        await session.answerClear(with: .success(ClearedResult(removed: 1)))
        let pinned = store(["api.example.com", "auth.example.com"])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.clearAll()

        #expect(model.exchanges.isEmpty)
        #expect(model.totalExchanges == 0)
        // The pins are preferences; the traffic store is the core's.
        #expect(pinned.domains == ["api.example.com", "auth.example.com"])
        #expect(model.filters.host == "api.example.com")
    }

    // MARK: - Empty states

    @Test("a pinned domain with no traffic says so, rather than blaming the filters")
    func quietDomain() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        await model.setHost("api.example.com")

        #expect(model.placeholder == .noHostTraffic("api.example.com"))
    }

    @Test("a domain with other filters on it is a filter miss, not a quiet domain")
    func quietDomainWithFilters() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()
        await model.setHost("api.example.com")

        await model.setMethod("POST")

        #expect(model.placeholder == .noMatches)
    }

    @Test("no traffic at all is still no traffic at all")
    func nothingAnywhere() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        #expect(model.placeholder == .noTraffic)
    }

    @Test("a quiet domain is still a pinned domain")
    func quietDomainStaysPinned() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let pinned = store(["api.example.com"])
        let model = model(session)
        await model.start()

        await model.setHost("api.example.com")

        #expect(model.placeholder == .noHostTraffic("api.example.com"))
        #expect(pinned.isPinned("api.example.com"))
    }
}
