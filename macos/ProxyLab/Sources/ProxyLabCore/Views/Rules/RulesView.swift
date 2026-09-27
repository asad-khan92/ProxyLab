//  The rules screen: every rule, in the order the core consults them, and the
//  one that is selected.
//
//      ┌──────────────────────────────────────────────────────┐
//      │ ● Connected  [ Traffic | Rules ]        + New Rule ▾ │
//      ├──────────────────┬───────────────────────────────────┤
//      │ ⏻ Block  analytics│ Block analytics                   │
//      │   analytics.exam… │ Matches   analytics.example.com   │
//      │   → 403        10 │ Action    Block · 403             │
//      │ ⏾ Map Local  cfg  │ Order     Priority 10             │
//      │   api.example.com │                                   │
//      ├──────────────────┴───────────────────────────────────┤
//      │ 2 rules, 1 enabled                                    │
//      └──────────────────────────────────────────────────────┘
//
//  The order on screen is the order the core will consult: `list_rules` returns
//  them sorted and this never sorts them again. The first enabled rule that
//  matches a request decides it, and the list is read top to bottom for exactly
//  that reason.

import SwiftUI

/// ProxyLab's rules.
public struct RulesView: View {
    private let model: RulesModel

    @Binding private var section: AppSection

    public init(model: RulesModel, section: Binding<AppSection>) {
        self.model = model
        _section = section
    }

    public var body: some View {
        NavigationSplitView {
            list
                .navigationSplitViewColumnWidth(min: 300, ideal: 360, max: 520)
        } detail: {
            RuleDetailView(model: model)
        }
        .navigationTitle("ProxyLab")
        .navigationSubtitle("Rules")
        .toolbar { toolbar }
        .safeAreaInset(edge: .bottom, spacing: 0) { statusBar }
        .task { await model.load() }
        .sheet(isPresented: editorBinding) {
            if let editor = model.editor {
                RuleEditorView(editor: editor, model: model)
            }
        }
        .confirmationDialog(
            "Delete “\(model.deleting?.name ?? "")”?",
            isPresented: deletingBinding
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

    // MARK: - The list

    @ViewBuilder
    private var list: some View {
        switch model.placeholder {
        case .rules:
            rows
        case .loading:
            LoadingView(title: "Loading rules…")
        case let .failed(reason):
            PlaceholderView(
                symbol: "exclamationmark.triangle",
                title: "The rules could not be loaded",
                detail: reason,
                action: ("Try Again", { Task { await model.load() } })
            )
        case .noRules:
            PlaceholderView(
                symbol: "slider.horizontal.3",
                title: "No rules",
                detail: "A rule blocks a request, answers it from a file on this Mac, or "
                    + "sends it somewhere else. Requests nothing matches are carried as usual.",
                action: ("New Rule", { model.beginCreating(.block) })
            )
        }
    }

    private var rows: some View {
        List(selection: selection) {
            ForEach(model.rules) { rule in
                RuleRow(rule: rule, model: model)
                    .tag(rule.id)
                    .contextMenu { menu(for: rule) }
            }
        }
        .listStyle(.inset)
    }

    @ViewBuilder
    private func menu(for rule: Rule) -> some View {
        Button("Edit…") { model.beginEditing(rule) }
        Button(rule.enabled ? "Disable" : "Enable") {
            Task { await model.setEnabled(!rule.enabled, for: rule) }
        }
        Divider()
        Button("Move Up") { Task { await model.moveUp(rule) } }
            .disabled(!model.canMoveUp(rule))
        Button("Move Down") { Task { await model.moveDown(rule) } }
            .disabled(!model.canMoveDown(rule))
        Divider()
        Button("Delete…", role: .destructive) { model.beginDeleting(rule) }
    }

    /// The list's selection, which is the model's.
    private var selection: Binding<String?> {
        Binding(get: { model.selectedID }, set: { model.selectedID = $0 })
    }

    /// Whether the editor sheet is up. Closing it discards the draft, which is
    /// what cancelling means: nothing was sent, so there is nothing to undo.
    private var editorBinding: Binding<Bool> {
        Binding(get: { model.editor != nil }, set: { if !$0 { model.cancelEditing() } })
    }

    private var deletingBinding: Binding<Bool> {
        Binding(get: { model.deleting != nil }, set: { if !$0 { model.deleting = nil } })
    }

    // MARK: - Chrome

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            SectionPicker(section: $section)
        }

        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                Task { await model.load() }
            } label: {
                Label("Reload Rules", systemImage: "arrow.clockwise")
            }
            .help("Ask the core for the rules again")

            Menu {
                ForEach(RuleActionKind.allCases) { kind in
                    Button {
                        model.beginCreating(kind)
                    } label: {
                        Label(kind.title, systemImage: kind.symbol)
                    }
                }
            } label: {
                Label("New Rule", systemImage: "plus")
            }
            .help("Write a new rule")
        }
    }

    private var statusBar: some View {
        HStack(spacing: 10) {
            Text(countLabel)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if let error = model.lastError {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(error)
            }

            Spacer()

            Text("First matching rule wins, top to bottom")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .help(
                    "Rules are consulted by priority, lowest first. The first enabled rule "
                        + "that matches a request decides it; nothing after it is consulted."
                )
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    private var countLabel: String {
        let count = model.rules.count
        guard count > 0 else { return "No rules" }

        let rules = count == 1 ? "1 rule" : "\(count) rules"

        return "\(rules), \(model.enabledCount) enabled"
    }
}
