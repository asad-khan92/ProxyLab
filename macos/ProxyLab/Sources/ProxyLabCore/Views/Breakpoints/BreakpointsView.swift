//  The breakpoints screen: what is stopped, and the exchange stopped right
//  now.
//
//      ┌──────────────────────────────────────────────────────┐
//      │ ● Connected  [ Traffic | Rules | Breaks ]  + Break ▾ │
//      ├──────────────────┬───────────────────────────────────┤
//      │ ⏻ Request        │ POST https://api.example.com/x    │
//      │   api.example.com│ ┌ Method ──┐ ┌ URL ──────────────┐│
//      │ ⏾ Response       │ │ POST     │ │ https://api.…/x   ││
//      │   api.example.com│ └──────────┘ └───────────────────┘│
//      ├──────────────────┤ Headers                           │
//      │ Throttling    ⏻  │ Accept: application/json          │
//      │ 200 ms / 128 KB/s│ Body                              │
//      │                  │ {"id":1}                          │
//      │                  │        [ Cancel ]  [ Continue ]   │
//      ├──────────────────┴───────────────────────────────────┤
//      │ 2 breakpoints, 1 enabled · 1 held                     │
//      └──────────────────────────────────────────────────────┘
//
//  The sidebar is the configuration — which exchanges are stopped, and how
//  slow the link is pretending to be — and the detail is the one exchange
//  waiting to be decided about. There is no debugger here: no stepping, no
//  call stack, and no history of what an exchange used to be. One exchange is
//  held, its parts are editable, and it either carries on or it does not.
//
//  # Leaving this screen does not free anything
//
//  The app watches for pauses for as long as it is connected, whichever
//  screen is showing, because the core only holds an exchange while somebody
//  is watching. Switching to Traffic does not release what is held; quitting
//  does.

import SwiftUI

/// ProxyLab's breakpoints, and the exchange one is holding.
public struct BreakpointsView: View {
    private let model: BreakpointsModel

    @Binding private var section: AppSection
    @State private var draft = BreakpointDraft()

    public init(model: BreakpointsModel, section: Binding<AppSection>) {
        self.model = model
        _section = section
    }

    public var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
        } detail: {
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("ProxyLab")
        .navigationSubtitle("Breakpoints")
        .toolbar { toolbar }
        .safeAreaInset(edge: .bottom, spacing: 0) { statusBar }
        .sheet(isPresented: $draft.isOpen) {
            BreakpointEditorSheet(draft: $draft, model: model)
        }
    }

    // MARK: - The sidebar

    private var sidebar: some View {
        // The same shape as the traffic sidebar: the list takes the room the
        // throttling controls do not need, and those controls scroll if a
        // short window cannot show them whole.
        BottomPinnedColumn(minimumContentHeight: 80) {
            list
        } footer: {
            VStack(spacing: 0) {
                Divider()
                ThrottleControls(model: model)
            }
        }
    }

    @ViewBuilder
    private var list: some View {
        switch model.state {
        case .loading where model.breakpoints.isEmpty:
            LoadingView(title: "Loading breakpoints…")
        case let .failed(reason) where model.breakpoints.isEmpty:
            PlaceholderView(
                symbol: "exclamationmark.triangle",
                title: "The breakpoints could not be loaded",
                detail: UserFacingNotice(reason).summary,
                action: ("Try Again", { Task { await model.load() } })
            )
        default:
            if model.breakpoints.isEmpty {
                PlaceholderView(
                    symbol: "pause.circle",
                    title: BreakpointEmptyCopy.title,
                    detail: BreakpointEmptyCopy.detail
                )
            } else {
                List {
                    ForEach(model.breakpoints) { breakpoint in
                        BreakpointRow(breakpoint: breakpoint, model: model)
                            .contextMenu {
                                Button(breakpoint.enabled ? "Disable" : "Enable") {
                                    Task {
                                        await model.setEnabled(!breakpoint.enabled, for: breakpoint)
                                    }
                                }
                                Divider()
                                Button("Delete Breakpoint", role: .destructive) {
                                    Task { await model.delete(breakpoint) }
                                }
                            }
                    }
                }
                .listStyle(.inset)
            }
        }
    }

    // MARK: - The detail

    @ViewBuilder
    private var detail: some View {
        if let editor = model.editor {
            PauseEditorView(editor: editor, model: model)
                .id(editor.paused.pauseID)
        } else if !model.watching {
            PlaceholderView(
                symbol: "pause.circle.fill",
                title: "Nothing is being held",
                detail: "ProxyLab is not watching for pauses, so nothing will be stopped.",
                action: ("Watch", { Task { await model.startWatching() } })
            )
        } else {
            PlaceholderView(
                symbol: "pause.circle",
                title: "Nothing is being held",
                detail: model.breakpoints.isEmpty
                    ? "Add a breakpoint, and the next matching exchange will wait here."
                    : "The next matching exchange will wait here."
            )
        }
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
                Label("Reload Breakpoints", systemImage: "arrow.clockwise")
            }
            .help("Ask the core for the breakpoints again")

            Menu {
                ForEach(BreakpointDirection.allCases, id: \.self) { direction in
                    Button {
                        draft.open(direction)
                    } label: {
                        Label(direction.title, systemImage: direction.symbol)
                    }
                }
            } label: {
                Label("New Breakpoint", systemImage: "plus")
            }
            .help("Stop a matching request or response")
        }
    }

    private var statusBar: some View {
        HStack(spacing: 10) {
            Text(countLabel)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if let error = model.lastError {
                StatusNotice(message: error)
            } else if let action = model.lastAction {
                Text(action)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(action)
            }

            Spacer()

            Text(model.watching ? "Watching for pauses" : "Not watching — nothing will pause")
                .font(.system(size: 10))
                .foregroundStyle(model.watching ? .tertiary : .secondary)
                .help(
                    "The core only holds an exchange while ProxyLab is watching. Quitting the "
                        + "app releases anything it was holding."
                )
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    private var countLabel: String {
        let count = model.breakpoints.count
        let breakpoints = switch count {
        case 0: "No breakpoints"
        case 1: "1 breakpoint, \(model.enabledCount) enabled"
        default: "\(count) breakpoints, \(model.enabledCount) enabled"
        }

        guard !model.paused.isEmpty else { return breakpoints }

        return "\(breakpoints) · \(model.paused.count) held"
    }
}

/// One configured breakpoint in the sidebar.
struct BreakpointRow: View {
    let breakpoint: Breakpoint
    let model: BreakpointsModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Toggle(
                "Enabled",
                isOn: Binding(
                    get: { breakpoint.enabled },
                    set: { enabled in
                        Task { await model.setEnabled(enabled, for: breakpoint) }
                    }
                )
            )
            .toggleStyle(.switch)
            .controlSize(.mini)
            .labelsHidden()
            .disabled(model.busyIDs.contains(breakpoint.id))

            VStack(alignment: .leading, spacing: 2) {
                Text(breakpoint.direction.title)
                    .font(.system(size: 12, weight: .medium))
                Text(breakpoint.matcher.summary)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer()
        }
        .padding(.vertical, 2)
        .opacity(breakpoint.enabled ? 1 : 0.55)
    }
}

// MARK: - Naming

extension BreakpointDirection {
    /// What the interface calls this half.
    public var title: String {
        switch self {
        case .request: "Request"
        case .response: "Response"
        case .websocket: "WebSocket"
        }
    }

    /// The symbol beside it.
    public var symbol: String {
        switch self {
        case .request: "arrow.up.circle"
        case .response: "arrow.down.circle"
        case .websocket: "dot.radiowaves.left.and.right"
        }
    }

    /// What it is for, in one line.
    public var explanation: String {
        switch self {
        case .request: "Held before it is sent upstream."
        case .response: "Held after it arrives and before the client is given it."
        case .websocket: "A text message is held before it continues."
        }
    }
}

extension RuleMatcher {
    /// The criteria this narrows on, as one line.
    public var summary: String {
        guard !isAny else { return "Every request" }

        var parts: [String] = []
        if let method { parts.append(method.text.uppercased()) }
        if let host { parts.append(host.text) }
        if let path { parts.append(path.isPrefix ? "\(path.text)*" : path.text) }

        return parts.joined(separator: " ")
    }
}
