//  The rules screen: what it shows, and what it refuses to change on its own.
//
//  The behaviour worth being sure of is that nothing on screen changes before
//  the core says it changed — a rule that has vanished from a list but not from
//  the proxy is worse than a list that takes a moment — and that the order shown
//  is always the order the core will consult.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Rules list")
struct RulesModelTests {
    private func model(_ session: FakeRulesSession) -> RulesModel {
        RulesModel(session: session, chooser: FakeFileChooser())
    }

    // MARK: - Loading

    @Test("the list starts out loading, before anything has been asked")
    func startsLoading() {
        let model = model(FakeRulesSession())

        #expect(model.state == .loading)
        #expect(model.placeholder == .loading)
        #expect(model.rules.isEmpty)
    }

    @Test("loading shows the rules in the order the core sent them")
    func loads() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()

        #expect(model.state == .loaded)
        #expect(model.rules.map(\.id) == ["rule-a", "rule-b", "rule-c"])
        #expect(model.placeholder == .rules)
        #expect(model.lastError == nil)
    }

    @Test("a core with no rules says so, rather than looking like a failure")
    func emptyState() async {
        let session = FakeRulesSession()
        await session.answerList(with: [])

        let model = model(session)
        await model.load()

        #expect(model.state == .loaded)
        #expect(model.placeholder == .noRules)
    }

    @Test("a list that could not be fetched says why, and offers to try again")
    func loadFails() async {
        let session = FakeRulesSession()
        await session.answerLists(with: [.failure(.cannotConnect(path: "/tmp/x.sock"))])

        let model = model(session)
        await model.load()

        guard case let .failed(reason) = model.placeholder else {
            Issue.record("expected a failure")
            return
        }

        #expect(reason.contains("/tmp/x.sock"))
        #expect(model.lastError == reason)
    }

    @Test("the rules are counted, and so are the ones that are switched on")
    func counts() async {
        let session = FakeRulesSession()
        await session.answerList(
            with: [
                RuleFixtures.rule(id: "a", enabled: true),
                RuleFixtures.rule(id: "b", enabled: false),
                RuleFixtures.rule(id: "c", enabled: true),
            ]
        )

        let model = model(session)
        await model.load()

        #expect(model.rules.count == 3)
        #expect(model.enabledCount == 2)
    }

    @Test("a selection the core no longer has is dropped when the list reloads")
    func staleSelectionDropped() async {
        let session = FakeRulesSession()
        await session.answerLists(
            with: [.success(RuleFixtures.ordered()), .success([RuleFixtures.rule(id: "rule-a")])]
        )

        let model = model(session)
        await model.load()
        model.selectedID = "rule-c"

        await model.load()

        #expect(model.selectedID == nil)
        #expect(model.selected == nil)
    }

    // MARK: - Turning one on and off

    @Test("turning a rule off sends one field and shows what the core confirmed")
    func disable() async {
        let rule = RuleFixtures.rule(id: "rule-a", enabled: true)
        let session = FakeRulesSession()
        await session.answerList(with: [rule])
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-a", enabled: false)))

        let model = model(session)
        await model.load()
        await model.setEnabled(false, for: rule)

        #expect(model.rules.first?.enabled == false)
        #expect(await session.updates() .map(\.update) == [RuleUpdate(enabled: false)])
        #expect(model.lastError == nil)
    }

    @Test("a switch the core refused leaves the rule as the core still holds it")
    func disableFails() async {
        let rule = RuleFixtures.rule(id: "rule-a", enabled: true)
        let session = FakeRulesSession()
        await session.answerList(with: [rule])
        await session.answerUpdate(with: .failure(.connectionClosed))

        let model = model(session)
        await model.load()
        await model.setEnabled(false, for: rule)

        // Still on: the proxy is still applying it, so the list still says so.
        #expect(model.rules.first?.enabled == true)
        #expect(model.lastError?.contains("closed") == true)
    }

    @Test("switching a rule that something else removed reloads rather than insisting")
    func disableVanishedRule() async {
        let rule = RuleFixtures.rule(id: "rule-a")
        let session = FakeRulesSession()
        await session.answerLists(with: [.success([rule]), .success([])])
        await session.answerUpdate(with: .failure(RuleFixtures.notFound))

        let model = model(session)
        await model.load()
        await model.setEnabled(false, for: rule)

        #expect(model.rules.isEmpty)
        #expect(model.lastError == "That rule no longer exists.")
        #expect(await session.listCount() == 2)
    }

    // MARK: - The order

    @Test("a rule at the top cannot be moved up, and one at the bottom cannot move down")
    func moveBounds() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()

        #expect(!model.canMoveUp(model.rules[0]))
        #expect(model.canMoveDown(model.rules[0]))
        #expect(model.canMoveUp(model.rules[2]))
        #expect(!model.canMoveDown(model.rules[2]))
    }

    @Test("moving a rule up writes the priorities that produce the new order")
    func moveUpWritesPriorities() async {
        let session = FakeRulesSession()
        let moved = [
            RuleFixtures.rule(id: "rule-b", priority: 0),
            RuleFixtures.rule(id: "rule-a", priority: 10),
            RuleFixtures.rule(id: "rule-c", priority: 20),
        ]
        await session.answerLists(with: [.success(RuleFixtures.ordered()), .success(moved)])
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-a")))

        let model = model(session)
        await model.load()
        await model.moveUp(model.rules[1])

        // rule-b goes to 0 and rule-a to 10; rule-c is already at 20 and is
        // not sent at all.
        let updates = await session.updates()
        #expect(updates.count == 2)
        #expect(updates.contains { $0.id == "rule-b" && $0.update == RuleUpdate(priority: 0) })
        #expect(updates.contains { $0.id == "rule-a" && $0.update == RuleUpdate(priority: 10) })
    }

    @Test("the order shown after a move is the one the core gave back")
    func moveReloadsFromTheCore() async {
        let session = FakeRulesSession()
        let moved = [
            RuleFixtures.rule(id: "rule-b", priority: 0),
            RuleFixtures.rule(id: "rule-a", priority: 10),
            RuleFixtures.rule(id: "rule-c", priority: 20),
        ]
        await session.answerLists(with: [.success(RuleFixtures.ordered()), .success(moved)])
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-a")))

        let model = model(session)
        await model.load()
        await model.moveUp(model.rules[1])

        #expect(model.rules.map(\.id) == ["rule-b", "rule-a", "rule-c"])
        #expect(model.rules.map(\.priority) == [0, 10, 20])
        #expect(await session.listCount() == 2)
    }

    @Test("moving a rule down is the same move in the other direction")
    func moveDown() async {
        let session = FakeRulesSession()
        await session.answerLists(
            with: [.success(RuleFixtures.ordered()), .success(RuleFixtures.ordered())]
        )
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-a")))

        let model = model(session)
        await model.load()
        await model.moveDown(model.rules[0])

        let updates = await session.updates()
        #expect(updates.contains { $0.id == "rule-a" && $0.update == RuleUpdate(priority: 10) })
        #expect(updates.contains { $0.id == "rule-b" && $0.update == RuleUpdate(priority: 0) })
    }

    @Test("rules sitting at the same priority are spread apart so an order exists")
    func equalPrioritiesAreRenumbered() async {
        let session = FakeRulesSession()
        let flat = [
            RuleFixtures.rule(id: "rule-a", priority: 5),
            RuleFixtures.rule(id: "rule-b", priority: 5),
        ]
        await session.answerLists(with: [.success(flat), .success(flat)])
        await session.answerUpdate(with: .success(RuleFixtures.rule(id: "rule-a")))

        let model = model(session)
        await model.load()
        await model.moveUp(model.rules[1])

        // Swapping two equal numbers would change nothing, and the core breaks
        // the tie by id — which is not something a person can move a rule past.
        let updates = await session.updates()
        #expect(updates.contains { $0.id == "rule-b" && $0.update == RuleUpdate(priority: 0) })
        #expect(updates.contains { $0.id == "rule-a" && $0.update == RuleUpdate(priority: 10) })
    }

    @Test("a move the core refused still ends with the order the core has")
    func moveFails() async {
        let session = FakeRulesSession()
        await session.answerLists(
            with: [.success(RuleFixtures.ordered()), .success(RuleFixtures.ordered())]
        )
        await session.answerUpdate(with: .failure(.connectionClosed))

        let model = model(session)
        await model.load()
        await model.moveUp(model.rules[1])

        #expect(model.lastError?.contains("closed") == true)
        #expect(model.rules.map(\.id) == ["rule-a", "rule-b", "rule-c"])
        #expect(await session.listCount() == 2)
    }

    // MARK: - Deleting

    @Test("deleting is asked for, and nothing goes until it is confirmed")
    func deleteNeedsConfirming() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()
        model.beginDeleting(model.rules[0])

        #expect(model.deleting?.id == "rule-a")
        #expect(model.rules.count == 3)
        #expect(await session.calls.count { if case .delete = $0 { return true } else { return false } } == 0)
    }

    @Test("a confirmed delete removes the rule once the core says it is gone")
    func deleteConfirmed() async {
        let session = FakeRulesSession()
        await session.answerLists(
            with: [
                .success(RuleFixtures.ordered()),
                .success(Array(RuleFixtures.ordered().dropFirst())),
            ]
        )
        await session.answerDelete(with: .success(RuleDeletedResult(ruleID: "rule-a", deleted: true)))

        let model = model(session)
        await model.load()
        model.selectedID = "rule-a"
        model.beginDeleting(model.rules[0])

        #expect(await model.confirmDelete())
        #expect(model.rules.map(\.id) == ["rule-b", "rule-c"])
        #expect(model.selectedID == nil)
        #expect(model.deleting == nil)
        #expect(model.lastError == nil)
    }

    @Test("a delete that failed keeps the rule and says what went wrong")
    func deleteFails() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())
        await session.answerDelete(with: .failure(.connectionClosed))

        let model = model(session)
        await model.load()
        model.beginDeleting(model.rules[0])

        #expect(await model.confirmDelete() == false)
        #expect(model.rules.map(\.id) == ["rule-a", "rule-b", "rule-c"])
        #expect(model.lastError?.contains("closed") == true)
    }

    @Test("a rule something else already removed is not reported as a failure")
    func deleteAlreadyGone() async {
        let session = FakeRulesSession()
        await session.answerLists(with: [.success(RuleFixtures.ordered()), .success([])])
        await session.answerDelete(
            with: .success(RuleDeletedResult(ruleID: "rule-a", deleted: false))
        )

        let model = model(session)
        await model.load()
        model.beginDeleting(model.rules[0])

        #expect(await model.confirmDelete())
        #expect(model.lastError == "That rule no longer existed.")
        #expect(model.rules.isEmpty)
    }

    @Test("cancelling a delete leaves the core alone")
    func deleteCancelled() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()
        model.beginDeleting(model.rules[0])
        model.deleting = nil

        #expect(await model.confirmDelete() == false)
        #expect(model.rules.count == 3)
    }

    // MARK: - The editor

    @Test("a new rule opens the editor on the action that was chosen")
    func beginCreating() async {
        let model = model(FakeRulesSession())
        model.beginCreating(.mapRemote)

        #expect(model.editor?.mode == .create)
        #expect(model.editor?.actionKind == .mapRemote)
    }

    @Test("editing a rule opens the editor on that rule")
    func beginEditing() async {
        let session = FakeRulesSession()
        await session.answerList(with: RuleFixtures.ordered())

        let model = model(session)
        await model.load()
        model.beginEditing(model.rules[1])

        #expect(model.editor?.mode == .edit(ruleID: "rule-b"))
        #expect(model.editor?.name == "second")
    }

    @Test("cancelling closes the editor and changes nothing on the core")
    func cancelEditing() async {
        let session = FakeRulesSession()
        let model = model(session)
        model.beginCreating(.block)
        model.editor?.name = "half typed"
        model.cancelEditing()

        #expect(model.editor == nil)
        #expect(await session.calls.isEmpty)
    }

    @Test("a saved rule closes the editor, is selected, and the list is asked for again")
    func saveClosesTheEditor() async {
        let created = RuleFixtures.rule(id: "rule-new", name: "new rule")
        let session = FakeRulesSession()
        await session.answerLists(with: [.success([created])])
        await session.answerCreate(with: .success(created))

        let model = model(session)
        model.beginCreating(.block)
        model.editor?.name = "new rule"

        #expect(await model.saveEditor())
        #expect(model.editor == nil)
        #expect(model.selectedID == "rule-new")
        #expect(model.rules.map(\.id) == ["rule-new"])
    }

    @Test("a save that failed keeps the editor open with everything still in it")
    func saveKeepsTheEditorOpen() async {
        let session = FakeRulesSession()
        await session.answerCreate(
            with: .failure(RuleFixtures.refusal("a rule name must not contain control characters"))
        )

        let model = model(session)
        model.beginCreating(.block)
        model.editor?.name = "a name"
        model.editor?.host = "api.example.com"

        #expect(await model.saveEditor() == false)
        #expect(model.editor != nil)
        #expect(model.editor?.name == "a name")
        #expect(model.editor?.host == "api.example.com")
        #expect(model.editor?.fieldErrors[.name] != nil)
        #expect(await session.listCount() == 0)
    }

    @Test("saving with no editor open does nothing")
    func saveWithNoEditor() async {
        let model = model(FakeRulesSession())

        #expect(await model.saveEditor() == false)
    }
}

@MainActor
@Suite("Rule display")
struct RuleDisplayTests {
    @Test("a matcher reads as the criteria it sets, in the order they are compared")
    func matcherSummary() {
        let matcher = RuleMatcher(
            host: .exact("api.example.com"),
            path: .exact("/users"),
            method: .exact("get")
        )

        #expect(RuleDisplay.matcherSummary(matcher) == "api.example.com · /users · GET")
    }

    @Test("a prefix path is marked as one, so it does not read as an exact path")
    func prefixMarked() {
        let matcher = RuleMatcher(path: .prefix("/assets"))

        #expect(RuleDisplay.matcherSummary(matcher) == "/assets*")
    }

    @Test("a matcher that narrows nothing says it matches every request")
    func anyMatcher() {
        #expect(RuleDisplay.matcherSummary(.any) == "Every request")
    }

    @Test("an action reads as what the core does with a matching request")
    func actionSummary() {
        #expect(RuleDisplay.actionSummary(.allow) == "Carried as usual")
        #expect(RuleDisplay.actionSummary(.block(BlockAction(status: 451))) == "Answered 451")
        #expect(
            RuleDisplay.actionSummary(.mapLocal(MapLocalAction(path: "/tmp/a/users.json")))
                == "users.json"
        )
        #expect(
            RuleDisplay.actionSummary(
                .mapRemote(RemoteTarget(scheme: .https, host: "staging.example.com"))
            ) == "https://staging.example.com"
        )
    }

    @Test("an unset criterion is shown as Any rather than as nothing")
    func unsetCriterion() {
        #expect(RuleDisplay.criterionText(nil) == "Any")
        #expect(RuleDisplay.criterionText("") == "Any")
        #expect(RuleDisplay.criterionText("api.example.com") == "api.example.com")
    }

    @Test("every action has a name, an explanation, and a symbol")
    func actionKinds() {
        for kind in RuleActionKind.allCases {
            #expect(!kind.title.isEmpty)
            #expect(!kind.explanation.isEmpty)
            #expect(!kind.symbol.isEmpty)
        }
    }

    @Test("the window's sections each have a name and a symbol")
    func sections() {
        #expect(AppSection.allCases.map(\.title) == ["Traffic", "Rules", "Breaks"])
        for section in AppSection.allCases {
            #expect(!section.symbol.isEmpty)
        }
    }
}
