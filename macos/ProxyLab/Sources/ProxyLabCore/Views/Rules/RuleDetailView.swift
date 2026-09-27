//  Everything about the selected rule, and the things that can be done to it.
//
//  The whole configuration, not a summary: a rule that is not doing what
//  someone expected is usually a rule whose matcher says something slightly
//  different from what they read in the list. So every criterion is shown,
//  including the ones that are not set — because an unset criterion matches
//  everything, and that is the part people get wrong.

import SwiftUI

/// The selected rule, in full.
struct RuleDetailView: View {
    let model: RulesModel

    var body: some View {
        if let rule = model.selected {
            detail(for: rule)
        } else {
            PlaceholderView(
                symbol: "slider.horizontal.3",
                title: "No rule selected",
                detail: "Select a rule to see which requests it is for and what it does "
                    + "with them."
            )
        }
    }

    private func detail(for rule: Rule) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header(for: rule)
                Divider()
                matching(for: rule)
                Divider()
                action(for: rule)
                Divider()
                order(for: rule)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toolbar {
            ToolbarItemGroup(placement: .automatic) {
                Button {
                    model.beginEditing(rule)
                } label: {
                    Label("Edit Rule", systemImage: "pencil")
                }
                .help("Change this rule")

                Button(role: .destructive) {
                    model.beginDeleting(rule)
                } label: {
                    Label("Delete Rule", systemImage: "trash")
                }
                .help("Remove this rule")
            }
        }
    }

    // MARK: - Sections

    private func header(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: rule.action.kind.symbol)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                Text(rule.name)
                    .font(.system(size: 15, weight: .semibold))
                    .textSelection(.enabled)
            }

            HStack(spacing: 8) {
                Label(
                    rule.enabled ? "Enabled" : "Disabled",
                    systemImage: rule.enabled ? "checkmark.circle.fill" : "pause.circle"
                )
                .font(.system(size: 11))
                .foregroundStyle(rule.enabled ? Color.green : Color.secondary)

                Text(rule.action.kind.explanation)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func matching(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeading(
                "Matches",
                help: "Every criterion that is set must match. One that is not set matches "
                    + "anything."
            )

            DetailRow(
                "Host",
                value: rule.matcher.host?.text,
                note: "exact, case-insensitive; the port is never compared"
            )
            DetailRow(
                "Path",
                value: rule.matcher.path?.text,
                note: rule.matcher.path?.isPrefix == true
                    ? "every path starting with this; the query is never compared"
                    : "exact, case-sensitive; the query is never compared"
            )
            DetailRow(
                "Method",
                value: rule.matcher.method?.text.uppercased(),
                note: "exact, case-insensitive"
            )

            if rule.matcher.isAny {
                Text("This rule matches every request the proxy carries.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
            }
        }
    }

    @ViewBuilder
    private func action(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeading("Action", help: nil)

            DetailRow("Does", value: rule.action.kind.title, note: nil)

            switch rule.action {
            case .allow:
                Text("The request is carried to its origin as the proxy would anyway.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            case let .block(block):
                DetailRow(
                    "Answers",
                    value: "\(block.status)",
                    note: "with one fixed sentence the core writes; the origin is never "
                        + "contacted"
                )
            case let .mapLocal(map):
                DetailRow(
                    "File",
                    value: RuleDisplay.abbreviated(map.path),
                    note: "read by the core each time the rule answers; nothing from the "
                        + "request is joined to this path"
                )
                DetailRow(
                    "Content type",
                    value: map.contentType,
                    note: "worked out from the file's extension when not set"
                )
                DetailRow("Answers", value: "\(map.status)", note: nil)
            case let .mapRemote(target):
                DetailRow("Destination", value: target.url, note: nil)
                DetailRow(
                    "Path",
                    value: target.path,
                    note: "the request's own path is kept when this is not set"
                )
                DetailRow(
                    "Query",
                    value: target.query,
                    note: "the request's own query is kept when this is not set"
                )
                Text(
                    "The method, body, and headers are the request's own. Only Host is "
                        + "rewritten, to name where the request is actually going."
                )
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            case let .redirect(redirect):
                DetailRow(
                    "Location",
                    value: redirect.target.url,
                    note: "exactly as written; nothing from the request is carried over"
                )
                DetailRow(
                    "Answers",
                    value: "\(redirect.status)",
                    note: "the origin is never contacted"
                )
            case let .modifyRequest(modify):
                changeList(RuleDisplay.changes(of: modify))
                note(
                    "Applied only to a request that is sent to an origin, after any request "
                        + "breakpoint. The traffic list records the request as it was sent."
                )
            case let .modifyResponse(modify):
                changeList(RuleDisplay.changes(of: modify))
                note(
                    "Applied to the origin's response before any response breakpoint shows "
                        + "it. The traffic list records what the client was given."
                )
            }
        }
    }

    /// What a Modify rule changes, one line each.
    private func changeList(_ changes: [String]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(changes.enumerated()), id: \.offset) { _, change in
                Label(change, systemImage: "pencil")
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func order(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeading(
                "Order",
                help: "Rules are consulted by priority, lowest first. The first enabled rule "
                    + "that matches decides; nothing after it is consulted. A Modify rule "
                    + "decides nothing, so the rules after it still are."
            )

            HStack(spacing: 10) {
                DetailRow("Priority", value: "\(rule.priority)", note: nil)

                Spacer()

                Button {
                    Task { await model.moveUp(rule) }
                } label: {
                    Label("Move Up", systemImage: "chevron.up")
                }
                .disabled(!model.canMoveUp(rule))
                .help("Consult this rule earlier")

                Button {
                    Task { await model.moveDown(rule) }
                } label: {
                    Label("Move Down", systemImage: "chevron.down")
                }
                .disabled(!model.canMoveDown(rule))
                .help("Consult this rule later")
            }
            .labelStyle(.iconOnly)
            .controlSize(.small)
        }
    }
}

/// A heading above one group of a rule's configuration.
private struct SectionHeading: View {
    let title: String
    let help: String?

    init(_ title: String, help: String?) {
        self.title = title
        self.help = help
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
            if let help {
                Text(help)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// One labelled value, with what the core does with it underneath.
///
/// A value that is not set says "Any" rather than being blank, because a
/// criterion that is not set matches everything and a blank line reads as a
/// criterion that matches nothing.
private struct DetailRow: View {
    let label: String
    let value: String?
    let note: String?

    init(_ label: String, value: String?, note: String?) {
        self.label = label
        self.value = value
        self.note = note
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 92, alignment: .leading)

            VStack(alignment: .leading, spacing: 1) {
                Text(value ?? RuleDisplay.anything)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(value == nil ? .tertiary : .primary)
                    .textSelection(.enabled)
                if let note {
                    Text(note)
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}
