//  The rules screen: filtering one collection, reading a row, and moving a
//  rule without giving the others a new order of their own.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Rules screen")
struct RulesScreenTests {
    private func model(_ session: FakeRulesSession) -> RulesModel {
        RulesModel(session: session, chooser: FakeFileChooser())
    }

    private func loaded(_ rules: [Rule]) async -> RulesModel {
        let session = FakeRulesSession()
        await session.answerList(with: rules)
        let model = model(session)
        await model.load()

        return model
    }

    // MARK: - Categories and search

    @Test("a category shows that kind, in the order the core consults them")
    func categoryFiltersWithoutReordering() async {
        let rules = sample()
        let model = await loaded(rules)

        #expect(model.visibleRules.map(\.id) == ["block", "local", "remote"])
        #expect(model.count(in: .all) == 3)
        #expect(model.count(in: .mapLocal) == 1)
        #expect(model.count(in: .block) == 1)
        #expect(model.count(in: .redirect) == 0)

        model.category = .mapLocal

        #expect(model.visibleRules.map(\.id) == ["local"])
        #expect(model.placeholder == .rules)

        model.category = .redirect

        #expect(model.visibleRules.isEmpty)
        #expect(model.placeholder == .emptyCategory("No Redirect rules"))
        // The collection is unchanged.
        #expect(model.rules.map(\.id) == ["block", "local", "remote"])
    }

    @Test("search narrows the selected category and ignores header values")
    func searchComposesWithCategory() async {
        var rules = sample()
        rules.append(
            RuleFixtures.rule(
                id: "profile",
                name: "Force profile success",
                priority: 40,
                matcher: RuleMatcher(
                    host: .exact("api.example.com"),
                    path: .exact("/profile"),
                    method: .exact("GET")
                ),
                action: .modifyResponse(
                    ModifyResponseAction(
                        status: 200,
                        headers: [HeaderEdit(.set, name: "X-Debug", value: "super-secret")]
                    )
                )
            )
        )
        let model = await loaded(rules)

        model.searchText = "products"
        #expect(model.visibleRules.map(\.id) == ["local"])

        model.category = .block
        #expect(model.visibleRules.isEmpty)
        #expect(model.placeholder == .noMatches)
        // The category count does not follow the search.
        #expect(model.count(in: .block) == 1)

        model.category = .all
        model.searchText = "POST"
        #expect(model.visibleRules.map(\.id) == ["block"])

        model.searchText = "staging.example.com"
        #expect(model.visibleRules.map(\.id) == ["remote"])

        model.searchText = "super-secret"
        #expect(model.visibleRules.isEmpty)

        model.searchText = "X-Debug"
        #expect(model.visibleRules.map(\.id) == ["profile"])

        model.searchText = "   "
        #expect(model.visibleRules.map(\.id) == ["block", "local", "remote", "profile"])
    }

    @Test("an empty core, an empty category, and a missed search each say so")
    func emptyCopy() {
        let none = RulesEmptyCopy.forPlaceholder(.noRules)
        #expect(none?.title == "No rules yet")
        #expect(none?.detail?.contains("right-click captured traffic") == true)

        #expect(RulesEmptyCopy.forPlaceholder(.noMatches)?.title == "No matching rules")
        #expect(RulesEmptyCopy.forPlaceholder(.noMatches)?.detail == nil)
        #expect(
            RulesEmptyCopy.forPlaceholder(.emptyCategory("No Map Local rules"))?.title
                == "No Map Local rules"
        )
        #expect(RulesEmptyCopy.forPlaceholder(.rules) == nil)
    }

    // MARK: - How a row and a detail read

    @Test("a row reads the type, the match, and the action, and not a header value")
    func rowPresentation() {
        let local = RuleRowPresentation(rule: sample()[1])
        #expect(local.enabled)
        #expect(local.name == "Products fixture")
        #expect(local.type == "Map Local")
        #expect(local.match == "GET api.example.com/v1/products")
        #expect(local.action == "→ products.json")
        #expect(local.priority == 10)

        let remote = RuleRowPresentation(rule: sample()[2])
        #expect(remote.type == "Map Remote")
        #expect(remote.match == "api.example.com/v1/*")
        #expect(remote.action == "→ staging.example.com")

        let blocked = RuleFixtures.rule(
            id: "off",
            name: "Disable old endpoint",
            enabled: false,
            matcher: RuleMatcher(
                host: .exact("api.example.com"),
                path: .exact("/v1/legacy"),
                method: .exact("POST")
            ),
            action: .block(BlockAction(status: 403))
        )
        let row = RuleRowPresentation(rule: blocked)
        #expect(!row.enabled)
        #expect(row.type == "Block")
        #expect(row.match == "POST api.example.com/v1/legacy")
        #expect(row.action == "Answered 403")

        let secret = RuleRowPresentation(
            rule: RuleFixtures.rule(
                id: "secret",
                action: .modifyResponse(
                    ModifyResponseAction(
                        status: 200,
                        headers: [HeaderEdit(.set, name: "X-Debug", value: "super-secret")]
                    )
                )
            )
        )
        #expect(secret.action?.contains("X-Debug") == true)
        #expect(secret.action?.contains("super-secret") == false)
    }

    @Test("the detail splits a rule into match and action")
    func detailContent() {
        let local = RuleDetailContent(rule: sample()[1])
        #expect(local.match.map(\.label) == ["Host", "Path", "Method"])
        #expect(local.match.map(\.value) == ["api.example.com", "/v1/products", "GET"])
        #expect(local.action.map(\.label) == ["Local file", "Status", "Content type"])
        #expect(local.action.map(\.value).contains("200"))
        #expect(local.action.map(\.value).contains { $0.contains("products.json") })
        #expect(local.changes.isEmpty)

        let remote = RuleDetailContent(rule: sample()[2])
        #expect(remote.action.map(\.label) == ["Destination"])
        #expect(remote.action.first?.value == "https://staging.example.com")

        let modified = RuleDetailContent(
            rule: RuleFixtures.rule(
                id: "profile",
                action: .modifyResponse(
                    ModifyResponseAction(
                        status: 200,
                        headers: [HeaderEdit(.set, name: "X-Debug", value: "super-secret")]
                    )
                )
            )
        )
        #expect(modified.action.isEmpty)
        #expect(modified.changes.contains("Status 200"))
        #expect(modified.changes.contains("Set X-Debug"))
        #expect(modified.changes.joined(separator: " ").contains("super-secret") == false)

        let open = RuleDetailContent(rule: RuleFixtures.rule(id: "any", matcher: .any, action: .allow))
        #expect(open.match.map(\.value) == ["Any", "Any", "Any"])
        #expect(open.action.first?.value == "Carried as usual")
    }

    // MARK: - Enabled

    @Test("turning a rule off sends enabled and leaves its priority")
    func enableLeavesPriority() async {
        let rule = RuleFixtures.rule(id: "rule-a", enabled: true, priority: 40)
        let session = FakeRulesSession()
        await session.answerList(with: [rule])
        await session.answerUpdate(
            with: .success(RuleFixtures.rule(id: "rule-a", enabled: false, priority: 40))
        )

        let model = model(session)
        await model.load()
        await model.setEnabled(false, for: rule)

        #expect(model.rules.first?.enabled == false)
        #expect(model.rules.first?.priority == 40)
        #expect(await session.updates().map(\.update) == [RuleUpdate(enabled: false)])
    }

    // MARK: - Duplicate

    @Test("duplicating opens a new draft and does not write it yet")
    func duplicateIsADraft() async {
        let source = RuleFixtures.rule(
            id: "rule-a",
            name: "Products fixture",
            enabled: false,
            priority: 30,
            matcher: RuleMatcher(
                host: .exact("api.example.com"),
                path: .prefix("/v1/"),
                method: .exact("GET")
            ),
            action: .block(BlockAction(status: 451))
        )
        let session = FakeRulesSession()
        let model = model(session)
        model.beginDuplicating(source)

        let editor = model.editor
        #expect(editor?.mode == .create)
        #expect(editor?.title == "Duplicate Rule")
        #expect(editor?.name == "Products fixture Copy")
        #expect(editor?.enabled == false)
        #expect(editor?.priority == "30")
        #expect(editor?.host == "api.example.com")
        #expect(editor?.path == "/v1/")
        #expect(editor?.pathIsPrefix == true)
        #expect(editor?.method == "GET")
        #expect(editor?.actionKind == .block)
        #expect(editor?.blockStatus == "451")
        #expect(editor?.fieldErrors.isEmpty == true)
        #expect(await session.calls.isEmpty)
    }

    @Test("saving a duplicate creates a rule and selects the id the core minted")
    func duplicateCreates() async {
        let source = RuleFixtures.rule(
            id: "rule-a",
            name: "Products fixture",
            priority: 30,
            matcher: RuleMatcher(host: .exact("api.example.com"), path: .exact("/v1/products")),
            action: .block(BlockAction(status: 403))
        )
        let created = RuleFixtures.rule(id: "rule-copy", name: "Products fixture Copy", priority: 30)
        let session = FakeRulesSession()
        await session.answerLists(with: [.success([source, created])])
        await session.answerCreate(with: .success(created))

        let model = model(session)
        model.beginDuplicating(source)

        #expect(await model.saveEditor())
        #expect(model.editor == nil)
        #expect(model.selectedID == "rule-copy")

        let creates = await session.calls.compactMap { call in
            if case let .create(rule) = call { return rule }

            return nil
        }
        #expect(creates.count == 1)
        #expect(creates[0].name == "Products fixture Copy")
        #expect(creates[0].priority == 30)
        #expect(creates[0].matcher == source.matcher)
        #expect(creates[0].action == source.action)
        #expect(await session.updates().isEmpty)
    }

    @Test("saving a rule the current category hides shows the list that contains it")
    func saveRevealsAHiddenRule() async {
        let block = RuleFixtures.rule(id: "block", name: "Disable old endpoint", priority: 0)
        let created = RuleFixtures.rule(
            id: "opened",
            name: "except health",
            priority: 10,
            matcher: RuleMatcher(path: .exact("/health")),
            action: .allow
        )
        let session = FakeRulesSession()
        await session.answerLists(with: [.success([block, created])])
        await session.answerCreate(with: .success(created))

        let model = model(session)
        await model.load()
        model.category = .block
        model.searchText = "nope"
        model.beginCreating(.allow)
        model.editor?.name = "except health"
        model.editor?.path = "/health"

        #expect(await model.saveEditor())
        #expect(model.category == .all)
        #expect(model.searchText.isEmpty)
        #expect(model.selectedID == "opened")
        #expect(model.visibleRules.contains { $0.id == "opened" })
    }

    @Test("a copied name still fits the length the core allows")
    func duplicateNameFits() {
        let copy = RuleEditorModel.duplicateName(for: String(repeating: "a", count: 200))

        #expect(copy.count <= maxRuleNameLength)
        #expect(copy.hasSuffix(" Copy"))
    }

    @Test("duplicating a modify rule keeps its edits, including a header value the list hides")
    func duplicateKeepsEdits() async {
        let source = RuleFixtures.rule(
            id: "rule-a",
            name: "Force profile success",
            action: .modifyResponse(
                ModifyResponseAction(
                    status: 200,
                    headers: [HeaderEdit(.set, name: "X-Debug", value: "super-secret")]
                )
            )
        )
        let model = model(FakeRulesSession())
        model.beginDuplicating(source)

        #expect(model.editor?.mode.isCreate == true)
        #expect(model.editor?.modifyResponse.status == "200")
        #expect(model.editor?.modifyResponse.headers.first?.name == "X-Debug")
        #expect(model.editor?.modifyResponse.headers.first?.value == "super-secret")
    }

    // MARK: - Order

    @Test("moving a filtered rule keeps the relative order of the rules it passes")
    func filteredMovePreservesTheOthers() async {
        let block = RuleFixtures.rule(id: "block", name: "Block", priority: 0, action: .block(BlockAction()))
        let localB = RuleFixtures.rule(
            id: "b",
            name: "B",
            priority: 10,
            action: .mapLocal(MapLocalAction(path: "/tmp/b.json"))
        )
        let modify = RuleFixtures.rule(
            id: "mod",
            name: "Modify",
            priority: 20,
            action: .modifyRequest(ModifyRequestAction(method: "POST"))
        )
        let localD = RuleFixtures.rule(
            id: "d",
            name: "D",
            priority: 30,
            action: .mapLocal(MapLocalAction(path: "/tmp/d.json"))
        )
        let original = [block, localB, modify, localD]
        let session = FakeRulesSession()
        await session.answerLists(with: [.success(original), .success(original)])
        await session.answerUpdate(with: .success(localD))

        let model = model(session)
        await model.load()
        model.category = .mapLocal
        await model.moveUp(localD)

        // D moves to B's place. Block, B, and Modify stay in that order:
        // block, D, B, modify. Priorities are the usual steps of ten.
        let updates = await session.updates()
        #expect(updates.contains { $0.id == "d" && $0.update == RuleUpdate(priority: 10) })
        #expect(updates.contains { $0.id == "b" && $0.update == RuleUpdate(priority: 20) })
        #expect(updates.contains { $0.id == "mod" && $0.update == RuleUpdate(priority: 30) })
        #expect(updates.contains { $0.id == "block" } == false)
        #expect(updates.allSatisfy { $0.update.enabled == nil && $0.update.action == nil })
    }

    @Test("a drag to the front writes the same priority steps as move up")
    func dragToFront() async {
        let session = FakeRulesSession()
        await session.answerLists(
            with: [.success(RuleFixtures.ordered()), .success(RuleFixtures.ordered())]
        )
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-c")))

        let model = model(session)
        await model.load()
        await model.moveVisible(from: IndexSet(integer: 2), to: 0)

        let updates = await session.updates()
        #expect(updates.contains { $0.id == "rule-c" && $0.update == RuleUpdate(priority: 0) })
        #expect(updates.contains { $0.id == "rule-a" && $0.update == RuleUpdate(priority: 10) })
        #expect(updates.contains { $0.id == "rule-b" && $0.update == RuleUpdate(priority: 20) })
    }

    @Test("dropping a rule where it already sits writes nothing")
    func moveNoOp() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()
        await model.moveVisible(from: IndexSet(integer: 0), to: 1)
        await model.moveVisible(from: IndexSet([0, 1]), to: 3)

        #expect(await session.updates().isEmpty)
        #expect(await session.listCount() == 1)
    }

    @Test("the consultation order after a filtered move is the order RuleOrder describes")
    func ruleOrderInsertsBeforeTheVisibleNeighbour() {
        let rules = [
            RuleFixtures.rule(id: "block", priority: 0),
            RuleFixtures.rule(id: "b", priority: 10),
            RuleFixtures.rule(id: "mod", priority: 20),
            RuleFixtures.rule(id: "d", priority: 30),
        ]
        let visible = [rules[1], rules[3]]
        let moved = RuleOrder.moving(rules, visible: visible, from: 1, to: 0)

        #expect(moved?.map(\.id) == ["block", "d", "b", "mod"])
        #expect(RuleOrder.moving(rules, visible: rules, from: 0, to: 1) == nil)
    }

    // MARK: - Delete still waits

    @Test("delete still waits for confirmation before anything is removed")
    func deleteStillConfirms() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()
        model.selectedID = "rule-a"
        model.beginDeleting(model.rules[0])

        #expect(model.deleting?.id == "rule-a")
        #expect(model.selected?.id == "rule-a")
        #expect(model.rules.count == 3)
        #expect(await session.calls.count { if case .delete = $0 { true } else { false } } == 0)
    }

    // MARK: - Width

    @Test("the screen is three columns at the window's minimum and stacks below that")
    func layoutForWidth() {
        #expect(RulesScreenLayout.forWidth(900) == .columns)
        #expect(RulesScreenLayout.forWidth(RulesScreenLayout.columnsWidth) == .columns)
        #expect(RulesScreenLayout.forWidth(RulesScreenLayout.columnsWidth - 1) == .listAndDetail)
        #expect(RulesScreenLayout.forWidth(RulesScreenLayout.listAndDetailWidth) == .listAndDetail)
        #expect(RulesScreenLayout.forWidth(RulesScreenLayout.listAndDetailWidth - 1) == .stacked)
        #expect(RulesScreenLayout.forWidth(420) == .stacked)
    }

    // MARK: - Fixtures

    /// Block, Map Local, Map Remote, in consultation order.
    private func sample() -> [Rule] {
        [
            RuleFixtures.rule(
                id: "block",
                name: "Disable old endpoint",
                priority: 0,
                matcher: RuleMatcher(
                    host: .exact("api.example.com"),
                    path: .exact("/v1/legacy"),
                    method: .exact("POST")
                ),
                action: .block(BlockAction(status: 403))
            ),
            RuleFixtures.rule(
                id: "local",
                name: "Products fixture",
                priority: 10,
                matcher: RuleMatcher(
                    host: .exact("api.example.com"),
                    path: .exact("/v1/products"),
                    method: .exact("GET")
                ),
                action: .mapLocal(MapLocalAction(path: "/Users/dev/mocks/products.json", status: 200))
            ),
            RuleFixtures.rule(
                id: "remote",
                name: "Staging API",
                priority: 20,
                matcher: RuleMatcher(host: .exact("api.example.com"), path: .prefix("/v1/")),
                action: .mapRemote(RemoteTarget(scheme: .https, host: "staging.example.com"))
            ),
        ]
    }
}
