//  The traffic-row context menu: which actions a row offers, and that each
//  one calls the path the rest of the window already uses.
//
//  Replay opens the same sheet Edit & Replay opens. Copy as cURL asks the
//  core for the command. Save Request opens the naming prompt. Pin toggles
//  the pinned-domain store. Host, method, and status go through the filters
//  the sidebar and the filter bar already set, and they leave the others
//  set. Delete is the toolbar's delete.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Traffic row context menu")
struct TrafficRowMenuTests {
    private func row(
        id: String = "a",
        method: String = "GET",
        url: String = "https://api.example.com/products",
        status: UInt16? = 200,
        scheme: Scheme = .https,
        version: HTTPVersion = .http11,
        destination: Destination? = Destination(host: "api.example.com", port: 443),
        responseContentType: String? = "application/json",
        error: TrafficError? = nil
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:1",
            scheme: scheme,
            destination: destination,
            method: method,
            url: url,
            version: version,
            status: status,
            responseContentType: responseContentType,
            completed: error == nil,
            error: error
        )
    }

    private func menu(_ summary: ExchangeSummary, pinned: Bool = false) -> TrafficRowMenu {
        TrafficRowMenu(summary: summary, isPinned: pinned)
    }

    private func started(
        _ session: FakeSession,
        clipboard: FakeClipboard = FakeClipboard()
    ) async -> TrafficInspectorModel {
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            clipboard: clipboard,
            reconnect: .manual
        )
        await model.start()

        return model
    }

    // MARK: - What a row offers

    @Test("a normal HTTPS request offers every action")
    func https() {
        let actions = menu(row())

        #expect(actions.canReplay)
        #expect(actions.canCopyCurl)
        #expect(actions.canBreakpoint)
        #expect(actions.canSave)
        #expect(actions.canDelete)
        #expect(actions.domainActionTitle == "Pin Domain")
        #expect(actions.host == "api.example.com")
        #expect(actions.canFilterMethod)
        #expect(actions.method == "GET")
        #expect(actions.statusClass == .success)
        #expect(actions.showsNarrowing)
    }

    @Test("plaintext HTTP and HTTP/2 are requests that can be sent again")
    func httpAndHTTP2() {
        let http = menu(row(scheme: .http, version: .http11))
        let http2 = menu(row(status: 204, version: .http2))

        #expect(http.canReplay)
        #expect(http.canCopyCurl)
        #expect(http2.canReplay)
        #expect(http2.canCopyCurl)
        #expect(http2.statusClass == .success)
    }

    @Test("a failed exchange can be replayed, and has no status to filter by")
    func failed() {
        let actions = menu(
            row(
                status: nil,
                destination: Destination(host: "api.example.com", port: 443),
                error: TrafficError(kind: .upstreamConnect, summary: "could not connect")
            )
        )

        #expect(actions.canReplay)
        #expect(actions.canCopyCurl)
        #expect(actions.canSave)
        #expect(actions.canDelete)
        #expect(actions.statusClass == nil)
        #expect(actions.domainActionTitle == "Pin Domain")
    }

    @Test("a WebSocket handshake is not offered as a replay or a cURL command")
    func websocket() {
        let actions = menu(row(status: 101, responseContentType: nil))

        #expect(!actions.canReplay)
        #expect(!actions.canCopyCurl)
        #expect(actions.canBreakpoint)
        #expect(actions.canSave)
        #expect(actions.canDelete)
        #expect(actions.statusClass == .informational)
        #expect(actions.domainActionTitle == "Pin Domain")
    }

    @Test("an event stream is not offered as a replay or a cURL command")
    func eventStream() {
        let plain = menu(row(responseContentType: "text/event-stream"))
        let charset = menu(row(responseContentType: "text/event-stream; charset=utf-8"))

        #expect(!plain.canReplay)
        #expect(!plain.canCopyCurl)
        #expect(plain.canBreakpoint)
        #expect(!charset.canReplay)
        #expect(!charset.canCopyCurl)
        #expect(plain.canSave)
        #expect(plain.statusClass == .success)
    }

    @Test("a pinned host offers unpin, and a row with no destination offers neither")
    func pinTitles() {
        #expect(menu(row(), pinned: true).domainActionTitle == "Unpin Domain")
        #expect(menu(row(), pinned: false).domainActionTitle == "Pin Domain")

        let nowhere = menu(row(destination: nil, responseContentType: nil))
        #expect(nowhere.domainActionTitle == nil)
        #expect(nowhere.host == nil)
        #expect(nowhere.canFilterMethod)
    }

    // MARK: - Replay, copy, save, delete

    @Test("replay opens the existing sheet on this row and leaves the selection")
    func replay() async throws {
        let session = FakeSession()
        let command = "curl 'https://api.example.com/products'"
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [row(id: "a"), row(id: "b")], nextCursor: nil, total: 2))
        ])
        await session.answerExchange("b", with: .success(Fixtures.exchange(id: "b")))
        await session.answerCurl("a", with: .success(CurlGenerated(curl: command, truncated: false)))
        await session.answerParses(with: [
            .success(CurlParseResult(request: ReplayRequest(method: "GET", url: "https://api.example.com/products")))
        ])
        let model = await started(session)
        await model.select("b")

        await menu(row(id: "a")).replay(using: model)

        #expect(model.selectedID == "b")
        #expect(model.importer?.stage == .editing)
        #expect(model.importer?.url == "https://api.example.com/products")
        #expect(await session.parsedCommands == [command])
        #expect(await session.curlRequests == ["a"])
    }

    @Test("a handshake is not sent to the replay sheet")
    func replayRefused() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [], nextCursor: nil, total: 0))
        ])
        let model = await started(session)

        await menu(row(status: 101)).replay(using: model)
        await menu(row(responseContentType: "text/event-stream")).copyCurl(using: model)

        #expect(model.importer == nil)
        #expect(await session.curlRequests.isEmpty)
    }

    @Test("copy as cURL writes the core's command")
    func copy() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        let command = "curl 'https://api.example.com/products' \\\n  -H 'Accept: application/json'"
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [], nextCursor: nil, total: 0))
        ])
        await session.answerCurl("a", with: .success(CurlGenerated(curl: command, truncated: false)))
        let model = await started(session, clipboard: clipboard)

        await menu(row()).copyCurl(using: model)

        #expect(clipboard.copies == [command])
        #expect(await session.curlRequests == ["a"])
        #expect(model.lastAction == "Copied as cURL.")
    }

    @Test("save request opens the existing prompt for this exchange")
    func save() {
        let model = TrafficInspectorModel(
            session: FakeSession(),
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        let saved = SavedRequestsModel(session: FakeSavedRequestsSession(), traffic: FakeSession())

        menu(row(method: "POST", url: "https://api.example.com/v1/orders?x=1")).save(
            using: model,
            saved: saved
        )

        #expect(saved.saving?.exchangeID == "a")
        #expect(saved.saving?.name == "POST /v1/orders")
    }

    @Test("delete removes this exchange and leaves another selection")
    func deleteOther() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(
                exchanges: [row(id: "a"), row(id: "b")],
                nextCursor: nil,
                total: 2
            ))
        ])
        await session.answerExchange("b", with: .success(Fixtures.exchange(id: "b")))
        let model = await started(session)
        await model.select("b")

        await menu(row(id: "a")).delete(using: model)

        #expect(await session.deletedIDs == ["a"])
        #expect(model.exchanges.map(\.id) == ["b"])
        #expect(model.selectedID == "b")
        #expect(model.totalExchanges == 1)
    }

    // MARK: - Pin and filters

    @Test("pin and unpin use the pinned-domain store")
    func pin() {
        let pinned = PinnedDomainsStore(storage: FakePinnedDomainsStorage())
        let actions = menu(row())

        actions.pin(using: pinned)
        #expect(pinned.isPinned("api.example.com"))
        #expect(menu(row(), pinned: true).domainActionTitle == "Unpin Domain")

        menu(row(), pinned: true).pin(using: pinned)
        #expect(!pinned.isPinned("api.example.com"))
    }

    @Test("host, method, and status compose with the filters already set")
    func filtersCompose() async throws {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [], nextCursor: nil, total: 0))
        ])
        let model = await started(session)
        model.setSearch("user")
        await model.applySearchNow()
        await model.setContentType("json")
        await model.setHTTPVersion(.http2)
        await model.setStreamKind("sse")
        await model.setFailed(true)
        await model.setSort(.oldestFirst)
        await model.setHost("cdn.example.com")
        await model.setStatusCode(404)

        let actions = menu(row(method: "POST", status: 201))
        await actions.filterMethod(using: model)
        #expect(model.filters.host == "cdn.example.com")

        await actions.filterStatus(using: model)
        await actions.filterHost(using: model)

        let last = try #require(await session.listRequests.last)
        #expect(last.host == "api.example.com")
        #expect(last.method == "POST")
        #expect(last.status == .class(2))
        #expect(model.filters.statusCode == nil)
        #expect(last.search == "user")
        #expect(last.contentType == "json")
        #expect(last.httpVersion == .http2)
        #expect(last.sse)
        #expect(!last.websocket)
        #expect(last.failed)
        #expect(last.order == .oldest)
    }
}

@Suite("Rule chooser lines")
struct RuleChooserLineTests {
    @Test("the match reads as a request, and the target is the file or the host")
    func mapLocalAndRemote() {
        let local = RuleMatcher(
            host: .exact("api.example.com"),
            path: .exact("/products"),
            method: .exact("get")
        )
        #expect(RuleDisplay.chooserMatch(local) == "GET api.example.com/products")
        #expect(
            RuleDisplay.chooserTarget(.mapLocal(MapLocalAction(path: "/Users/dev/mocks/products.json")))
                == "→ products.json"
        )

        let remote = RuleMatcher(host: .exact("api.example.com"), path: .prefix("/v1/"))
        #expect(RuleDisplay.chooserMatch(remote) == "api.example.com/v1/*")
        #expect(
            RuleDisplay.chooserTarget(
                .mapRemote(RemoteTarget(scheme: .https, host: "staging.example.com"))
            ) == "→ staging.example.com"
        )
    }

    @Test("a modify response names the status it sets, and never a header value")
    func modifyAndRedirect() {
        #expect(
            RuleDisplay.chooserTarget(.modifyResponse(ModifyResponseAction(status: 200)))
                == "Status 200"
        )
        let secret = RuleDisplay.chooserTarget(
            .modifyRequest(
                ModifyRequestAction(headers: [
                    HeaderEdit(.set, name: "Authorization", value: "Bearer secret")
                ])
            )
        )
        #expect(secret == "Set Authorization")
        #expect(secret?.contains("secret") != true)

        #expect(
            RuleDisplay.chooserTarget(
                .redirect(
                    RedirectAction(
                        target: RemoteTarget(scheme: .https, host: "staging.example.com", path: "/v2")
                    )
                )
            ) == "302 → staging.example.com/v2"
        )
        #expect(RuleDisplay.chooserTarget(.allow) == nil)
        #expect(RuleDisplay.chooserTarget(.block(BlockAction())) == "Answered 403")
        #expect(RuleDisplay.chooserMatch(.any) == "Every request")
    }
}
