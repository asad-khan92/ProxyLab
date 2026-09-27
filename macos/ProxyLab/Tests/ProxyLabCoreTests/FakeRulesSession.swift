//  A core that answers rule requests with whatever a test told it to.
//
//  The rules model's job is what it does with an answer — what it shows, what
//  it sends next, what it refuses to change until the core agrees — and none of
//  that needs a socket. Every call returns what it was configured to return and
//  records that it happened, so a test asserts on the requests that were made
//  rather than on a delay.

import Foundation

@testable import ProxyLabCore

/// One call a test can assert was made.
enum RuleCall: Equatable {
    case list
    case get(String)
    case create(NewRule)
    case update(String, RuleUpdate)
    case delete(String)
}

/// A `RulesSession` whose answers are set by the test.
actor FakeRulesSession: RulesSession {
    /// Every call made, in order.
    private(set) var calls: [RuleCall] = []

    private var lists: [Result<[Rule], IPCError>] = []
    private var getResult: Result<Rule, IPCError>?
    private var createResult: Result<Rule, IPCError>?
    private var updateResult: Result<Rule, IPCError>?
    private var updateResults: [Result<Rule, IPCError>] = []
    private var deleteResult: Result<RuleDeletedResult, IPCError>?

    // MARK: - What the test sets

    /// The answers to successive `listRules` calls, in order.
    ///
    /// The last one is repeated once they run out, so a test that only cares
    /// about the first list sets one.
    func answerLists(with answers: [Result<[Rule], IPCError>]) { lists = answers }

    /// The answer to every `listRules` call.
    func answerList(with rules: [Rule]) { lists = [.success(rules)] }

    func answerGet(with answer: Result<Rule, IPCError>) { getResult = answer }
    func answerCreate(with answer: Result<Rule, IPCError>) { createResult = answer }
    func answerUpdate(with answer: Result<Rule, IPCError>) { updateResult = answer }

    /// The answers to successive `updateRule` calls, in order, before the
    /// standing answer is used.
    func answerUpdates(with answers: [Result<Rule, IPCError>]) { updateResults = answers }

    func answerDelete(with answer: Result<RuleDeletedResult, IPCError>) { deleteResult = answer }

    /// The updates that were sent, in order.
    func updates() -> [(id: String, update: RuleUpdate)] {
        calls.compactMap { call in
            guard case let .update(id, update) = call else { return nil }

            return (id, update)
        }
    }

    /// How many times the list was asked for.
    func listCount() -> Int {
        calls.count { $0 == .list }
    }

    // MARK: - RulesSession

    func listRules() async throws -> [Rule] {
        calls.append(.list)

        guard !lists.isEmpty else { return [] }

        return try (lists.count == 1 ? lists[0] : lists.removeFirst()).get()
    }

    func rule(id: String) async throws -> Rule {
        calls.append(.get(id))

        guard let getResult else { throw IPCError.notConnected }

        return try getResult.get()
    }

    func createRule(_ rule: NewRule) async throws -> Rule {
        calls.append(.create(rule))

        guard let createResult else { throw IPCError.notConnected }

        return try createResult.get()
    }

    func updateRule(id: String, _ update: RuleUpdate) async throws -> Rule {
        calls.append(.update(id, update))

        if !updateResults.isEmpty { return try updateResults.removeFirst().get() }
        guard let updateResult else { throw IPCError.notConnected }

        return try updateResult.get()
    }

    func deleteRule(id: String) async throws -> RuleDeletedResult {
        calls.append(.delete(id))

        guard let deleteResult else { return RuleDeletedResult(ruleID: id, deleted: true) }

        return try deleteResult.get()
    }
}

/// A file chooser that answers with whatever the test put in it.
///
/// What the open panel would have returned, without a panel: `nil` is a
/// cancelled pick, which is the case worth being able to write a test for.
@MainActor
final class FakeFileChooser: LocalFileChooser {
    /// What the next pick returns.
    var answer: String?
    /// The paths the chooser was opened at, in order.
    private(set) var openedAt: [String?] = []
    /// How many times it was opened.
    private(set) var openCount = 0

    init(answer: String? = nil) {
        self.answer = answer
    }

    func chooseFile(startingAt current: String?) -> String? {
        openCount += 1
        openedAt.append(current)

        return answer
    }
}
