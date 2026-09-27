//  What the list does when the query changes, when pages arrive, and when
//  traffic arrives while someone is reading a request.
//
//  Nothing here sleeps. A query change is awaited because the method that
//  makes it is async; the one thing that happens on a timer — the search
//  settling — is either flushed on purpose or waited for by watching the
//  session for the call it causes.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Traffic list behaviour")
@MainActor
struct TrafficListBehaviourTests {
    /// A model over a session a test controls, with no reconnection and a
    /// settling time long enough that it never fires by itself.
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
        TrafficListResult(
            exchanges: rows,
            nextCursor: next,
            total: total ?? UInt64(rows.count)
        )
    }

    // MARK: - Search

    @Test("typing does not ask the core once per keystroke")
    func searchIsDebounced() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()
        #expect(await session.listCount == 1)

        model.setSearch("a")
        model.setSearch("ap")
        model.setSearch("api")

        // Still one: the request each keystroke scheduled was cancelled by
        // the next, and none of them has settled.
        #expect(await session.listCount == 1)
        #expect(model.filters.search == "api")

        await model.applySearchNow()

        #expect(await session.listCount == 2)
        #expect(await session.listRequests.last?.search == "api")
    }

    @Test("a search that settles on its own asks the core once")
    func searchSettles() async throws {
        let session = FakeSession()
        // Short enough to fire during the test; the test waits for the call
        // it causes rather than for the time it takes.
        let model = model(session, settling: .milliseconds(1))
        await model.start()

        model.setSearch("login")
        await session.waitForListCalls(2)

        #expect(await session.listRequests.last?.search == "login")
    }

    @Test("an empty search asks for everything again")
    func clearingTheSearch() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        model.setSearch("login")
        await model.applySearchNow()
        model.setSearch("")
        await model.applySearchNow()

        #expect(await session.listRequests.last?.search == nil)
        #expect(!model.filters.isNarrowed)
    }

    @Test("a search of spaces is not a search")
    func whitespaceSearch() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        model.setSearch("   ")
        await model.applySearchNow()

        #expect(await session.listRequests.last?.search == nil)
    }

    @Test("special characters are sent as the characters they are")
    func specialCharacters() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        for term in ["50%", "a_b", "back\\slash", "q=1&r=2", "ünïcodé"] {
            model.setSearch(term)
            await model.applySearchNow()

            #expect(await session.listRequests.last?.search == term, "\(term)")
        }
    }

    @Test("a search that changes again before settling asks only for the last one")
    func rapidlyChangingSearch() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        for term in ["l", "lo", "log", "logi", "login"] {
            model.setSearch(term)
        }
        await model.applySearchNow()

        #expect(await session.listCount == 2)
        #expect(await session.listRequests.last?.search == "login")
    }

    // MARK: - Filters

    @Test("each filter is sent to the core as the parameter it is")
    func filtersAreSent() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await model.setMethod("POST")
        #expect(await session.listRequests.last?.method == "POST")

        await model.setStatus(.clientError)
        #expect(await session.listRequests.last?.status == .class(4))

        await model.setHost("api.example.com")
        #expect(await session.listRequests.last?.host == "api.example.com")

        await model.setContentType("json")
        #expect(await session.listRequests.last?.contentType == "json")

        await model.setHTTPVersion(.http2)
        #expect(await session.listRequests.last?.httpVersion == .http2)

        await model.setStreamKind("websocket")
        #expect(await session.listRequests.last?.websocket == true)

        await model.setFailed(true)
        #expect(await session.listRequests.last?.failed == true)

        await model.setStatusCode(404)
        #expect(await session.listRequests.last?.status == .exact(404))

        // And they are all still set: each is one request carrying all of it.
        let last = try #require(await session.listRequests.last)
        #expect(last.method == "POST")
        #expect(last.status == .exact(404))
        #expect(last.host == "api.example.com")
        #expect(last.contentType == "json")
        #expect(last.httpVersion == .http2)
        #expect(last.websocket)
        #expect(last.failed)
    }

    @Test("setting a filter to what it already is asks nothing")
    func unchangedFilter() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await model.setMethod("GET")
        let after = await session.listCount
        await model.setMethod("GET")

        #expect(await session.listCount == after)
    }

    @Test("a search and a filter are one request, not two lists")
    func searchAndFilterCombine() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        model.setSearch("users")
        await model.applySearchNow()
        await model.setMethod("POST")

        let last = try #require(await session.listRequests.last)
        #expect(last.search == "users")
        #expect(last.method == "POST")
    }

    @Test("regex and case flags are sent with the search")
    func searchFlagsAreSent() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        model.setSearch(#"user-\d+"#)
        await model.applySearchNow()
        await model.setSearchRegex(true)
        await model.setSearchCaseSensitive(true)

        let last = try #require(await session.listRequests.last)
        #expect(last.search == #"user-\d+"#)
        #expect(last.searchRegex)
        #expect(last.searchCaseSensitive)
    }

    @Test("clearing the filters reloads without any of them")
    func clearFilters() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        model.setSearch("users")
        await model.applySearchNow()
        await model.setMethod("POST")
        await model.setStatus(.serverError)
        await model.setHost("api.example.com")
        await model.setSort(.oldestFirst)
        #expect(model.filters.isNarrowed)

        await model.clearFilters()

        let last = try #require(await session.listRequests.last)
        #expect(last.search == nil)
        #expect(last.method == nil)
        #expect(last.status == nil)
        #expect(last.host == nil)
        // The order is not a filter, and is not undone by clearing them.
        #expect(last.order == .oldest)
        #expect(!model.filters.isNarrowed)
        #expect(model.exchanges.map(\.id) == ["a"])
    }

    @Test("clearing filters that are already clear asks nothing")
    func clearWithNothingSet() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()
        let after = await session.listCount

        await model.clearFilters()

        #expect(await session.listCount == after)
    }

    // MARK: - Sorting

    @Test("the list is newest first until it is asked not to be")
    func defaultSort() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        #expect(model.filters.sort == .newestFirst)
        #expect(await session.listRequests.first?.order == .newest)
    }

    @Test("the other end of the order is asked for as that")
    func oldestFirst() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await model.setSort(.oldestFirst)

        #expect(await session.listRequests.last?.order == .oldest)
    }

    @Test("newest first puts an arriving exchange at the top")
    func insertNewestFirst() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(page([row("b", seconds: 200), row("a", seconds: 100)]))]
        )
        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(row("c", seconds: 300))))

        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])
    }

    @Test("oldest first puts an arriving exchange at the end")
    func insertOldestFirst() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(page([row("a", seconds: 100), row("b", seconds: 200)]))]
        )
        let model = model(session)
        await model.start()
        await model.setSort(.oldestFirst)

        await model.handle(.event(.trafficAdded(row("c", seconds: 300))))

        #expect(model.exchanges.map(\.id) == ["a", "b", "c"])
    }

    @Test("an exchange out of order goes where the order puts it")
    func insertInTheMiddle() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(page([row("c", seconds: 300), row("a", seconds: 100)]))]
        )
        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(row("b", seconds: 200))))

        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])
    }

    @Test("exchanges from the same millisecond are ordered by id, either way round")
    func sameMillisecond() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        for id in ["b", "a", "c"] {
            await model.handle(.event(.trafficAdded(row(id, seconds: 100))))
        }
        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])

        await model.setSort(.oldestFirst)
        await session.answerLists(with: [.success(page([]))])
        await model.loadFirstPage()
        for id in ["b", "a", "c"] {
            await model.handle(.event(.trafficAdded(row(id, seconds: 100))))
        }

        #expect(model.exchanges.map(\.id) == ["a", "b", "c"])
    }

    @Test("changing the order starts the paging again")
    func sortResetsPaging() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(with: [.success(page([row("a")], next: cursor, total: 9))])
        let model = model(session)
        await model.start()
        #expect(model.hasMoreTraffic)

        await session.answerLists(with: [.success(page([row("a")]))])
        await model.setSort(.oldestFirst)

        // A cursor names a place in the order it came from, so the first page
        // of the other direction is asked for without one.
        let last = try #require(await session.listRequests.last)
        #expect(last.cursor == nil)
        #expect(last.order == .oldest)
    }

    // MARK: - Pagination

    @Test("the first page is asked for with a page size the core accepts")
    func firstPage() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        #expect(await session.listRequests.first?.limit == TrafficInspectorModel.pageSize)
        #expect(TrafficInspectorModel.pageSize <= 1_000)
        #expect(await session.listRequests.first?.cursor == nil)
    }

    @Test("the next page is added after the rows already loaded")
    func nextPage() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 200_000, id: "b")
        await session.answerLists(
            with: [
                .success(page([row("c", seconds: 300), row("b", seconds: 200)], next: cursor, total: 3)),
                .success(page([row("a", seconds: 100)], total: 3)),
            ]
        )
        let model = model(session)
        await model.start()
        #expect(model.hasMoreTraffic)

        await model.loadNextPage()

        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])
        #expect(!model.hasMoreTraffic)
        #expect(await session.listRequests.last?.cursor == cursor)
    }

    @Test("a page that repeats a row does not repeat the row")
    func noDuplicatesAcrossPages() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 200_000, id: "b")
        await session.answerLists(
            with: [
                .success(page([row("c", seconds: 300), row("b", seconds: 200)], next: cursor, total: 3)),
                // `b` again: an exchange that also arrived as an event while
                // the page was in flight.
                .success(page([row("b", seconds: 200), row("a", seconds: 100)], total: 3)),
            ]
        )
        let model = model(session)
        await model.start()

        await model.loadNextPage()

        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])
    }

    @Test("the end of the list is the end of the paging")
    func endOfResults() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        #expect(!model.hasMoreTraffic)
        let after = await session.listCount

        await model.loadNextPage()

        // Nothing to ask for, so nothing is asked.
        #expect(await session.listCount == after)
    }

    @Test("an empty last page ends the paging without emptying the list")
    func emptyFinalPage() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(
            with: [
                .success(page([row("a")], next: cursor, total: 1)),
                .success(page([], total: 1)),
            ]
        )
        let model = model(session)
        await model.start()

        await model.loadNextPage()

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(!model.hasMoreTraffic)
        #expect(model.placeholder == .rows)
    }

    @Test("the same page is not fetched twice at once")
    func noConcurrentPageRequests() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(with: [.success(page([row("a")], next: cursor, total: 9))])
        let model = model(session)
        await model.start()
        let after = await session.listCount

        // Two scroll triggers landing together.
        async let first: Void = model.loadNextPage()
        async let second: Void = model.loadNextPage()
        _ = await (first, second)

        #expect(await session.listCount == after + 1)
    }

    @Test("a query that changes discards the page the old one asked for")
    func pageFromASupersededQuery() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(
            with: [.success(page([row("a", host: "old.example.com")], next: cursor, total: 9))]
        )
        let model = model(session)
        await model.start()

        await session.answerLists(with: [.success(page([row("z", host: "new.example.com")]))])
        await model.setHost("new.example.com")

        #expect(model.exchanges.map(\.id) == ["z"])
        #expect(!model.hasMoreTraffic)
    }

    @Test("more is fetched as the end of the list comes into view")
    func loadsMoreNearTheEnd() async throws {
        let session = FakeSession()
        let rows = (0..<30).map { row("row-\($0)", seconds: UInt64(1_000 - $0)) }
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "row-29")
        await session.answerLists(with: [.success(page(rows, next: cursor, total: 100))])
        let model = model(session)
        await model.start()

        #expect(!model.shouldLoadMore(atRow: 0))
        #expect(model.shouldLoadMore(atRow: 29))

        // And not once the core has said there is nothing more.
        await session.answerLists(with: [.success(page(rows))])
        await model.loadFirstPage()
        #expect(!model.shouldLoadMore(atRow: 29))
    }

    // MARK: - Live traffic

    @Test("an exchange the filters match appears")
    func matchingEventAppears() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()
        await model.setMethod("POST")

        await model.handle(.event(.trafficAdded(row("a", method: "POST"))))

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.totalExchanges == 1)
    }

    @Test("a search reloads when new traffic arrives, because bodies are not on the event")
    func searchReloadsOnArrival() async throws {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(page([])),
            .success(page([])),
            .success(page([row("hit")])),
        ])
        let model = model(session)
        await model.start()

        model.setSearch("secret-in-body")
        await model.applySearchNow()

        await model.handle(.event(.trafficAdded(row("hit"))))
        #expect(model.exchanges.isEmpty)

        await model.applySearchNow()
        #expect(model.exchanges.map(\.id) == ["hit"])
    }

    @Test("an exchange the filters exclude does not appear")
    func nonMatchingEventIsIgnored() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()
        await model.setMethod("POST")

        await model.handle(.event(.trafficAdded(row("a", method: "GET"))))

        #expect(model.exchanges.isEmpty)
        #expect(model.totalExchanges == 0)
        // And it is not the reason to ask the core for the list again.
        #expect(await session.listCount == 2)
    }

    @Test("every filter is applied to an arriving exchange")
    func eventAgainstEveryFilter() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        model.setSearch("users")
        await model.applySearchNow()
        await model.setMethod("POST")
        await model.setStatus(.success)
        await model.setHost("api.example.com")

        await model.handle(
            .event(
                .trafficAdded(
                    row("wanted", method: "POST", host: "api.example.com", path: "/v1/users", status: 201)
                )
            )
        )
        for unwanted in [
            row("m", method: "GET", path: "/v1/users", status: 201),
            row("s", method: "POST", path: "/v1/users", status: 404),
            row("h", method: "POST", host: "cdn.example.com", path: "/v1/users", status: 201),
            row("q", method: "POST", path: "/v1/orders", status: 201),
        ] {
            await model.handle(.event(.trafficAdded(unwanted)))
        }

        #expect(model.exchanges.map(\.id) == ["wanted"])
    }

    @Test("the same exchange arriving twice is one row")
    func duplicateEvent() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(row("a", status: nil))))
        await model.handle(.event(.trafficAdded(row("a", status: 200))))

        #expect(model.exchanges.count == 1)
        #expect(model.exchanges[0].status == 200)
        #expect(model.totalExchanges == 1)
    }

    @Test("oldest first, an arriving exchange waits for the paging to reach it")
    func oldestFirstWithPagesOutstanding() async throws {
        // It belongs at the very end of the whole result set, and the loaded
        // rows do not reach that end yet. Putting it after the last loaded row
        // would show it before exchanges that are older than it.
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(
            with: [.success(page([row("a", seconds: 100)], next: cursor, total: 9))]
        )
        let model = model(session)
        await model.start()
        await model.setSort(.oldestFirst)

        await model.handle(.event(.trafficAdded(row("z", seconds: 900))))

        #expect(model.exchanges.map(\.id) == ["a"])
        // Not lost: the core has one more matching exchange, and the count
        // says so.
        #expect(model.totalExchanges == 10)
    }

    @Test("an exchange that was lost makes the list ask again")
    func trafficLostReloads() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()
        let after = await session.listCount

        await model.handle(.event(.trafficLost(missed: 9)))

        #expect(await session.listCount == after + 1)
    }

    @Test("a reload after a loss carries the filters that are set")
    func trafficLostKeepsFilters() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()
        await model.setMethod("POST")

        await model.handle(.event(.trafficLost(missed: 3)))

        #expect(await session.listRequests.last?.method == "POST")
    }

    // MARK: - Selection

    @Test("traffic arriving does not disturb what is selected")
    func selectionSurvivesNewTraffic() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a", seconds: 100)]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = model(session)
        await model.start()
        await model.select("a")

        for id in ["x", "y", "z"] {
            await model.handle(.event(.trafficAdded(row(id, seconds: 900))))
        }

        #expect(model.selectedID == "a")
        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the selected exchange should still be loaded")
            return
        }
        #expect(exchange.id == "a")
    }

    @Test("the detail never describes a row other than the selected one")
    func detailFollowsSelection() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a"), row("b")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerExchange("b", with: .success(Fixtures.exchange(id: "b")))
        let model = model(session)
        await model.start()

        await model.select("a")
        await model.select("b")

        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the selected exchange should be loaded")
            return
        }
        #expect(exchange.id == model.selectedID)
        #expect(exchange.id == "b")
    }

    @Test("a filter that hides the selected exchange clears the selection")
    func selectionClearedByFilters() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a", method: "GET")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = model(session)
        await model.start()
        await model.select("a")

        // The new query does not hold it, so the detail pane would otherwise
        // be describing a row that is not on screen.
        await session.answerLists(with: [.success(page([]))])
        await model.setMethod("POST")

        #expect(model.selectedID == nil)
        #expect(model.detail == .none)
    }

    @Test("a filter that still shows the selected exchange keeps it selected")
    func selectionKeptByFilters() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a", method: "POST")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = model(session)
        await model.start()
        await model.select("a")

        await model.setMethod("POST")

        #expect(model.selectedID == "a")
        guard case .loaded = model.detail else {
            Issue.record("the exchange is still listed, so it is still shown")
            return
        }
    }

    @Test("deleting the selected exchange clears the selection")
    func deleteClearsSelection() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a"), row("b")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = model(session)
        await model.start()
        await model.select("a")

        await model.deleteSelected()

        #expect(model.selectedID == nil)
        #expect(model.detail == .none)
        #expect(model.exchanges.map(\.id) == ["b"])
    }

    @Test("an exchange deleted elsewhere clears the selection when it is asked for")
    func selectionOfADeletedExchange() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        // No answer configured, so the fake refuses with not_found, the way
        // the core does for an exchange that is not there.
        let model = model(session)
        await model.start()

        await model.select("a")

        #expect(model.detail == .unavailable)
        #expect(model.exchanges.isEmpty)
    }

    // MARK: - What an empty list says

    @Test("an empty core and an unlucky query say different things")
    func placeholders() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        #expect(model.placeholder == .noTraffic)

        await model.setMethod("POST")
        #expect(model.placeholder == .noMatches)

        await model.clearFilters()
        #expect(model.placeholder == .noTraffic)
    }

    @Test("a list with rows in it has nothing to say")
    func placeholderWithRows() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a")]))])
        let model = model(session)
        await model.start()

        #expect(model.placeholder == .rows)
    }

    @Test("a core that is not running says so rather than saying there is no traffic")
    func placeholderWhenDisconnected() async throws {
        let session = FakeSession()
        await session.failConnect(with: .cannotConnect(path: "/tmp/proxylab-test.sock"))
        let model = model(session)
        await model.start()

        guard case .failed = model.placeholder else {
            Issue.record("a list that could not be fetched is not an empty one")
            return
        }
    }

    @Test("a list that could not be fetched says why")
    func placeholderWhenFailed() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.failure(.connectionClosed)])
        let model = model(session)
        await model.start()

        #expect(model.placeholder == .failed(IPCError.connectionClosed.message))
    }

    @Test("a list that fails while filters are set still says it failed")
    func failureBeatsNoMatches() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await session.answerLists(with: [.failure(.connectionClosed)])
        await model.setMethod("POST")

        #expect(model.placeholder == .failed(IPCError.connectionClosed.message))
    }

    @Test("a page that fails while paging leaves the rows that are loaded")
    func pageFailureKeepsRows() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 100_000, id: "a")
        await session.answerLists(with: [.success(page([row("a")], next: cursor, total: 9))])
        let model = model(session)
        await model.start()

        await session.answerLists(with: [.failure(.connectionClosed)])
        await model.loadNextPage()

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.lastError == IPCError.connectionClosed.message)
        #expect(!model.isLoadingMore)
    }

    // MARK: - When the core goes away

    @Test("a connection lost while filters are set keeps the rows and says so")
    func connectionLostWhileFiltered() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([row("a", method: "POST")]))])
        let model = model(session)
        await model.start()
        await model.setMethod("POST")

        // What a malformed line from the core comes to: the client cannot
        // agree with it about the protocol, so the connection ends.
        await model.handle(.connectionLost(.malformedMessage))

        #expect(model.connection == .disconnected(IPCError.malformedMessage.message))
        #expect(model.lastError == IPCError.malformedMessage.message)
        // The rows that were fetched are still what the core last said.
        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.filters.method == "POST")
    }

    @Test("a filter changed while the core is away fails without losing the filter")
    func filterChangeWhileDisconnected() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await session.answerLists(with: [.failure(.notConnected)])
        await model.setStatus(.serverError)

        #expect(model.filters.status == .serverError)
        #expect(model.placeholder == .failed(IPCError.notConnected.message))
    }

    @Test("reconnecting asks for the list the filters describe")
    func reconnectKeepsTheQuery() async throws {
        let session = FakeSession()
        await session.failConnect(with: .cannotConnect(path: "/tmp/proxylab-test.sock"))
        let model = model(session)
        await model.start()

        await session.failConnect(with: nil)
        model.setSearch("login")
        await model.applySearchNow()
        await model.retryConnection()

        #expect(model.connection == .connected)
        #expect(await session.listRequests.last?.search == "login")
    }

    // MARK: - The host menu

    @Test("the hosts offered are the ones in the list")
    func knownHosts() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [
                .success(
                    page([
                        row("a", host: "api.example.com"),
                        row("b", host: "cdn.example.com"),
                        row("c", host: "api.example.com"),
                    ])
                )
            ]
        )
        let model = model(session)
        await model.start()

        #expect(model.knownHosts == ["api.example.com", "cdn.example.com"])
    }

    @Test("a host that is filtered on stays in the menu even when nothing matches")
    func filteredHostStaysOffered() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(page([]))])
        let model = model(session)
        await model.start()

        await model.setHost("gone.example.com")

        // Otherwise the only way out of the filter would be the Clear button.
        #expect(model.knownHosts == ["gone.example.com"])
    }
}
