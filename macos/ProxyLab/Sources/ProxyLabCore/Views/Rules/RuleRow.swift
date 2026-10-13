//  One rule in the list.
//
//      ⏻  Products fixture          Map Local     10
//         GET api.example.com/v1/products
//         → products.json
//
//  The switch is first because a disabled rule is consulted by nothing. The
//  name is what a person scans for, the type and the priority sit on that
//  same line, and the two lines under it are the match and the action — the
//  same words the traffic-rule chooser uses. No badge, and no header value.

import SwiftUI

/// A rule, as the rules list shows it.
struct RuleRow: View {
    let rule: Rule
    let model: RulesModel

    private var presentation: RuleRowPresentation { RuleRowPresentation(rule: rule) }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Toggle("", isOn: enabled)
                .toggleStyle(.switch)
                .controlSize(.mini)
                .labelsHidden()
                .disabled(model.busyIDs.contains(rule.id))
                .padding(.top, 1)
                .help(rule.enabled ? "Consulted" : "Not consulted")
                .accessibilityLabel(rule.enabled ? "Disable \(rule.name)" : "Enable \(rule.name)")

            VStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(presentation.name)
                        .font(.system(size: 12, weight: .medium))
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .layoutPriority(1)

                    Text(presentation.type)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .fixedSize(horizontal: true, vertical: false)

                    Spacer(minLength: 4)

                    Text("\(presentation.priority)")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .help("Priority \(presentation.priority). Lower is consulted first.")
                }

                Text(presentation.match)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)

                if let action = presentation.action {
                    Text(action)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .foregroundStyle(rule.enabled ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        }
        .padding(.vertical, 2)
        .accessibilityValue(rule.enabled ? "Enabled" : "Disabled")
    }

    /// The switch reads the rule and is written through the core. Priority is
    /// not part of that write.
    private var enabled: Binding<Bool> {
        Binding(
            get: { rule.enabled },
            set: { on in Task { await model.setEnabled(on, for: rule) } }
        )
    }
}
