//  One rule in the list.
//
//      ⏻  ⛔️ Block analytics                            Block
//         analytics.example.com · /collect                  10
//         Answered 403
//
//  Three lines, in the order a person reads a rule: what it is called, which
//  requests it is for, and what happens to them. The switch is first because a
//  disabled rule is consulted by nothing and everything else about it is then
//  beside the point.

import SwiftUI

/// A rule, as the rules list shows it.
struct RuleRow: View {
    let rule: Rule
    let model: RulesModel

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Toggle("", isOn: enabled)
                .toggleStyle(.switch)
                .controlSize(.mini)
                .labelsHidden()
                .disabled(model.busyIDs.contains(rule.id))
                .padding(.top, 1)
                .help(rule.enabled ? "Consulted for every request" : "Not consulted at all")
                .accessibilityLabel(rule.enabled ? "Disable \(rule.name)" : "Enable \(rule.name)")

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Image(systemName: rule.action.kind.symbol)
                        .font(.system(size: 10))
                        .foregroundStyle(tint)
                        .frame(width: 12)
                    Text(rule.name)
                        .font(.system(size: 12, weight: .medium))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }

                Text(RuleDisplay.matcherSummary(rule.matcher))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)

                Text(RuleDisplay.actionSummary(rule.action))
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 6)

            VStack(alignment: .trailing, spacing: 4) {
                ActionTag(kind: rule.action.kind, tint: tint)
                Text("\(rule.priority)")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .help("Priority \(rule.priority). Lower is consulted first.")
            }
        }
        .padding(.vertical, 4)
        // A disabled rule is still shown in full — it is part of the order and
        // will be again as soon as it is switched on — but it is dimmed, so a
        // list of rules reads as the rules that are actually being applied.
        .opacity(rule.enabled ? 1 : 0.5)
    }

    /// The switch, which reads the model and is written through the core.
    private var enabled: Binding<Bool> {
        Binding(
            get: { rule.enabled },
            set: { on in Task { await model.setEnabled(on, for: rule) } }
        )
    }

    /// Colour says what the rule does with a request, and nothing else.
    private var tint: Color {
        switch rule.action.kind {
        case .allow: .green
        case .block: .red
        case .mapLocal: .purple
        case .mapRemote: .blue
        case .redirect: .indigo
        case .modifyRequest, .modifyResponse: .orange
        }
    }
}

/// What kind of rule this is, as a word.
struct ActionTag: View {
    let kind: RuleActionKind
    let tint: Color

    var body: some View {
        Text(kind.title)
            .font(.system(size: 9, weight: .semibold))
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(tint.opacity(0.16), in: RoundedRectangle(cornerRadius: 4))
            .foregroundStyle(tint)
    }
}
