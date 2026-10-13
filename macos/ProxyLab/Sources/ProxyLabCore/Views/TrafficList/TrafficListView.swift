//  The list of captured exchanges, in the order the query asks for.
//
//  One row per exchange, and never a body: a list is what is scanned. Method,
//  status, host, path, and duration are always there. Content kind, protocol,
//  and the clock appear when the column is wide enough. Selecting a domain
//  quiets the host, because every visible row is already that host.
//
//  The list is never reloaded to show a new exchange. A `traffic_added` event
//  carries the whole row, and the model inserts it where the current sort puts
//  it, so a row arriving does not move the selection or reset the scroll.
//  Inserts are not animated: a burst should extend the list, not bounce it.
//
//  Rows are identified by exchange id, which is stable for as long as the
//  exchange is kept. That is what lets SwiftUI move a row rather than rebuild
//  the list when one is inserted at the top.

import SwiftUI

/// Every exchange the current query matches, or what is happening instead.
struct TrafficListView: View {
    let model: TrafficInspectorModel
    let breakpoints: BreakpointsModel
    let saved: SavedRequestsModel
    let pinned: PinnedDomainsStore
    @Binding var selection: String?
    var searchFocused: FocusState<Bool>.Binding
    /// Opens the rule chooser for one action on this row.
    var onApplyRule: (RuleActionKind, ExchangeSummary) -> Void = { _, _ in }

    /// Which trailing facts fit. Measured from the list, so every row agrees,
    /// and updated only when the column crosses a threshold.
    @State private var rowWidth: TrafficRowWidth = .regular

    var body: some View {
        VStack(spacing: 0) {
            TrafficFilterBar(model: model, searchFocused: searchFocused)
            if !breakpoints.paused.isEmpty {
                held
                Divider()
            }

            switch model.placeholder {
            case .rows:
                list

            case .loading:
                LoadingView(title: "Loading traffic…")

            default:
                if let copy = TrafficEmptyCopy.content(model.placeholder) {
                    PlaceholderView(
                        symbol: copy.symbol,
                        title: copy.title,
                        detail: copy.detail,
                        action: copy.action.map { action in
                            (action.title, { perform(action) })
                        }
                    )
                }
            }
        }
        .frame(minWidth: 280, maxWidth: .infinity, maxHeight: .infinity)
    }

    /// The empty state's one action. A quiet domain widens the list. A filter
    /// that matched nothing clears the filters. A failed load tries again.
    private func perform(_ action: TrafficEmptyCopy.Action) {
        switch action {
        case .retryConnection:
            Task { await model.retryConnection() }
        case .reload:
            Task { await model.loadFirstPage() }
        case .clearFilters:
            Task { await model.clearFilters() }
        case .showAllTraffic:
            Task { await model.setHost(nil) }
        }
    }

    /// The exchanges being held, above the captured rows.
    ///
    /// They are not part of the traffic query. Choosing one opens the editor
    /// and leaves the others held.
    private var held: some View {
        VStack(spacing: 2) {
            ForEach(breakpoints.paused) { pause in
                Button {
                    breakpoints.selectPause(pause.pauseID)
                } label: {
                    PausedTrafficRow(
                        exchange: pause,
                        selected: breakpoints.showingHold && breakpoints.current?.pauseID == pause.pauseID
                    )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
    }

    /// A domain is selected when the host filter is set. Every visible row
    /// belongs to that host, so the row quiets it.
    private var domainSelected: Bool { model.filters.host != nil }

    private var list: some View {
        List(selection: $selection) {
            ForEach(model.exchanges) { summary in
                TrafficRow(
                    summary: summary,
                    width: rowWidth,
                    domainSelected: domainSelected
                )
                .tag(summary.id)
                .listRowInsets(EdgeInsets(top: 3, leading: 8, bottom: 3, trailing: 8))
                .contextMenu { contextMenu(for: summary) }
                .simultaneousGesture(TapGesture().onEnded {
                    breakpoints.dismissHoldInspector()
                })
                .onAppear {
                    // Paging happens as the end comes into view, so
                    // scrolling does not stop at a spinner. The model
                    // refuses a page it is already fetching. The row is
                    // found by id so the list is not copied to number it.
                    guard
                        let index = model.exchanges.firstIndex(where: { $0.id == summary.id }),
                        model.shouldLoadMore(atRow: index)
                    else { return }

                    Task { await model.loadNextPage() }
                }
            }

            if model.hasMoreTraffic {
                HStack {
                    Spacer()
                    if model.isLoadingMore {
                        ProgressView().controlSize(.small)
                    } else {
                        Button("Load more") { Task { await model.loadNextPage() } }
                            .buttonStyle(.link)
                            .font(.system(size: 11))
                    }
                    Spacer()
                }
                .listRowSeparator(.hidden)
            }
        }
        .listStyle(.inset)
        .environment(\.defaultMinListRowHeight, 24)
        .background {
            GeometryReader { proxy in
                Color.clear.preference(
                    key: TrafficListWidthPreference.self,
                    value: proxy.size.width
                )
            }
        }
        .onPreferenceChange(TrafficListWidthPreference.self) { width in
            // A list reports no width for a frame before it is laid out.
            // Ignoring that keeps the first real measurement from being
            // preceded by the narrow layout.
            guard width >= 200 else { return }

            let fitted = TrafficRowWidth.fitting(width)
            if fitted != rowWidth {
                rowWidth = fitted
            }
        }
        // A new page of rows changes the count. Animating that insert moves
        // every visible row, which is the opposite of what a live capture
        // wants. Resizing across a threshold is the same: the facts appear,
        // the rows do not travel.
        .animation(.none, value: model.exchanges.count)
        .animation(.none, value: rowWidth)
    }

    /// Replay, copy, save, a breakpoint, a rule, then pin and filter, then delete.
    ///
    /// Each item calls the path that already exists for it. A WebSocket
    /// handshake and an event stream leave Replay and Copy as cURL disabled.
    @ViewBuilder
    private func contextMenu(for summary: ExchangeSummary) -> some View {
        let menu = TrafficRowMenu(
            summary: summary,
            isPinned: TrafficDisplay.filterHost(summary).map(pinned.isPinned) ?? false
        )

        Button("Replay Request") {
            Task { await menu.replay(using: model) }
        }
        .disabled(!menu.canReplay)

        Button("Copy as cURL") {
            Task { await menu.copyCurl(using: model) }
        }
        .disabled(!menu.canCopyCurl)

        Button("Save Request") {
            menu.save(using: model, saved: saved)
        }

        Divider()

        Button("Breakpoint…") {
            menu.beginBreakpoint(using: breakpoints)
        }
        .disabled(!menu.canBreakpoint)

        Divider()

        Menu("Apply Rule") {
            ForEach(trafficRuleActions) { kind in
                Button("\(kind.title)…") {
                    onApplyRule(kind, summary)
                }
            }
        }

        if menu.showsNarrowing {
            Divider()

            if let title = menu.domainActionTitle {
                Button(title) {
                    menu.pin(using: pinned)
                }
            }

            if menu.host != nil {
                Button("Filter by Host") {
                    Task { await menu.filterHost(using: model) }
                }
            }

            if menu.canFilterMethod {
                Button("Filter by Method") {
                    Task { await menu.filterMethod(using: model) }
                }
            }

            if menu.statusClass != nil {
                Button("Filter by Status") {
                    Task { await menu.filterStatus(using: model) }
                }
            }
        }

        if menu.canDelete {
            Divider()

            Button("Delete Exchange", role: .destructive) {
                Task { await menu.delete(using: model) }
            }
        }
    }
}

/// The list column's width, so every row picks the same set of facts.
private struct TrafficListWidthPreference: PreferenceKey {
    static let defaultValue: CGFloat = 0

    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}
