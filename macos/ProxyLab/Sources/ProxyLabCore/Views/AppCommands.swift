//  The menu bar, and the shortcuts that go with it.
//
//  Every item calls ``AppCommandRouter``, which calls the same model method
//  the toolbar or the context menu already calls. Nothing here decides a
//  second time whether a replay is legal, and nothing here deletes without
//  the confirmation that action already uses.
//
//  Saved requests are not a screen, so they have no ⌘4. A saved request is
//  made from a captured exchange with Save Request, so ⌘N is New Rule and
//  only while Rules is showing.

import SwiftUI

/// Registers the shortcuts in ``AppCommand/key`` once.
extension View {
    @ViewBuilder
    func commandKey(_ command: AppCommand) -> some View {
        switch command.key {
        case .character(let character, false):
            keyboardShortcut(KeyEquivalent(character))
        case .character(let character, true):
            keyboardShortcut(KeyEquivalent(character), modifiers: [.command, .shift])
        case .delete:
            keyboardShortcut(.delete, modifiers: [])
        case .none:
            self
        }
    }
}

/// The app menu. Empty until the window has a router.
public struct ProxyLabCommands: Commands {
    var router: AppCommandRouter?

    public init(router: AppCommandRouter?) {
        self.router = router
    }

    public var body: some Commands {
        if let router {
            AppCommandMenus(router: router)
        }
    }
}

/// File, Edit, Capture, Traffic, Rules, View, and Help.
private struct AppCommandMenus: Commands {
    @ObservedObject var router: AppCommandRouter

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Rule") { Task { await router.perform(.newRule) } }
                .commandKey(.newRule)
                .disabled(!router.state.canNewRule)
        }

        CommandGroup(after: .newItem) {
            Button("Save Request") { Task { await router.perform(.saveRequest) } }
                .disabled(!router.state.canSaveRequest)

            Button("Import cURL…") { Task { await router.perform(.importCurl) } }
                .disabled(!router.state.canImportCurl)

            Divider()

            Button("Export Selected…") { Task { await router.perform(.exportSelected) } }
                .disabled(!router.state.canExportSelected)

            Button(router.state.exportTrafficTitle) { Task { await router.perform(.exportTraffic) } }
                .disabled(!router.state.canExportTraffic)

            Button("Import HAR…") { Task { await router.perform(.importHar) } }
                .disabled(!router.state.canImportHar)
        }

        CommandGroup(after: .textEditing) {
            Button("Find") { Task { await router.perform(.find) } }
                .commandKey(.find)
                .disabled(!router.state.canFocusSearch)
        }

        CommandGroup(after: .pasteboard) {
            Button(router.state.deleteTitle) { Task { await router.perform(.deleteSelection) } }
                .commandKey(.deleteSelection)
                .disabled(!router.state.canDeleteSelection)
        }

        CommandMenu("Capture") {
            Button {
                Task { await router.perform(.toggleCapture) }
            } label: {
                Label(router.state.captureTitle, systemImage: router.state.captureSymbol)
            }
            .commandKey(.toggleCapture)
            .disabled(!router.state.canToggleCapture)

            Divider()

            Button("Clear Traffic") { Task { await router.perform(.clearTraffic) } }
                .commandKey(.clearTraffic)
                .disabled(!router.state.canClearTraffic)
        }

        CommandMenu("Traffic") {
            Button("Replay Request") { Task { await router.perform(.replay) } }
                .commandKey(.replay)
                .disabled(!router.state.canReplay)

            Button("Copy as cURL") { Task { await router.perform(.copyCurl) } }
                .commandKey(.copyCurl)
                .disabled(!router.state.canCopyCurl)

            Button("Breakpoint…") { Task { await router.perform(.breakpoint) } }
                .disabled(!router.state.canBreakpoint)

            Menu("Apply Rule") {
                ForEach(trafficRuleActions) { kind in
                    Button("\(kind.title)…") { Task { await router.perform(.applyRule, kind: kind) } }
                        .disabled(!router.state.canApplyRule)
                }
            }
            .disabled(!router.state.canApplyRule)

            Divider()

            Button("Delete Exchange") { Task { await router.perform(.deleteSelection) } }
                .disabled(!router.state.canDeleteExchange)
        }

        CommandMenu("Rules") {
            Button("New Rule") { Task { await router.perform(.newRule) } }
                .disabled(!router.state.canNewRule)

            Button("Duplicate Rule") { Task { await router.perform(.duplicateRule) } }
                .commandKey(.duplicateRule)
                .disabled(!router.state.canDuplicateRule)

            Button("Delete Rule") { Task { await router.perform(.deleteSelection) } }
                .disabled(!router.state.canDeleteRule)
        }

        CommandGroup(after: .sidebar) {
            sectionButton("Traffic", command: .showTraffic, section: .traffic)
            sectionButton("Rules", command: .showRules, section: .rules)
            sectionButton("Breaks", command: .showBreaks, section: .breakpoints)
        }

        CommandGroup(after: .help) {
            Button("Diagnostics") { Task { await router.perform(.diagnostics) } }
        }
    }

    private func sectionButton(_ title: String, command: AppCommand, section: AppSection) -> some View {
        Button {
            Task { await router.perform(command) }
        } label: {
            if router.section == section {
                Label(title, systemImage: "checkmark")
            } else {
                Text(title)
            }
        }
        .commandKey(command)
    }
}

/// Tells the traffic screen to focus its search, or to ask before clearing.
struct TrafficCommandSignal: View {
    @ObservedObject var commands: AppCommandRouter
    var focus: () -> Void
    var clear: () -> Void

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
            .onChange(of: commands.focusSearchToken) { _, token in
                if token > 0 { focus() }
            }
            .onChange(of: commands.confirmClearToken) { _, token in
                if token > 0 { clear() }
            }
    }
}

/// Tells the rules screen to focus its search, or to open a new rule.
struct RulesCommandSignal: View {
    @ObservedObject var commands: AppCommandRouter
    var focus: () -> Void
    var newRule: () -> Void

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
            .onChange(of: commands.focusSearchToken) { _, token in
                if token > 0 { focus() }
            }
            .onChange(of: commands.newRuleToken) { _, token in
                if token > 0 { newRule() }
            }
    }
}

/// Writes the command state from the models the window is already showing.
///
/// One view, not a row. It reads the loaded page and the open detail, and it
/// does not ask the core for anything.
struct AppCommandRefresher: View {
    @ObservedObject var commands: AppCommandRouter
    let traffic: TrafficInspectorModel
    let rules: RulesModel
    let lifecycle: CaptureLifecycle

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .accessibilityHidden(true)
            .onChange(of: snapshot, initial: true) { _, new in
                commands.replaceState(new)
            }
    }

    private var snapshot: AppCommandState {
        AppCommandState(
            section: commands.section,
            captureAction: lifecycle.presentation.action,
            trafficConnected: traffic.connection == .connected,
            trafficNarrowed: traffic.filters.isNarrowed,
            selectedSummary: AppCommandState.summary(in: traffic),
            hasTrafficSelection: traffic.selectedID != nil,
            rulesEditorOpen: rules.editor != nil,
            choosingRuleKind: commands.choosingRuleKind,
            hasSelectedRule: rules.selected != nil
        )
    }
}
