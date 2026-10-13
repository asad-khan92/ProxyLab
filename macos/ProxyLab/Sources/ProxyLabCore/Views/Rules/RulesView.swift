//  The rules screen: every rule, in the order the core consults them.
//
//      ┌──────────┬─────────────────────┬────────────────────┐
//      │ All     8│ ⏻ Products fixture  │ Products fixture   │
//      │ Map Loc 2│   Map Local      10 │ Match              │
//      │ Map Rem 1│   GET api.example…  │   Host  api.examp… │
//      │ Modify  1│   → products.json   │ Action             │
//      │ Block   1│                     │   products.json    │
//      └──────────┴─────────────────────┴────────────────────┘
//
//  The categories and the search narrow that one list. They do not sort it.
//  Dragging a row, or Move Up and Move Down, writes the priorities that put
//  the rule where it was dropped. The detail pane is the same editor a new
//  rule uses; nothing is saved until Save.

import SwiftUI

/// ProxyLab's rules.
public struct RulesView: View {
    private let model: RulesModel

    @Binding private var section: AppSection
    /// Menus and shortcuts. Nil when a test draws the screen on its own.
    private let commands: AppCommandRouter?

    /// Whether the detail pane is asking which kind of rule to create.
    @State private var choosingKind = false
    /// In the stacked layout, whether the detail is showing instead of the list.
    @State private var showDetail = false
    @FocusState private var searchFocused: Bool

    public init(model: RulesModel, section: Binding<AppSection>) {
        self.init(model: model, section: section, commands: nil)
    }

    init(
        model: RulesModel,
        section: Binding<AppSection>,
        commands: AppCommandRouter?
    ) {
        self.model = model
        self.commands = commands
        _section = section
    }

    public var body: some View {
        GeometryReader { proxy in
            arranged(RulesScreenLayout.forWidth(proxy.size.width))
        }
        .navigationTitle("ProxyLab")
        .navigationSubtitle("Rules")
        .toolbar { toolbar }
        .safeAreaInset(edge: .bottom, spacing: 0) { statusBar }
        .task { await model.load() }
        .background {
            if let commands {
                RulesCommandSignal(commands: commands) {
                    searchFocused = true
                } newRule: {
                    beginNewRule()
                }
            }
        }
        .onChange(of: choosingKind, initial: true) { _, choosing in
            commands?.choosingRuleKind = choosing
        }
        .onChange(of: model.selectedID) { _, id in
            if id != nil {
                showDetail = true
                choosingKind = false
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

    // MARK: - Arrangement

    @ViewBuilder
    private func arranged(_ layout: RulesScreenLayout) -> some View {
        switch layout {
        case .columns:
            HStack(spacing: 0) {
                categoryColumn
                    .frame(width: 176)
                Divider()
                listColumn(showsCategoryMenu: false)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Divider()
                detailColumn(showsBack: false)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .listAndDetail:
            HStack(spacing: 0) {
                listColumn(showsCategoryMenu: true)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                Divider()
                detailColumn(showsBack: false)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .stacked:
            Group {
                if showDetail || model.editor != nil || choosingKind {
                    detailColumn(showsBack: true)
                } else {
                    listColumn(showsCategoryMenu: true)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: - Categories

    private var categoryColumn: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(RulesCategory.allCases) { category in
                categoryButton(category)
            }
            Spacer(minLength: 0)
        }
        .padding(8)
        .frame(maxHeight: .infinity, alignment: .top)
        .accessibilityLabel("Rule types")
    }

    private func categoryButton(_ category: RulesCategory) -> some View {
        let selected = model.category == category

        return Button {
            model.category = category
        } label: {
            HStack(spacing: 6) {
                Text(category.title)
                    .font(.system(size: 12, weight: selected ? .medium : .regular))
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text("\(model.count(in: category))")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected ? Color.accentColor.opacity(0.18) : Color.clear,
                in: RoundedRectangle(cornerRadius: 5)
            )
            .contentShape(RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .foregroundStyle(selected ? Color.accentColor : Color.primary)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityValue("\(model.count(in: category))")
    }

    // MARK: - The list

    private func listColumn(showsCategoryMenu: Bool) -> some View {
        VStack(spacing: 0) {
            if showsCategoryMenu {
                Picker("Show", selection: categoryBinding) {
                    ForEach(RulesCategory.allCases) { category in
                        Text("\(category.title)  \(model.count(in: category))").tag(category)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .padding(.horizontal, 10)
                .padding(.top, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            searchField

            Group {
                switch model.placeholder {
                case .rules:
                    rows
                case .loading:
                    LoadingView(title: "Loading rules…")
                case let .failed(reason):
                    PlaceholderView(
                        symbol: "exclamationmark.triangle",
                        title: "The rules could not be loaded",
                        detail: UserFacingNotice(reason).summary,
                        action: ("Try Again", { Task { await model.load() } })
                    )
                case .noRules, .emptyCategory, .noMatches:
                    if let copy = RulesEmptyCopy.forPlaceholder(model.placeholder) {
                        PlaceholderView(
                            symbol: model.placeholder == .noRules
                                ? "slider.horizontal.3" : "line.3.horizontal.decrease",
                            title: copy.title,
                            detail: copy.detail.map { UserFacingNotice($0).summary },
                            action: model.placeholder == .noRules
                                ? ("New Rule", { beginNewRule() }) : nil
                        )
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            TextField("Search rules", text: searchBinding)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .focused($searchFocused)
                .accessibilityLabel("Search rules")
            if !model.searchText.isEmpty {
                Button {
                    model.searchText = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Clear search")
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(.quaternary.opacity(0.45), in: RoundedRectangle(cornerRadius: 6))
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
    }

    private var rows: some View {
        List(selection: selection) {
            ForEach(model.visibleRules) { rule in
                RuleRow(rule: rule, model: model)
                    .tag(rule.id)
                    .contextMenu { menu(for: rule) }
                    .moveDisabled(!model.busyIDs.isEmpty)
            }
            .onMove { source, destination in
                Task { await model.moveVisible(from: source, to: destination) }
            }
        }
        .listStyle(.inset)
        .accessibilityLabel("Rules")
    }

    @ViewBuilder
    private func menu(for rule: Rule) -> some View {
        Button("Edit") { edit(rule) }
        Button("Duplicate Rule") { duplicate(rule) }
        Button(rule.enabled ? "Disable" : "Enable") {
            Task { await model.setEnabled(!rule.enabled, for: rule) }
        }
        Divider()
        Button("Move Up") { Task { await model.moveUp(rule) } }
            .disabled(!model.canMoveUp(rule))
        Button("Move Down") { Task { await model.moveDown(rule) } }
            .disabled(!model.canMoveDown(rule))
        Divider()
        Button("Delete Rule", role: .destructive) {
            model.selectedID = rule.id
            model.beginDeleting(rule)
        }
    }

    /// The list's selection. A rule the filter is hiding stays selected for
    /// when the filter is cleared, and the list itself shows no selection.
    private var selection: Binding<String?> {
        Binding(
            get: {
                guard let selectedID = model.selectedID,
                    model.visibleRules.contains(where: { $0.id == selectedID })
                else {
                    return nil
                }

                return selectedID
            },
            set: { newValue in
                if let newValue {
                    model.selectedID = newValue
                } else if let selectedID = model.selectedID,
                    model.visibleRules.contains(where: { $0.id == selectedID })
                {
                    // A click that clears a visible selection. A filter that
                    // merely hides the selection also reads as nil, and that
                    // must not drop it.
                    model.selectedID = nil
                }
            }
        )
    }

    private var searchBinding: Binding<String> {
        Binding(get: { model.searchText }, set: { model.searchText = $0 })
    }

    private var categoryBinding: Binding<RulesCategory> {
        Binding(get: { model.category }, set: { model.category = $0 })
    }

    private var deletingBinding: Binding<Bool> {
        Binding(get: { model.deleting != nil }, set: { if !$0 { model.deleting = nil } })
    }

    // MARK: - The detail

    @ViewBuilder
    private func detailColumn(showsBack: Bool) -> some View {
        VStack(spacing: 0) {
            if showsBack, model.editor == nil {
                HStack {
                    Button {
                        showDetail = false
                        choosingKind = false
                    } label: {
                        Label("Rules", systemImage: "chevron.left")
                    }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
                    Spacer()
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                Divider()
            }

            if let editor = model.editor {
                RuleEditorView(editor: editor, model: model, placement: .pane)
            } else if choosingKind {
                kindChooser
            } else {
                RuleDetailView(model: model)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var kindChooser: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("New Rule")
                        .font(.system(size: 15, weight: .semibold))
                    Text("Choose what it does. You can change that in the editor before saving.")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                kindGroup("Decides the request", kinds: RuleActionKind.allCases.filter { !$0.isTransformation })
                kindGroup("Changes the message", kinds: RuleActionKind.allCases.filter(\.isTransformation))

                Button("Cancel") { choosingKind = false }
                    .controlSize(.small)
                    .keyboardShortcut(.cancelAction)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func kindGroup(_ title: String, kinds: [RuleActionKind]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
            ForEach(kinds) { kind in
                Button {
                    choosingKind = false
                    showDetail = true
                    model.beginCreating(kind)
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: kind.symbol)
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                            .frame(width: 16)
                        Text(kind.title)
                            .font(.system(size: 12))
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: - Actions

    private func beginNewRule() {
        guard model.editor == nil else { return }

        choosingKind = true
        showDetail = true
    }

    private func edit(_ rule: Rule) {
        choosingKind = false
        showDetail = true
        model.selectedID = rule.id
        model.beginEditing(rule)
    }

    private func duplicate(_ rule: Rule) {
        choosingKind = false
        showDetail = true
        model.selectedID = rule.id
        model.beginDuplicating(rule)
    }

    private func duplicateSelection() {
        guard let rule = model.selected, model.editor == nil else { return }

        duplicate(rule)
    }

    private func deleteSelection() {
        guard let rule = model.selected, model.editor == nil, !choosingKind else { return }

        model.beginDeleting(rule)
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

            Button(action: beginNewRule) {
                Label("New Rule", systemImage: "plus")
            }
            .disabled(model.editor != nil)
            .help("Write a new rule")

            Button(action: duplicateSelection) {
                Label("Duplicate Rule", systemImage: "plus.square.on.square")
            }
            .disabled(model.selected == nil || model.editor != nil)
            .help("Copy the selected rule into a new draft")

            Button(action: deleteSelection) {
                Label("Delete Rule", systemImage: "trash")
            }
            .disabled(model.selected == nil || model.editor != nil || choosingKind)
            .help("Delete the selected rule")
        }
    }

    private var statusBar: some View {
        HStack(spacing: 10) {
            Text(countLabel)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if let error = model.lastError {
                StatusNotice(message: error)
            }

            Spacer(minLength: 8)

            Text("Top to bottom. Modify rules apply until the first deciding rule.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.tail)
                .help(
                    "Rules are consulted by priority, lowest first, and then by id. "
                        + "A Modify rule changes the message and the walk continues. "
                        + "Allow, Block, Map Local, Map Remote, and Redirect decide, "
                        + "and nothing below that decision is consulted."
                )
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    private var countLabel: String {
        switch model.placeholder {
        case .loading, .noRules:
            return "No rules"
        case .noMatches:
            return "No matching rules"
        case let .emptyCategory(title):
            return title
        case .failed:
            return "Rules unavailable"
        case .rules:
            let shown = model.visibleRules
            let enabled = shown.count { $0.enabled }
            let noun = shown.count == 1 ? "rule" : "rules"
            let searching = !model.searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            if searching { return "\(shown.count) matching, \(enabled) enabled" }
            if model.category != .all {
                return "\(shown.count) \(model.category.title) \(noun), \(enabled) enabled"
            }

            return "\(shown.count) \(noun), \(enabled) enabled"
        }
    }
}
