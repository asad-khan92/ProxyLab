//  The selected rule, split into what it matches and what it does.
//
//  Editing opens the same editor a new rule uses. This pane does not keep a
//  second copy of those fields, and it does not save on its own: Edit, then
//  Save or Cancel in the editor.

import SwiftUI

/// The selected rule, in full.
struct RuleDetailView: View {
    let model: RulesModel

    var body: some View {
        if let rule = model.selected, model.visibleRules.contains(where: { $0.id == rule.id }) {
            detail(for: rule)
        } else {
            PlaceholderView(
                symbol: "slider.horizontal.3",
                title: "No rule selected",
                detail: "Select a rule to see what it matches and what it does."
            )
        }
    }

    private func detail(for rule: Rule) -> some View {
        let content = RuleDetailContent(rule: rule)

        return ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header(for: rule)
                section("Match", note: "An empty criterion matches anything. A path ending in * is a prefix.") {
                    ForEach(Array(content.match.enumerated()), id: \.offset) { _, line in
                        DetailLine(label: line.label, value: line.value)
                    }
                }
                section("Action", note: content.actionNote) {
                    ForEach(Array(content.action.enumerated()), id: \.offset) { _, line in
                        DetailLine(label: line.label, value: line.value)
                    }
                    if content.action.isEmpty, content.changes.isEmpty {
                        Text("This rule changes nothing.")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                    }
                    ForEach(Array(content.changes.enumerated()), id: \.offset) { _, change in
                        Text(change)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                    }
                }
                order(for: rule)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func header(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(rule.name)
                    .font(.system(size: 15, weight: .semibold))
                    .lineLimit(2)
                    .textSelection(.enabled)
                Spacer(minLength: 8)
                Text(rule.action.kind.title)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 8) {
                Toggle("", isOn: enabled(rule))
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
                    .disabled(model.busyIDs.contains(rule.id))
                    .accessibilityLabel(rule.enabled ? "Disable \(rule.name)" : "Enable \(rule.name)")

                Text(rule.enabled ? "Enabled" : "Disabled")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
            }

            HStack(spacing: 8) {
                Button("Edit") { model.beginEditing(rule) }
                Button("Duplicate Rule") { model.beginDuplicating(rule) }
                Spacer(minLength: 8)
                Button("Delete Rule", role: .destructive) { model.beginDeleting(rule) }
            }
            .controlSize(.small)
        }
    }

    private func enabled(_ rule: Rule) -> Binding<Bool> {
        Binding(
            get: { rule.enabled },
            set: { on in Task { await model.setEnabled(on, for: rule) } }
        )
    }

    private func section<Content: View>(
        _ title: String,
        note: String?,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
            if let note {
                Text(note)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            content()
        }
    }

    private func order(for rule: Rule) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Order")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)

            HStack(spacing: 8) {
                if let place = model.rules.firstIndex(where: { $0.id == rule.id }) {
                    Text("\(place + 1) of \(model.rules.count)")
                        .font(.system(size: 11, design: .monospaced))
                }
                Text("Priority \(rule.priority)")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .help("Lower is consulted first. Equal priorities are ordered by id, oldest first.")

                Spacer(minLength: 8)

                Button {
                    Task { await model.moveUp(rule) }
                } label: {
                    Label("Move Up", systemImage: "chevron.up")
                }
                .labelStyle(.iconOnly)
                .disabled(!model.canMoveUp(rule))
                .help("Consult this rule earlier")

                Button {
                    Task { await model.moveDown(rule) }
                } label: {
                    Label("Move Down", systemImage: "chevron.down")
                }
                .labelStyle(.iconOnly)
                .disabled(!model.canMoveDown(rule))
                .help("Consult this rule later")
            }
            .controlSize(.small)

            Text("Modify rules apply until the first deciding rule. Nothing below that decision is consulted.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// One labelled value in the detail pane.
private struct DetailLine: View {
    let label: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 88, alignment: .leading)

            Text(value)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(value == RuleDisplay.anything ? .tertiary : .primary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
