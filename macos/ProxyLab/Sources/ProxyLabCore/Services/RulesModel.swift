//  What the Rules screen is showing, and how it got there.
//
//      open Rules ──▶ list_rules ──▶ the rules, in the order they are consulted
//                          ▲
//      create/update/delete ┘   every change is followed by the list again
//
//  # The core keeps the order
//
//  Rules are consulted by priority ascending and then by id, and `list_rules`
//  returns them in exactly that order. So the list is never sorted here: what
//  is shown is what the core will consult, in the order it will consult them,
//  and the way to be sure of that is to never hold an order of this app's own.
//  Every change is followed by asking for the list again.
//
//  A category and a search narrow that list. They are not a second collection
//  and they do not sort it. Moving a row writes priorities for the full order.
//
//  # Nothing changes on screen before the core says it changed
//
//  Turning a rule off, moving one, and deleting one all go to the core first.
//  A switch that flips and then flips back is worse than one that takes a
//  moment, and a rule that has vanished from a list but not from the proxy is
//  worse than either.
//
//  # Rules are asked for when the Rules screen needs them
//
//  There is no polling and no subscription: rule changes are not events, and
//  the core says so. The list is loaded when the screen appears and after
//  anything this app changed. A rule changed by something else is seen the
//  next time either happens.

import Foundation
import Observation

/// What the rules list is doing.
public enum RulesListState: Equatable, Sendable {
    /// The list has not arrived yet.
    case loading
    /// The list is what the core has.
    case loaded
    /// It could not be fetched.
    case failed(String)
}

/// What a rules list with no rows should say.
public enum RulesPlaceholder: Equatable, Sendable {
    /// There are rules; the list is showing them.
    case rules
    /// The list has not arrived yet.
    case loading
    /// It could not be fetched.
    case failed(String)
    /// The core has no rules at all.
    case noRules
    /// The core has rules, and none of them are the selected kind.
    case emptyCategory(String)
    /// The core has rules, and none of them match the search.
    case noMatches
}

/// The step between the priorities the list assigns when rules are reordered.
///
/// Room between two rules for one to be given a priority by hand without
/// having to renumber anything.
public let rulePriorityStep: Int32 = 10

/// Everything the Rules screen shows.
@MainActor
@Observable
public final class RulesModel {
    /// What the list is doing.
    public private(set) var state: RulesListState = .loading
    /// The rules, in the order the core consults them in.
    public private(set) var rules: [Rule] = []
    /// Which rule the detail pane is showing.
    public var selectedID: String?
    /// Which kind of rule the list is narrowed to. The collection is the same
    /// one either way.
    public var category: RulesCategory = .all
    /// The list search. A substring of the words a row shows, composed with
    /// ``category``. Empty shows every rule in the category.
    public var searchText = ""
    /// The last thing that went wrong, for the status bar.
    public private(set) var lastError: String?
    /// The rule the editor is open on, if it is.
    public private(set) var editor: RuleEditorModel?
    /// A rule action started from a traffic row, while its chooser is open.
    public private(set) var trafficRule: TrafficRuleRequest?
    /// The rule a delete is waiting to be confirmed for.
    public var deleting: Rule?
    /// Which rules have a change in flight, so their controls can be disabled
    /// rather than clicked twice.
    public private(set) var busyIDs: Set<String> = []

    private let session: any RulesSession
    private let chooser: any LocalFileChooser

    /// Which load is the current one, so a slow answer to a load that has been
    /// superseded does not replace the list a newer one produced.
    private var loadGeneration = 0

    public init(session: any RulesSession, chooser: any LocalFileChooser) {
        self.session = session
        self.chooser = chooser
    }

    /// The rule the detail pane is showing, if it is still in the list.
    public var selected: Rule? {
        guard let selectedID else { return nil }

        return rules.first { $0.id == selectedID }
    }

    /// The rules the list is showing: ``category``, then the search, in the
    /// order the core consults them. Nothing here sorts.
    public var visibleRules: [Rule] {
        rules.filter { category.contains($0) && RuleSearch.matches($0, query: searchText) }
    }

    /// What the list should say when it has no rows.
    public var placeholder: RulesPlaceholder {
        if rules.isEmpty {
            switch state {
            case .loading: return .loading
            case let .failed(reason): return .failed(reason)
            case .loaded: return .noRules
            }
        }

        guard visibleRules.isEmpty else { return .rules }

        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        if !query.isEmpty { return .noMatches }

        return .emptyCategory(category.emptyTitle)
    }

    /// How many rules are enabled.
    public var enabledCount: Int {
        rules.count { $0.enabled }
    }

    /// How many rules ``category`` contains, ignoring the search.
    ///
    /// The counts beside the categories stay put while a search is typed.
    public func count(in category: RulesCategory) -> Int {
        rules.count { category.contains($0) }
    }

    // MARK: - Loading

    /// Asks the core for every rule, in the order it consults them in.
    public func load() async {
        loadGeneration += 1
        let generation = loadGeneration

        if rules.isEmpty { state = .loading }

        do {
            let rules = try await session.listRules()
            guard generation == loadGeneration else { return }

            self.rules = rules
            state = .loaded
            lastError = nil

            // A rule that is no longer there cannot go on being selected.
            if let selectedID, !rules.contains(where: { $0.id == selectedID }) {
                self.selectedID = nil
            }
        } catch {
            guard generation == loadGeneration else { return }

            let failure = message(for: error)
            state = .failed(failure)
            lastError = failure
        }
    }

    // MARK: - Turning one on and off

    /// Turns `rule` on or off, and shows the state the core confirmed.
    ///
    /// One field: `update_rule` changes what it names and leaves the rest, so
    /// this cannot overwrite an edit something else made in the meantime.
    public func setEnabled(_ enabled: Bool, for rule: Rule) async {
        guard !busyIDs.contains(rule.id) else { return }

        busyIDs.insert(rule.id)
        defer { busyIDs.remove(rule.id) }

        do {
            let updated = try await session.updateRule(id: rule.id, RuleUpdate(enabled: enabled))
            replace(updated)
            lastError = nil
        } catch let error as IPCError where error.serverCode == .notFound {
            // Removed by something else. The list is wrong, not the request —
            // so it is fetched again, and the reason is set afterwards,
            // because a successful load clears whatever went wrong last.
            await load()
            lastError = "That rule no longer exists."
        } catch {
            // The switch goes back to what the core still holds, because that
            // is what the proxy is still applying.
            lastError = message(for: error)
        }
    }

    // MARK: - The order rules are consulted in

    /// Whether `rule` can be moved further up the list that is showing.
    ///
    /// That list is the consultation order, narrowed by the category and the
    /// search. Moving up passes whatever the filter is hiding between this
    /// rule and the one above it, and leaves every other rule in the same
    /// order relative to the rest.
    public func canMoveUp(_ rule: Rule) -> Bool {
        guard busyIDs.isEmpty, let index = visibleRules.firstIndex(where: { $0.id == rule.id })
        else {
            return false
        }

        return index > 0
    }

    /// Whether `rule` can be moved further down the list that is showing.
    public func canMoveDown(_ rule: Rule) -> Bool {
        guard busyIDs.isEmpty, let index = visibleRules.firstIndex(where: { $0.id == rule.id })
        else {
            return false
        }

        return index < visibleRules.count - 1
    }

    /// Moves `rule` one place earlier in the list that is showing.
    public func moveUp(_ rule: Rule) async {
        guard let index = visibleRules.firstIndex(where: { $0.id == rule.id }), index > 0 else {
            return
        }

        await moveVisible(from: index, to: index - 1)
    }

    /// Moves `rule` one place later in the list that is showing.
    public func moveDown(_ rule: Rule) async {
        guard let index = visibleRules.firstIndex(where: { $0.id == rule.id }),
            index < visibleRules.count - 1
        else {
            return
        }

        await moveVisible(from: index, to: index + 2)
    }

    /// Moves the visible rows `source` describes to `destination`.
    ///
    /// `destination` is the index to insert before, in the visible list as it
    /// is before the move. One row at a time: a drag moves one rule.
    public func moveVisible(from source: IndexSet, to destination: Int) async {
        guard source.count == 1, let source = source.first else { return }

        await moveVisible(from: source, to: destination)
    }

    /// Moves the visible rule at `source` to `destination`.
    ///
    /// See ``RuleOrder`` for where that lands in the full consultation order.
    /// The priorities written afterwards are the same steps an unfiltered move
    /// writes: ``rulePriorityStep`` from the top, and only where the number
    /// actually changes.
    public func moveVisible(from source: Int, to destination: Int) async {
        guard busyIDs.isEmpty else { return }
        guard let order = RuleOrder.moving(rules, visible: visibleRules, from: source, to: destination)
        else {
            return
        }

        await persist(order)
    }

    /// Writes a new order by writing new priorities, then reloads.
    ///
    /// The order is the priorities and nothing else — there is no sequence
    /// column and the app does not keep one — so moving a rule means giving
    /// the rules priorities that produce the wanted order. They are renumbered
    /// in steps of ``rulePriorityStep``, and only the rules whose priority
    /// actually changes are sent.
    ///
    /// Renumbering rather than swapping two numbers, because two rules may sit
    /// at the same priority — the core breaks that tie by id, which is not
    /// something a person can move a rule past. Spacing them out is what makes
    /// the displayed order a thing the core will agree with. A rule that did
    /// not move can still be given a new number when the spacing requires it;
    /// its place relative to the other rules that did not move does not change.
    private func persist(_ order: [Rule]) async {
        let changes = order.enumerated()
            .map { (rule: $0.element, priority: Int32($0.offset) * rulePriorityStep) }
            .filter { $0.rule.priority != $0.priority }

        guard !changes.isEmpty else { return }

        busyIDs = Set(changes.map(\.rule.id))
        defer { busyIDs = [] }

        var failure: String?
        do {
            for change in changes {
                _ = try await session.updateRule(
                    id: change.rule.id,
                    RuleUpdate(priority: change.priority)
                )
            }
        } catch {
            failure = message(for: error)
        }

        // Always, and whether or not every write landed: the order shown is
        // the order the core will consult, so it comes from the core. A move
        // that failed part way through leaves some rules renumbered and some
        // not, and what that produced is exactly what has to be shown.
        await load()
        lastError = failure
    }

    /// The rules whose action is `kind`, in the order the core consults them.
    public func rules(of kind: RuleActionKind) -> [Rule] {
        rules.filter { $0.action.kind == kind }
    }

    // MARK: - From a traffic row

    /// Opens the chooser for `request`: the rules of that kind, and the way
    /// to write a new one from the exchange.
    public func beginTrafficRule(_ request: TrafficRuleRequest) {
        editor = nil
        deleting = nil
        trafficRule = request
    }

    /// Closes the chooser without writing a rule.
    public func cancelTrafficRule() {
        trafficRule = nil
    }

    /// Turns `rule` on, if it is off, and closes the chooser once that has
    /// happened. A rule that is already on is already applied.
    @discardableResult
    public func applyRule(_ rule: Rule) async -> Bool {
        if !rule.enabled {
            await setEnabled(true, for: rule)
            guard rules.contains(where: { $0.id == rule.id && $0.enabled }) else { return false }
        }

        trafficRule = nil

        return true
    }

    // MARK: - The editor

    /// Opens the editor on a new rule of `kind`.
    ///
    /// `seed` fills the fields a captured exchange can supply. The editor is
    /// still a draft until it is saved.
    public func beginCreating(_ kind: RuleActionKind, seed: RuleDraftSeed? = nil) {
        editor = RuleEditorModel(creating: kind, seed: seed, session: session, chooser: chooser)
    }

    /// Opens the editor on a rule that exists.
    public func beginEditing(_ rule: Rule) {
        editor = RuleEditorModel(editing: rule, session: session, chooser: chooser)
    }

    /// Opens the editor on a new rule copied from `rule`.
    ///
    /// The copy is a draft until it is saved. It does not reuse `rule`'s id:
    /// saving it creates a rule, and the core mints the id. Nothing is sent
    /// until then.
    public func beginDuplicating(_ rule: Rule) {
        editor = RuleEditorModel(duplicating: rule, session: session, chooser: chooser)
    }

    /// Closes the editor, discarding whatever was in it.
    ///
    /// Nothing was sent, so there is nothing to undo.
    public func cancelEditing() {
        editor = nil
    }

    /// Saves what the editor holds, and closes it only if the core kept it.
    ///
    /// `true` when the rule was saved. When it was not, the editor is still
    /// open with everything still in it and the reason under the field it
    /// belongs to.
    @discardableResult
    public func saveEditor() async -> Bool {
        guard let editor else { return false }

        guard let saved = await editor.save() else { return false }

        self.editor = nil
        selectedID = saved.id
        lastError = nil

        // The core decides the order, and a new or re-prioritised rule may
        // have changed it.
        await load()
        reveal(saved.id)

        return true
    }

    // MARK: - Deleting

    /// Asks for a rule to be removed, once it is confirmed.
    ///
    /// Held rather than removed: the confirmation is the step, and the delete
    /// happens in ``confirmDelete()``.
    public func beginDeleting(_ rule: Rule) {
        deleting = rule
    }

    /// Removes the rule a delete was confirmed for.
    @discardableResult
    public func confirmDelete() async -> Bool {
        guard let rule = deleting else { return false }

        deleting = nil
        busyIDs.insert(rule.id)
        defer { busyIDs.remove(rule.id) }

        do {
            let result = try await session.deleteRule(id: rule.id)

            if selectedID == rule.id { selectedID = nil }
            await load()
            // `deleted: false` means it was already gone, which is the same
            // end state and not a failure — but it is worth saying, and it is
            // said after the reload, which clears whatever went wrong last.
            lastError = result.deleted ? nil : "That rule no longer existed."

            return true
        } catch {
            lastError = message(for: error)

            return false
        }
    }

    // MARK: - Housekeeping

    /// Makes a rule the list is not showing visible, so a save is not hidden
    /// by the category or the search that was up while it was written.
    private func reveal(_ id: String) {
        guard let rule = rules.first(where: { $0.id == id }),
            !visibleRules.contains(where: { $0.id == id })
        else {
            return
        }

        if !category.contains(rule) { category = .all }

        searchText = ""
    }

    /// Puts one changed rule back in the list where it already sits.
    ///
    /// The order cannot have changed: the only single-field change that goes
    /// through here is `enabled`, and the order is priority then id.
    private func replace(_ rule: Rule) {
        guard let index = rules.firstIndex(where: { $0.id == rule.id }) else { return }

        rules[index] = rule
    }

    /// What a person is told about `error`.
    private func message(for error: any Error) -> String {
        (error as? IPCError)?.message ?? "Something went wrong talking to the ProxyLab core."
    }
}
