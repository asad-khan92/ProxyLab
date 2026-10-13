//  A rule started from a captured exchange, and the hosts the sidebar lists.
//
//  The draft is what the editor is filled with. The catalog is what stays
//  listed when the traffic list is narrowed to one host. Neither is a second
//  rule model or a second traffic query.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Rule draft from traffic")
struct RuleDraftSeedTests {
    private func exchange(
        url: String = "https://api.example.com/v1/items?q=1",
        method: String = "POST",
        destination: Destination? = Destination(host: "staging.example.com", port: 8443),
        requestHeaders: Headers = Headers([
            Header(name: "Host", value: .text("api.example.com")),
            Header(name: "Authorization", value: .text("Bearer secret")),
            Header(name: "Cookie", value: .text("session=1")),
            Header(name: "Accept", value: .text("application/json")),
            Header(name: "Content-Type", value: .text("application/json")),
        ]),
        requestBody: BodyCapture = .complete(Data(#"{"a":1}"#.utf8)),
        status: UInt16? = 201,
        responseHeaders: Headers = Headers([
            Header(name: "Content-Type", value: .text("application/json; charset=utf-8")),
            Header(name: "Set-Cookie", value: .text("session=2")),
        ]),
        responseBody: BodyCapture = .complete(Data(#"{"ok":true}"#.utf8))
    ) -> TrafficExchange {
        TrafficExchange(
            id: "exchange-1",
            connection: ConnectionInfo(client: "127.0.0.1:1", scheme: .https, destination: destination),
            request: CapturedRequest(
                method: method,
                url: url,
                version: .http11,
                headers: requestHeaders,
                body: requestBody
            ),
            response: status.map {
                CapturedResponse(
                    status: $0,
                    version: .http11,
                    headers: responseHeaders,
                    body: responseBody
                )
            },
            timing: Timing(startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0))
        )
    }

    @Test("map local matches the request and leaves the file for the person")
    func mapLocal() {
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .mapLocal)

        #expect(seed.host == "api.example.com")
        #expect(seed.path == "/v1/items")
        #expect(seed.method == "POST")
        #expect(seed.name.contains("Map Local"))
        #expect(seed.localStatus == "201")
        #expect(seed.localContentType == "application/json")
        #expect(seed.destination == nil)
        #expect(seed.modifyRequest.headers.isEmpty)
    }

    @Test("map remote starts from the destination the proxy connected to")
    func mapRemote() {
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .mapRemote)

        #expect(seed.host == "api.example.com")
        #expect(seed.path == "/v1/items")
        #expect(seed.destination == "https://staging.example.com:8443/v1/items?q=1")
    }

    @Test("redirect prefills the source match and a starting location")
    func redirect() {
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .redirect)

        #expect(seed.host == "api.example.com")
        #expect(seed.path == "/v1/items")
        #expect(seed.method == "POST")
        #expect(seed.redirectURL == "https://staging.example.com:8443/v1/items?q=1")
    }

    @Test("modify request keeps the match, the body, and the headers that are not secrets")
    func modifyRequest() {
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .modifyRequest)

        #expect(seed.host == "api.example.com")
        #expect(seed.path == "/v1/items")
        #expect(seed.method == "POST")
        #expect(seed.modifyRequest.query.map(\.name) == ["q"])
        #expect(seed.modifyRequest.query.map(\.value) == ["1"])
        #expect(seed.modifyRequest.headers.map(\.name) == ["Content-Type", "Accept"])
        #expect(seed.modifyRequest.body.mode == .replace)
        #expect(seed.modifyRequest.body.text == #"{"a":1}"#)
        #expect(seed.modifyResponse.headers.isEmpty)
    }

    @Test("modify response prefills the status, the body, and not a cookie")
    func modifyResponse() {
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .modifyResponse)

        #expect(seed.modifyResponse.status == "201")
        #expect(seed.modifyResponse.headers.map(\.name) == ["Content-Type"])
        #expect(seed.modifyResponse.headers.map(\.value) == ["application/json; charset=utf-8"])
        #expect(seed.modifyResponse.body.text == #"{"ok":true}"#)
        #expect(seed.modifyRequest.headers.isEmpty)
        #expect(seed.modifyRequest.body.mode == .unchanged)
    }

    @Test("block and allow match the request and do not copy its answer")
    func blockAndAllow() {
        let blocked = RuleDraftSeed.from(exchange: exchange(), kind: .block)
        let allowed = RuleDraftSeed.from(exchange: exchange(), kind: .allow)

        #expect(blocked.host == "api.example.com")
        #expect(blocked.path == "/v1/items")
        #expect(blocked.method == "POST")
        #expect(blocked.localStatus == nil)
        #expect(allowed.modifyRequest.body.mode == .unchanged)
        #expect(allowed.destination == nil)
    }

    @Test("a summary can fill the match before the exchange is fetched")
    func summaryMatch() {
        let summary = ExchangeSummary(
            id: "row",
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: "GET",
            url: "https://api.example.com/health",
            version: .http11,
            status: 200,
            responseContentType: "application/json",
            completed: true
        )
        let seed = RuleDraftSeed.from(summary: summary, kind: .modifyRequest)

        #expect(seed.host == "api.example.com")
        #expect(seed.path == "/health")
        #expect(seed.method == "GET")
        #expect(seed.modifyRequest.headers.isEmpty)
        #expect(seed.modifyRequest.body.mode == .unchanged)
        #expect(seed.matchLine == "GET api.example.com /health")
    }

    @Test("a binary or truncated body is not written into the rule")
    func skipsBodiesThatAreNotText() {
        let binary = RuleDraftSeed.from(
            exchange: exchange(
                requestHeaders: Headers([
                    Header(name: "Content-Type", value: .text("image/png"))
                ]),
                requestBody: .complete(Data([0x89, 0x50, 0x4E, 0x47]))
            ),
            kind: .modifyRequest
        )
        #expect(binary.modifyRequest.body.mode == .unchanged)

        let truncated = RuleDraftSeed.from(
            exchange: exchange(
                requestBody: BodyCapture(data: Data(#"{"a":"#.utf8), size: 100, truncated: true)
            ),
            kind: .modifyRequest
        )
        #expect(truncated.modifyRequest.body.mode == .unchanged)
    }

    @Test("the editor opens with the draft, and a save sends that match")
    func editorSendsTheMatch() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new", name: "from traffic")))
        let seed = RuleDraftSeed.from(exchange: exchange(), kind: .modifyRequest)
        let editor = RuleEditorModel(
            creating: .modifyRequest,
            seed: seed,
            session: session,
            chooser: FakeFileChooser()
        )

        #expect(editor.host == "api.example.com")
        #expect(editor.path == "/v1/items")
        #expect(editor.method == "POST")
        #expect(editor.localPath.isEmpty)

        let saved = await editor.save()
        #expect(saved?.id == "new")

        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(new.matcher == RuleMatcher(
            host: .exact("api.example.com"),
            path: .exact("/v1/items"),
            method: .exact("POST")
        ))
        guard case let .modifyRequest(action) = new.action else {
            Issue.record("expected a modify request")
            return
        }
        #expect(action.headers.map(\.name) == ["Content-Type", "Accept"])
        #expect(action.body == .replace(#"{"a":1}"#))
    }
}

@Suite("Captured domains")
struct TrafficDomainCatalogTests {
    @Test("a host is listed once, in the spelling it was first seen")
    func once() {
        var catalog = TrafficDomainCatalog()
        catalog.observe("API.example.com")
        catalog.observe("api.example.com")
        catalog.observe("  ")
        catalog.observe(nil)
        catalog.observe("cdn.example.com")

        #expect(catalog.hosts == ["API.example.com", "cdn.example.com"])
        #expect(catalog.contains("api.example.com"))
    }

    @Test("a query narrows the sidebar list and an empty query does not")
    func query() {
        var catalog = TrafficDomainCatalog()
        catalog.observe("api.example.com")
        catalog.observe("cdn.example.com")
        catalog.observe("auth.example.com")

        #expect(catalog.matching("  ") == catalog.hosts)
        #expect(catalog.matching("API") == ["api.example.com"])
        #expect(catalog.matching("example") == catalog.hosts)
        #expect(catalog.matching("nope").isEmpty)
    }
}

@MainActor
@Suite("Domain browsing")
struct TrafficDomainBrowsingTests {
    private func row(_ id: String, host: String, path: String = "/v1") -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: host, port: 443),
            method: "GET",
            url: "https://\(host)\(path)",
            version: .http11,
            completed: true
        )
    }

    private func page(_ rows: [ExchangeSummary]) -> TrafficListResult {
        TrafficListResult(exchanges: rows, nextCursor: nil, total: UInt64(rows.count))
    }

    @Test("choosing a domain keeps the other domains listed")
    func filterKeepsDomains() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(page([row("a", host: "api.example.com"), row("b", host: "cdn.example.com")])),
            .success(page([row("a", host: "api.example.com")])),
        ])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await model.start()

        #expect(model.knownHosts == ["api.example.com", "cdn.example.com"])

        await model.setHost("api.example.com")

        #expect(model.filters.host == "api.example.com")
        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.knownHosts == ["api.example.com", "cdn.example.com"])

        await model.handle(.event(.trafficAdded(row("c", host: "other.example.com"))))

        #expect(model.exchanges.map(\.id) == ["a"])
        #expect(model.knownHosts == ["api.example.com", "cdn.example.com", "other.example.com"])
    }

    @Test("clearing traffic clears the domain list and keeps the selected host")
    func clearKeepsTheSelection() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(page([row("a", host: "api.example.com"), row("b", host: "cdn.example.com")]))
        ])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await model.start()
        await model.setHost("api.example.com")
        await model.clearAll()

        #expect(model.exchanges.isEmpty)
        #expect(model.knownHosts == ["api.example.com"])
        #expect(model.filters.host == "api.example.com")
    }

    @Test("all traffic is the host filter being unset")
    func allTraffic() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(page([row("a", host: "api.example.com")])),
            .success(page([row("a", host: "api.example.com")])),
        ])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await model.start()
        await model.setHost("api.example.com")
        await model.setHost(nil)

        #expect(model.filters.host == nil)
        #expect(PinnedDomainSelection(host: model.filters.host) == .allTraffic)
        #expect(model.knownHosts == ["api.example.com"])
    }
}

@MainActor
@Suite("Rules chosen from traffic")
struct TrafficRuleChooserTests {
    private func model(_ session: FakeRulesSession) -> RulesModel {
        RulesModel(session: session, chooser: FakeFileChooser())
    }

    @Test("the chooser lists only the kind that was asked for")
    func filtersByKind() async {
        let session = FakeRulesSession()
        await session.answerList(with: [
            RuleFixtures.rule(id: "block", action: .block(BlockAction())),
            RuleFixtures.rule(
                id: "local",
                name: "mock",
                action: .mapLocal(MapLocalAction(path: "/tmp/body.json"))
            ),
            RuleFixtures.rule(id: "allow", action: .allow),
        ])
        let model = model(session)
        await model.load()

        #expect(model.rules(of: .mapLocal).map(\.id) == ["local"])
        #expect(model.rules(of: .allow).map(\.id) == ["allow"])
    }

    @Test("apply turns a rule on and closes the chooser")
    func applyEnables() async {
        let rule = RuleFixtures.rule(id: "local", enabled: false, action: .mapLocal(MapLocalAction(path: "/tmp/a")))
        let session = FakeRulesSession()
        await session.answerList(with: [rule])
        await session.answerUpdate(with: .success(
            RuleFixtures.rule(id: "local", enabled: true, action: .mapLocal(MapLocalAction(path: "/tmp/a")))
        ))
        let model = model(session)
        await model.load()
        model.beginTrafficRule(
            TrafficRuleRequest(kind: .mapLocal, exchangeID: "e", seed: RuleDraftSeed(
                name: "Map Local",
                host: "api.example.com",
                path: "/v1",
                method: "GET",
                localStatus: nil,
                localContentType: nil,
                destination: nil,
                redirectURL: nil,
                modifyRequest: ModifyRequestDraft(),
                modifyResponse: ModifyResponseDraft()
            ))
        )

        let applied = await model.applyRule(rule)

        #expect(applied)
        #expect(model.trafficRule == nil)
        #expect(model.rules.first?.enabled == true)
        #expect(await session.updates().map(\.update) == [RuleUpdate(enabled: true)])
    }

    @Test("an enabled rule is left on, and the chooser closes without a write")
    func alreadyEnabledCloses() async {
        let rule = RuleFixtures.rule(
            id: "local",
            enabled: true,
            action: .mapLocal(MapLocalAction(path: "/tmp/a"))
        )
        let session = FakeRulesSession()
        await session.answerList(with: [rule])
        let model = model(session)
        await model.load()
        model.beginTrafficRule(
            TrafficRuleRequest(
                kind: .mapLocal,
                exchangeID: "e",
                seed: RuleDraftSeed(
                    name: "Map Local",
                    host: "api.example.com",
                    path: "/v1",
                    method: "GET",
                    localStatus: nil,
                    localContentType: nil,
                    destination: nil,
                    redirectURL: nil,
                    modifyRequest: ModifyRequestDraft(),
                    modifyResponse: ModifyResponseDraft()
                )
            )
        )

        let applied = await model.applyRule(rule)

        #expect(applied)
        #expect(model.trafficRule == nil)
        #expect(model.rules.first?.enabled == true)
        #expect(await session.updates().isEmpty)
    }

    @Test("a new rule from traffic opens the editor with the match filled in")
    func createsFromSeed() async throws {
        let model = model(FakeRulesSession())
        let seed = RuleDraftSeed(
            name: "Block api.example.com /v1",
            host: "api.example.com",
            path: "/v1",
            method: "GET",
            localStatus: nil,
            localContentType: nil,
            destination: nil,
            redirectURL: nil,
            modifyRequest: ModifyRequestDraft(),
            modifyResponse: ModifyResponseDraft()
        )

        model.beginCreating(.block, seed: seed)

        let editor = try #require(model.editor)
        #expect(editor.actionKind == .block)
        #expect(editor.host == "api.example.com")
        #expect(editor.path == "/v1")
        #expect(editor.method == "GET")
        #expect(editor.blockStatus == "403")
        #expect(editor.name == "Block api.example.com /v1")
    }
}
