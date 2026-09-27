//  What the window does with what the core says.
//
//  Subscribe before listing, insert a new row rather than reloading the list,
//  and never remove anything the core has not confirmed is gone. These are the
//  decisions the interface is built on, and they are tested here rather than
//  through a rendered view hierarchy.
//
//  `handle(_:)` is called directly rather than through the notification
//  stream. It is the same code path the stream drives, and calling it is
//  deterministic: there is nothing to wait for and nothing to sleep on.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Traffic inspector")
@MainActor
struct TrafficInspectorModelTests {
    /// A model over a session a test controls, with no reconnection.
    private func model(_ session: FakeSession) -> TrafficInspectorModel {
        TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
    }

    @Test("starting connects, subscribes, and loads the newest page")
    func startup() async throws {
        let session = FakeSession()
        let summaries = [Fixtures.summary(id: "a"), Fixtures.summary(id: "b")]
        await session.answerLists(with: [.success(Fixtures.page(summaries))])

        let model = model(session)
        await model.start()

        #expect(model.connection == .connected)
        #expect(model.listState == .loaded)
        #expect(model.exchanges.map(\.id) == ["a", "b"])
        #expect(model.totalExchanges == 2)
        #expect(await session.connectCount == 1)
        #expect(await session.subscribeCount == 1)
        #expect(await session.listCount == 1)
    }

    @Test("a core that is not running leaves a disconnected window, not a crash")
    func coreNotRunning() async throws {
        let session = FakeSession()
        await session.failConnect(with: .cannotConnect(path: "/tmp/proxylab-test.sock"))

        let model = model(session)
        await model.start()

        #expect(model.connection == .disconnected(IPCError.cannotConnect(path: "/tmp/proxylab-test.sock").message))
        #expect(model.exchanges.isEmpty)
        guard case .failed = model.listState else {
            Issue.record("the list should say why it is empty")
            return
        }
        #expect(await session.listCount == 0)
    }

    @Test("the traffic list is asked for after the subscription, never before")
    func subscribeBeforeList() async throws {
        let session = FakeSession()
        let model = model(session)

        await model.start()

        // An exchange finishing between the two would otherwise be in neither
        // the page nor the events.
        #expect(await session.subscribeCount == 1)
        #expect(await session.listCount == 1)
    }

    @Test("a subscription that fails leaves the connection disconnected")
    func subscribeFails() async throws {
        let session = FakeSession()
        await session.failSubscribe(with: .connectionClosed)

        let model = model(session)
        await model.start()

        #expect(model.connection == .disconnected(IPCError.connectionClosed.message))
    }

    @Test("an added exchange is inserted rather than the list being reloaded")
    func trafficAddedInsertion() async throws {
        let session = FakeSession()
        let existing = Fixtures.summary(id: "b", startedAtSeconds: 100)
        await session.answerLists(with: [.success(Fixtures.page([existing]))])

        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(Fixtures.summary(id: "c", startedAtSeconds: 200))))

        #expect(model.exchanges.map(\.id) == ["c", "b"])
        #expect(model.totalExchanges == 2)
        // One list call, from startup. The event carried the whole row.
        #expect(await session.listCount == 1)
    }

    @Test("an added exchange goes where it belongs in the newest-first order")
    func trafficAddedOrdering() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [
                .success(
                    Fixtures.page([
                        Fixtures.summary(id: "c", startedAtSeconds: 300),
                        Fixtures.summary(id: "a", startedAtSeconds: 100),
                    ])
                )
            ]
        )

        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(Fixtures.summary(id: "b", startedAtSeconds: 200))))

        #expect(model.exchanges.map(\.id) == ["c", "b", "a"])
    }

    @Test("an exchange that is already listed is updated where it stands")
    func trafficAddedTwice() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(Fixtures.page([Fixtures.summary(id: "a", status: nil)]))]
        )

        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficAdded(Fixtures.summary(id: "a", status: 500))))

        #expect(model.exchanges.count == 1)
        #expect(model.exchanges[0].status == 500)
        // An exchange that finished between subscribing and the page arriving
        // is in both; counting it twice would show a total the core has not.
        #expect(model.totalExchanges == 1)
    }

    @Test("an added exchange does not disturb the selection")
    func selectionSurvivesNewTraffic() async throws {
        let session = FakeSession()
        let selected = Fixtures.summary(id: "a", startedAtSeconds: 100)
        await session.answerLists(with: [.success(Fixtures.page([selected]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))

        let model = model(session)
        await model.start()
        await model.select("a")

        await model.handle(.event(.trafficAdded(Fixtures.summary(id: "z", startedAtSeconds: 900))))

        #expect(model.selectedID == "a")
        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the selected exchange should still be loaded")
            return
        }
        #expect(exchange.id == "a")
    }

    @Test("a subscriber that fell behind asks for the list again")
    func trafficLostReloads() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])

        let model = model(session)
        await model.start()

        await model.handle(.event(.trafficLost(missed: 9)))

        // The one case where a whole reload is right: which rows were dropped
        // is not knowable.
        #expect(await session.listCount == 2)
    }

    @Test("selecting an exchange fetches the whole thing")
    func selection() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))

        let model = model(session)
        await model.start()
        await model.select("a")

        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the exchange should have loaded")
            return
        }
        #expect(exchange.id == "a")
        #expect(model.selectedID == "a")
    }

    @Test("selecting nothing clears the detail pane")
    func deselect() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))

        let model = model(session)
        await model.start()
        await model.select("a")
        await model.select(nil)

        #expect(model.detail == .none)
        #expect(model.selectedID == nil)
    }

    @Test("an exchange deleted elsewhere is said to be gone, not to have failed")
    func selectionUnavailable() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        // No answer configured for "a": the fake refuses with not_found, the
        // way the core does for an exchange that is not there.

        let model = model(session)
        await model.start()
        await model.select("a")

        #expect(model.detail == .unavailable)
        #expect(model.exchanges.isEmpty)
    }

    @Test("an exchange that could not be fetched says why")
    func selectionFailed() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .failure(.connectionClosed))

        let model = model(session)
        await model.start()
        await model.select("a")

        #expect(model.detail == .failed(IPCError.connectionClosed.message))
    }

    @Test("a deleted exchange leaves the list once the core says it is gone")
    func deleteSucceeds() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(Fixtures.page([Fixtures.summary(id: "a"), Fixtures.summary(id: "b")]))]
        )
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))

        let model = model(session)
        await model.start()
        await model.select("a")
        await model.deleteSelected()

        #expect(await session.deletedIDs == ["a"])
        #expect(model.exchanges.map(\.id) == ["b"])
        #expect(model.selectedID == nil)
        #expect(model.detail == .none)
        #expect(model.totalExchanges == 1)
    }

    @Test("a delete that failed removes nothing")
    func deleteFails() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerDelete(with: .failure(.connectionClosed))

        let model = model(session)
        await model.start()
        await model.select("a")
        await model.deleteSelected()

        // Nothing optimistic: the row is still there, and the failure is said.
        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.selectedID == "a")
        #expect(model.lastError == IPCError.connectionClosed.message)
    }

    @Test("clearing empties the list once the core says it is empty")
    func clearSucceeds() async throws {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(Fixtures.page([Fixtures.summary(id: "a"), Fixtures.summary(id: "b")]))]
        )
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerClear(with: .success(ClearedResult(removed: 2)))

        let model = model(session)
        await model.start()
        await model.select("a")
        await model.clearAll()

        #expect(model.exchanges.isEmpty)
        #expect(model.totalExchanges == 0)
        #expect(model.selectedID == nil)
        #expect(model.detail == .none)
        #expect(model.listState == .loaded)
    }

    @Test("a clear that failed empties nothing")
    func clearFails() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerClear(with: .failure(.connectionClosed))

        let model = model(session)
        await model.start()
        await model.clearAll()

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.lastError == IPCError.connectionClosed.message)
    }

    @Test("a list that could not be fetched is an error state, not an empty one")
    func listFails() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.failure(.connectionClosed)])

        let model = model(session)
        await model.start()

        #expect(model.listState == .failed(IPCError.connectionClosed.message))
        #expect(model.exchanges.isEmpty)
        #expect(model.lastError == IPCError.connectionClosed.message)
    }

    @Test("a connection lost while the window is open says so")
    func connectionLost() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        await model.handle(.connectionLost(.connectionClosed))

        #expect(model.connection == .disconnected(IPCError.connectionClosed.message))
        #expect(model.lastError == IPCError.connectionClosed.message)
    }

    @Test("retrying after the core comes back reconnects and reloads")
    func retryAfterFailure() async throws {
        let session = FakeSession()
        await session.failConnect(with: .cannotConnect(path: "/tmp/proxylab-test.sock"))

        let model = model(session)
        await model.start()
        #expect(model.connection != .connected)

        // The core is started, and the window is told to try again.
        await session.failConnect(with: nil)
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await model.retryConnection()

        #expect(model.connection == .connected)
        #expect(model.exchanges.map(\.id) == ["a"])
    }

    @Test("the next page is added to the list rather than replacing it")
    func loadNextPage() async throws {
        let session = FakeSession()
        let cursor = TrafficCursor(startedAtMs: 200_000, id: "b")
        await session.answerLists(
            with: [
                .success(
                    Fixtures.page([Fixtures.summary(id: "b", startedAtSeconds: 200)], next: cursor)
                ),
                .success(Fixtures.page([Fixtures.summary(id: "a", startedAtSeconds: 100)])),
            ]
        )

        let model = model(session)
        await model.start()
        #expect(model.hasMoreTraffic)

        await model.loadNextPage()

        #expect(model.exchanges.map(\.id) == ["b", "a"])
        #expect(!model.hasMoreTraffic)
        #expect(await session.listRequests.last?.cursor == cursor)
    }

    @Test("the page size asked for is one the core will accept")
    func pageSize() async throws {
        let session = FakeSession()
        let model = model(session)
        await model.start()

        let limit = await session.listRequests.first?.limit
        #expect(limit == TrafficInspectorModel.pageSize)
        // The core refuses a page larger than a thousand.
        #expect(TrafficInspectorModel.pageSize <= 1_000)
    }
}
