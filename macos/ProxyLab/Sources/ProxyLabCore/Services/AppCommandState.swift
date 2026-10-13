//  What the menus and keyboard shortcuts are allowed to do.
//
//  The window already has the models. This reads them. It does not fetch an
//  exchange to decide whether a command is enabled, and it does not grow a
//  second replay, a second delete, or a second capture lifecycle. A command
//  that deletes a rule asks for the confirmation the rules screen already
//  shows. A command that clears traffic asks for the confirmation the traffic
//  screen already shows. Neither one performs the delete itself.

import Foundation

/// A menu command, and the shortcut it owns when it has one.
///
/// The key is the one the menu uses. A second button must not register the
/// same key: SwiftUI would then have two shortcuts for one action.
enum AppCommand: Equatable {
    case toggleCapture
    case find
    case replay
    case copyCurl
    case deleteSelection
    case clearTraffic
    case newRule
    case duplicateRule
    case saveRequest
    case breakpoint
    case applyRule
    case showTraffic
    case showRules
    case showBreaks
    case diagnostics
    case exportSelected
    case exportTraffic
    case importCurl
    case importHar

    /// Whether `state` lets this command run.
    func isEnabled(_ state: AppCommandState) -> Bool {
        switch self {
        case .toggleCapture: state.canToggleCapture
        case .find: state.canFocusSearch
        case .replay: state.canReplay
        case .copyCurl: state.canCopyCurl
        case .deleteSelection: state.canDeleteSelection
        case .clearTraffic: state.canClearTraffic
        case .newRule: state.canNewRule
        case .duplicateRule: state.canDuplicateRule
        case .saveRequest: state.canSaveRequest
        case .breakpoint: state.canBreakpoint
        case .applyRule: state.canApplyRule
        case .showTraffic, .showRules, .showBreaks, .diagnostics: true
        case .exportSelected: state.canExportSelected
        case .exportTraffic: state.canExportTraffic
        case .importCurl: state.canImportCurl
        case .importHar: state.canImportHar
        }
    }

    /// The shortcut, when this command has one that should be registered once.
    var key: AppCommandKey {
        switch self {
        case .toggleCapture: .character("s", shift: true)
        case .find: .character("f")
        case .replay: .character("r")
        case .copyCurl: .character("c", shift: true)
        case .deleteSelection: .delete
        case .clearTraffic: .character("k")
        case .newRule: .character("n")
        case .duplicateRule: .character("d")
        case .showTraffic: .character("1")
        case .showRules: .character("2")
        case .showBreaks: .character("3")
        case .saveRequest, .breakpoint, .applyRule, .diagnostics, .exportSelected, .exportTraffic,
            .importCurl, .importHar:
            .none
        }
    }
}

/// A shortcut the menu registers. Command is held except for Delete, which is
/// the bare key the traffic and rules toolbars already used.
enum AppCommandKey: Equatable {
    case character(Character, shift: Bool = false)
    case delete
    case none
}

/// Whether each command can run, from state the window already has.
struct AppCommandState: Equatable {
    var section: AppSection
    /// Nil while capture is starting or stopping, and neither button is shown.
    var captureAction: CaptureControlAction?
    var trafficConnected: Bool
    var trafficNarrowed: Bool
    /// The selected row, when it is already loaded. Nil does not mean "go and
    /// fetch it": replay stays disabled until the list or the open detail has it.
    var selectedSummary: ExchangeSummary?
    var hasTrafficSelection: Bool
    var rulesEditorOpen: Bool
    var choosingRuleKind: Bool
    var hasSelectedRule: Bool

    var canToggleCapture: Bool { captureAction != nil }

    var captureTitle: String {
        captureAction == .stop ? "Stop Capture" : "Start Capture"
    }

    var captureSymbol: String {
        captureAction == .stop ? "stop.fill" : "play.fill"
    }

    var canFocusSearch: Bool { section == .traffic || section == .rules }

    var canReplay: Bool {
        guard section == .traffic, trafficConnected, let selectedSummary else { return false }

        return TrafficRowMenu(summary: selectedSummary, isPinned: false).canReplay
    }

    var canCopyCurl: Bool {
        guard section == .traffic, trafficConnected, let selectedSummary else { return false }

        return TrafficRowMenu(summary: selectedSummary, isPinned: false).canCopyCurl
    }

    var canDeleteExchange: Bool { section == .traffic && hasTrafficSelection }

    var canDeleteRule: Bool {
        section == .rules && hasSelectedRule && !rulesEditorOpen && !choosingRuleKind
    }

    var canDeleteSelection: Bool {
        switch section {
        case .traffic: canDeleteExchange
        case .rules: canDeleteRule
        case .breakpoints: false
        }
    }

    var deleteTitle: String {
        switch section {
        case .traffic: "Delete Exchange"
        case .rules: "Delete Rule"
        case .breakpoints: "Delete"
        }
    }

    var canClearTraffic: Bool { section == .traffic && trafficConnected }

    var canNewRule: Bool { section == .rules && !rulesEditorOpen && !choosingRuleKind }

    var canDuplicateRule: Bool {
        section == .rules && hasSelectedRule && !rulesEditorOpen && !choosingRuleKind
    }

    var canSaveRequest: Bool { section == .traffic && hasTrafficSelection }

    var canBreakpoint: Bool {
        guard section == .traffic, let selectedSummary else { return false }

        return TrafficRowMenu(summary: selectedSummary, isPinned: false).canBreakpoint
    }

    var canApplyRule: Bool { section == .traffic && selectedSummary != nil }

    var canExportSelected: Bool { section == .traffic && trafficConnected && hasTrafficSelection }

    var canExportTraffic: Bool { section == .traffic && trafficConnected }

    var canImportCurl: Bool { section == .traffic && trafficConnected }

    var canImportHar: Bool { section == .traffic && trafficConnected }

    var exportTrafficTitle: String {
        trafficNarrowed ? "Export Filtered…" : "Export All…"
    }

    static let inactive = AppCommandState(
        section: .traffic,
        captureAction: nil,
        trafficConnected: false,
        trafficNarrowed: false,
        selectedSummary: nil,
        hasTrafficSelection: false,
        rulesEditorOpen: false,
        choosingRuleKind: false,
        hasSelectedRule: false
    )

    /// The selected exchange, from the page or the detail already open.
    ///
    /// A summary that is in neither place is not fetched. The command stays
    /// disabled until the row or the detail has it.
    @MainActor
    static func summary(in traffic: TrafficInspectorModel) -> ExchangeSummary? {
        guard let id = traffic.selectedID else { return nil }
        if let summary = traffic.exchanges.first(where: { $0.id == id }) {
            return summary
        }
        if case let .loaded(exchange) = traffic.detail, exchange.id == id {
            return ExchangeSummary(exchange: exchange)
        }
        return nil
    }
}

/// Routes a command to the model method the buttons already call.
///
/// Clear traffic and delete rule stop at the confirmation those screens
/// already present. The tokens are how a command asks the visible screen to
/// show that confirmation, or to move focus, without a second copy of the
/// dialog.
@MainActor
public final class AppCommandRouter: ObservableObject {
    let traffic: TrafficInspectorModel
    let rules: RulesModel
    let saved: SavedRequestsModel
    private let breakpoints: BreakpointsModel?
    private let performCapture: @MainActor (CaptureControlAction) -> Void

    @Published var section: AppSection = .traffic
    @Published var choosingRuleKind = false
    @Published private(set) var state: AppCommandState = .inactive
    @Published private(set) var focusSearchToken = 0
    @Published private(set) var newRuleToken = 0
    @Published private(set) var confirmClearToken = 0
    @Published private(set) var diagnosticsToken = 0

    public init(
        traffic: TrafficInspectorModel,
        rules: RulesModel,
        saved: SavedRequestsModel,
        breakpoints: BreakpointsModel? = nil,
        performCapture: @escaping @MainActor (CaptureControlAction) -> Void
    ) {
        self.traffic = traffic
        self.rules = rules
        self.saved = saved
        self.breakpoints = breakpoints
        self.performCapture = performCapture
    }

    func replaceState(_ state: AppCommandState) {
        guard self.state != state else { return }

        self.state = state
    }

    func perform(_ command: AppCommand, kind: RuleActionKind? = nil) async {
        guard command.isEnabled(state) else { return }

        switch command {
        case .toggleCapture:
            guard let action = state.captureAction else { return }
            performCapture(action)
        case .find:
            focusSearchToken += 1
        case .replay:
            await traffic.editSelectedAsRequest()
        case .copyCurl:
            await traffic.copySelectedAsCurl()
        case .deleteSelection:
            await deleteSelection()
        case .clearTraffic:
            confirmClearToken += 1
        case .newRule:
            newRuleToken += 1
        case .duplicateRule:
            guard let rule = rules.selected else { return }
            rules.beginDuplicating(rule)
        case .saveRequest:
            guard let id = traffic.selectedID else { return }
            saved.beginSave(
                exchange: id,
                suggestedName: traffic.suggestedSaveName ?? "Saved request"
            )
        case .breakpoint:
            guard let breakpoints, let summary = state.selectedSummary else { return }
            TrafficRowMenu(summary: summary, isPinned: false).beginBreakpoint(using: breakpoints)
        case .applyRule:
            guard let kind, let summary = state.selectedSummary else { return }
            rules.beginTrafficRule(
                TrafficRuleRequest(
                    kind: kind,
                    exchangeID: summary.id,
                    seed: RuleDraftSeed.from(summary: summary, kind: kind)
                )
            )
        case .showTraffic:
            section = .traffic
        case .showRules:
            section = .rules
        case .showBreaks:
            section = .breakpoints
        case .diagnostics:
            diagnosticsToken += 1
        case .exportSelected:
            await traffic.exportSelectedAsHar()
        case .exportTraffic:
            await traffic.exportFilteredAsHar()
        case .importCurl:
            traffic.beginImport()
        case .importHar:
            await traffic.importHar()
        }
    }

    /// Delete Exchange removes the row, which is what the toolbar already does.
    /// Delete Rule asks first, which is what the rules screen already does.
    private func deleteSelection() async {
        switch state.section {
        case .traffic:
            await traffic.deleteSelected()
        case .rules:
            guard let rule = rules.selected else { return }
            rules.beginDeleting(rule)
        case .breakpoints:
            break
        }
    }
}
