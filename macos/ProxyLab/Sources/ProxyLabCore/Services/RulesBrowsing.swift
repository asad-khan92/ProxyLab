//  How the Rules screen narrows and reads the one rules collection.
//
//  Categories and search filter that collection. They do not sort it, and
//  they do not keep a list of their own: `list_rules` is already in the order
//  the core consults, and a filter leaves that order alone.
//
//  Moving a rule still means writing priorities the core will agree with.
//  A filtered list is a subset of that order, so a move inserts the rule
//  among the others rather than renumbering a private sequence.

import CoreGraphics
import Foundation

// MARK: - Categories

/// Which rules the list is showing.
///
/// One collection, narrowed. `all` is every rule the core returned.
public enum RulesCategory: String, CaseIterable, Identifiable, Hashable, Sendable {
    case all
    case mapLocal
    case mapRemote
    case modifyRequest
    case modifyResponse
    case redirect
    case block
    case allow

    public var id: String { rawValue }

    /// The sidebar label.
    var title: String {
        switch self {
        case .all: "All"
        case .mapLocal: "Map Local"
        case .mapRemote: "Map Remote"
        case .modifyRequest: "Modify Request"
        case .modifyResponse: "Modify Response"
        case .redirect: "Redirect"
        case .block: "Block"
        case .allow: "Allow"
        }
    }

    /// What an empty category says.
    var emptyTitle: String {
        switch self {
        case .all: "No rules yet"
        default: "No \(title) rules"
        }
    }

    /// The action this category is limited to, if it is limited to one.
    var kind: RuleActionKind? {
        switch self {
        case .all: nil
        case .mapLocal: .mapLocal
        case .mapRemote: .mapRemote
        case .modifyRequest: .modifyRequest
        case .modifyResponse: .modifyResponse
        case .redirect: .redirect
        case .block: .block
        case .allow: .allow
        }
    }

    /// Whether `rule` belongs in this category.
    func contains(_ rule: Rule) -> Bool {
        guard let kind else { return true }

        return rule.action.kind == kind
    }
}

// MARK: - Search

/// A substring search over the words a rule is shown with.
///
/// Name, host, path, method, and the action summary. No query language, and
/// no header value: the list does not show those, and a search should not
/// either.
enum RuleSearch {
    static func matches(_ rule: Rule, query: String) -> Bool {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return true }

        let row = RuleRowPresentation(rule: rule)
        let fields = [
            row.name,
            row.type,
            row.match,
            row.action ?? "",
            rule.matcher.host?.text ?? "",
            rule.matcher.path?.text ?? "",
            rule.matcher.method?.text ?? "",
            RuleDisplay.matcherSummary(rule.matcher),
            RuleDisplay.actionSummary(rule.action),
        ]

        return fields.contains { $0.localizedStandardContains(query) }
    }
}

// MARK: - The row

/// One rule, as a list row reads it.
///
/// The match and the action are ``RuleDisplay``'s lines, the same ones the
/// traffic-rule chooser uses, so the two screens cannot describe one rule
/// two ways.
struct RuleRowPresentation: Equatable {
    let enabled: Bool
    let name: String
    let type: String
    let match: String
    /// What the rule does with a match, when there is something to say past
    /// the type. Header values are not here.
    let action: String?
    let priority: Int32

    init(rule: Rule) {
        enabled = rule.enabled
        name = rule.name
        type = rule.action.kind.title
        match = RuleDisplay.chooserMatch(rule.matcher)
        action = Self.actionLine(rule.action)
        priority = rule.priority
    }

    /// The action line, preferring the chooser's target and falling back to
    /// the longer summary for an action that has no target — Allow, which
    /// says it carries the request.
    private static func actionLine(_ action: RuleAction) -> String? {
        if let target = RuleDisplay.chooserTarget(action), !target.isEmpty {
            return target
        }

        let summary = RuleDisplay.actionSummary(action)

        return summary.isEmpty ? nil : summary
    }
}

// MARK: - The detail

/// The selected rule, split into Match and Action the way the detail pane
/// shows it. Header values are left out, the same way the row leaves them out.
struct RuleDetailContent: Equatable {
    struct Line: Equatable {
        let label: String
        let value: String
    }

    let match: [Line]
    /// Action configuration that is not a list of edits.
    let action: [Line]
    /// What a Modify rule changes, one phrase each.
    let changes: [String]
    let actionNote: String

    init(rule: Rule) {
        match = [
            Line(label: "Host", value: RuleDisplay.criterionText(rule.matcher.host?.text)),
            Line(label: "Path", value: Self.path(rule.matcher.path)),
            Line(
                label: "Method",
                value: RuleDisplay.criterionText(rule.matcher.method?.text.uppercased())
            ),
        ]

        switch rule.action {
        case .allow:
            action = [Line(label: "Result", value: "Carried as usual")]
            changes = []
            actionNote = "The request is carried on. Nothing below this rule is consulted."
        case let .block(block):
            action = [Line(label: "Status", value: String(block.status))]
            changes = []
            actionNote = "Answered here. The origin is not contacted."
        case let .mapLocal(map):
            let type = map.contentType.flatMap { $0.isEmpty ? nil : $0 } ?? "From the file extension"
            action = [
                Line(label: "Local file", value: RuleDisplay.abbreviated(map.path)),
                Line(label: "Status", value: String(map.status)),
                Line(label: "Content type", value: type),
            ]
            changes = []
            actionNote = "Answered from this file. Nothing from the request is added to the path."
        case let .mapRemote(target):
            action = [Line(label: "Destination", value: target.url)]
            changes = []
            actionNote = "Sent to this URL. The request's method, headers, and body are kept."
        case let .redirect(redirect):
            action = [
                Line(label: "Location", value: redirect.target.url),
                Line(label: "Status", value: String(redirect.status)),
            ]
            changes = []
            actionNote = "Answered with this Location. The origin is not contacted."
        case let .modifyRequest(modify):
            action = []
            changes = RuleDisplay.changes(of: modify)
            actionNote =
                "Applied before the request is sent. Rules below this one are still consulted."
        case let .modifyResponse(modify):
            action = []
            changes = RuleDisplay.changes(of: modify)
            actionNote =
                "Applied to the origin's response. Rules below this one are still consulted."
        }
    }

    private static func path(_ path: PathMatch?) -> String {
        guard let path else { return RuleDisplay.anything }

        return path.isPrefix ? "\(path.text)*" : path.text
    }
}

// MARK: - Empty copy

/// The words an empty rules list uses.
struct RulesEmptyCopy: Equatable {
    let title: String
    let detail: String?

    /// `nil` when the list has rows to show.
    static func forPlaceholder(_ placeholder: RulesPlaceholder) -> RulesEmptyCopy? {
        switch placeholder {
        case .rules:
            return nil
        case .loading:
            return RulesEmptyCopy(title: "Loading rules…", detail: nil)
        case let .failed(reason):
            return RulesEmptyCopy(title: "The rules could not be loaded", detail: reason)
        case .noRules:
            return RulesEmptyCopy(
                title: "No rules yet",
                detail: "Create a rule or right-click captured traffic to start from a request."
            )
        case .noMatches:
            return RulesEmptyCopy(title: "No matching rules", detail: nil)
        case let .emptyCategory(title):
            return RulesEmptyCopy(title: title, detail: nil)
        }
    }
}

// MARK: - Order

/// Where a rule lands in the consultation order when it is dragged or moved.
///
/// `destination` is the index to insert before in `visible`, in that list as
/// it stands before the move — the index a list's `onMove` reports.
///
/// Rules that are not in `visible` keep their relative order. The moved rule
/// is placed in front of the visible rule that should follow it, which means
/// it passes any rule that sat between the two. Their order against each
/// other does not change. Priorities are assigned afterwards, by the same
/// step the unfiltered move already uses.
enum RuleOrder {
    static func moving(
        _ rules: [Rule],
        visible: [Rule],
        from source: Int,
        to destination: Int
    ) -> [Rule]? {
        guard visible.indices.contains(source) else { return nil }
        guard (0...visible.count).contains(destination) else { return nil }
        guard destination != source, destination != source + 1 else { return nil }

        let moving = visible[source]
        let before = destination < visible.count ? visible[destination] : nil
        guard before?.id != moving.id else { return nil }

        var order = rules
        guard let current = order.firstIndex(where: { $0.id == moving.id }) else { return nil }

        let rule = order.remove(at: current)
        let insert: Int
        if let before {
            guard let index = order.firstIndex(where: { $0.id == before.id }) else { return nil }

            insert = index
        } else {
            insert = order.count
        }

        order.insert(rule, at: insert)
        guard order.map(\.id) != rules.map(\.id) else { return nil }

        return order
    }
}

// MARK: - Width

/// How the three regions fit across the window.
///
/// The window's minimum is wide enough for ``columns``. The other two are
/// what the screen does when it is given less, so nothing has to scroll
/// sideways to reach a control.
enum RulesScreenLayout: Equatable {
    /// Categories, the list, and the selected rule.
    case columns
    /// The list and the selected rule. Categories become a menu.
    case listAndDetail
    /// The list, or the selected rule, one at a time.
    case stacked

    /// Three columns fit from here up. The window opens wider than this and
    /// cannot be narrower than 900.
    static let columnsWidth: CGFloat = 860
    /// List and detail fit from here up.
    static let listAndDetailWidth: CGFloat = 640

    static func forWidth(_ width: CGFloat) -> RulesScreenLayout {
        if width >= columnsWidth { return .columns }
        if width >= listAndDetailWidth { return .listAndDetail }

        return .stacked
    }
}
