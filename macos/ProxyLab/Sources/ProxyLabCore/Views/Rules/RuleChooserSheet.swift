//  The rules of one kind, started from a traffic row.
//
//      Map Local…
//        ┌─ Map Local ─────────────────────────────┐
//        │ POST api.example.com /v1/users          │
//        │ ○ mock the config              Enabled  │
//        │   GET api.example.com/v1/users          │
//        │   → users.json                          │
//        │ [Delete]  [Cancel] [New Rule] [Apply]   │
//        └─────────────────────────────────────────┘
//                          │
//                          ▼  New Rule
//                    the rule editor, filled in
//                    from the selected exchange
//
//  Apply turns an existing rule on. A rule that is already on says Done,
//  which closes the sheet and leaves it on: there is no second action that
//  would apply it to this one request. The proxy applies an enabled rule
//  when a request matches it; this sheet does not match anything itself.
//  New Rule opens the same editor the Rules screen uses. Delete is the same
//  delete.

import SwiftUI

/// The rule actions a traffic row offers, in the order the menu shows them.
let trafficRuleActions: [RuleActionKind] = [
    .mapLocal, .mapRemote, .modifyRequest, .modifyResponse, .redirect, .block, .allow,
]

/// Existing rules of one kind, and the way to write a new one from a request.
struct RuleChooserSheet: View {
    let model: RulesModel
    let request: TrafficRuleRequest
    /// The whole exchange, when the core still has it. `nil` keeps the draft
    /// the row itself could fill in.
    let exchange: (String) async -> TrafficExchange?

    @State private var selectedID: String?
    @State private var creating = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
            Divider()
            footer
        }
        .frame(width: 480, height: 440)
        .task { await model.load() }
        .sheet(isPresented: editorPresented) {
            if let editor = model.editor {
                RuleEditorView(editor: editor, model: model)
            }
        }
        .confirmationDialog(
            "Delete “\(model.deleting?.name ?? "")”?",
            isPresented: deletingPresented
        ) {
            Button("Delete Rule", role: .destructive) {
                Task { await model.confirmDelete() }
            }
            Button("Cancel", role: .cancel) { model.deleting = nil }
        } message: {
            Text(
                "The rule is removed from the core and the proxy stops applying it. "
                    + "This cannot be undone."
            )
        }
    }

    // MARK: - Chrome

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: request.kind.symbol)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(request.kind.title)
                    .font(.system(size: 13, weight: .semibold))
                if !request.seed.matchLine.isEmpty {
                    Text(request.seed.matchLine)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            Spacer(minLength: 8)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var content: some View {
        if model.rules.isEmpty, model.state == .loading {
            LoadingView(title: "Loading rules…")
        } else if case let .failed(reason) = model.state, model.rules.isEmpty {
            PlaceholderView(
                symbol: "exclamationmark.triangle",
                title: "The rules could not be loaded",
                detail: reason,
                action: ("Try Again", { Task { await model.load() } })
            )
        } else if matches.isEmpty {
            PlaceholderView(
                symbol: request.kind.symbol,
                title: "No \(request.kind.title) rules",
                detail: "New Rule writes one from this request. Host, path and method "
                    + "are filled in. You can change them before saving."
            )
        } else {
            List(selection: $selectedID) {
                ForEach(matches) { rule in
                    chooserRow(rule)
                        .tag(rule.id)
                }
            }
            .listStyle(.inset)
            .accessibilityLabel("\(request.kind.title) rules")
        }
    }

    private func chooserRow(_ rule: Rule) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: rule.action.kind.symbol)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 14)
                .padding(.top, 2)

            VStack(alignment: .leading, spacing: 2) {
                Text(rule.name)
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                Text(RuleDisplay.chooserMatch(rule.matcher))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let target = RuleDisplay.chooserTarget(rule.action) {
                    Text(target)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }

            Spacer(minLength: 8)

            Text(rule.enabled ? "Enabled" : "Disabled")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(rule.enabled ? Color.green : Color.secondary)
        }
        .padding(.vertical, 4)
        .opacity(rule.enabled ? 1 : 0.72)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(rule.name), \(rule.enabled ? "enabled" : "disabled")")
        .accessibilityValue(chooserValue(rule))
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Button("Delete Rule", role: .destructive) {
                guard let selected else { return }
                model.beginDeleting(selected)
            }
            .disabled(selected == nil || creating)

            Spacer()

            Button("Cancel") { model.cancelTrafficRule() }
                .keyboardShortcut(.cancelAction)

            Button("New Rule") { create() }
                .disabled(creating)
                .keyboardShortcut(selected == nil ? .defaultAction : nil)

            Button(confirmTitle) {
                guard let selected else { return }
                Task { await model.applyRule(selected) }
            }
            .disabled(selected == nil || creating || model.busyIDs.contains(selected?.id ?? ""))
            .keyboardShortcut(selected == nil ? nil : .defaultAction)
            .help(confirmHelp)

            if creating {
                ProgressView().controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    // MARK: - What is selected

    private var matches: [Rule] { model.rules(of: request.kind) }

    private var selected: Rule? {
        guard let selectedID else { return nil }

        return matches.first { $0.id == selectedID }
    }

    /// Apply turns a rule on. Done closes the sheet when the rule is already on.
    private var confirmTitle: String {
        selected?.enabled == true ? "Done" : "Apply"
    }

    private var confirmHelp: String {
        if selected?.enabled == true {
            return "This rule is already enabled. The proxy applies it when a request matches."
        }

        return "Turns the rule on. The proxy applies it when a request matches."
    }

    private func chooserValue(_ rule: Rule) -> String {
        let match = RuleDisplay.chooserMatch(rule.matcher)
        guard let target = RuleDisplay.chooserTarget(rule.action) else { return match }

        return "\(match), \(target)"
    }

    private var editorPresented: Binding<Bool> {
        Binding(
            get: { model.editor != nil },
            set: { showing in if !showing { model.cancelEditing() } }
        )
    }

    private var deletingPresented: Binding<Bool> {
        Binding(
            get: { model.deleting != nil },
            set: { showing in if !showing { model.deleting = nil } }
        )
    }

    /// Opens the editor on a new rule of this kind, filled in from the exchange
    /// when the core still has it.
    private func create() {
        creating = true
        Task {
            defer { creating = false }
            let full = await exchange(request.exchangeID)
            guard model.trafficRule != nil else { return }

            let seed = full.map { RuleDraftSeed.from(exchange: $0, kind: request.kind) } ?? request.seed
            model.beginCreating(request.kind, seed: seed)
        }
    }
}
