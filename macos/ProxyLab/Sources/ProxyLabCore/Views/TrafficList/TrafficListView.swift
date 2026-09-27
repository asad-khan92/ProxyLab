//  The list of captured exchanges, in the order the query asks for.
//
//  One row per exchange, and never a body: a list is what is scanned, and what
//  is scanned is method, host, path, and how it ended. Everything else is a
//  selection away, fetched for the one exchange that was selected.
//
//  The list is never reloaded to show a new exchange. A `traffic_added` event
//  carries the whole row, and the model inserts it where the current sort puts
//  it, so a row arriving does not move the selection or reset the scroll.
//
//  Rows are identified by exchange id, which is stable for as long as the
//  exchange is kept. That is what lets SwiftUI move a row rather than rebuild
//  the list when one is inserted at the top.

import SwiftUI

/// Every exchange the current query matches, or what is happening instead.
struct TrafficListView: View {
    let model: TrafficInspectorModel
    let pinned: PinnedDomainsStore
    @Binding var selection: String?

    var body: some View {
        VStack(spacing: 0) {
            TrafficFilterBar(model: model)

            switch model.placeholder {
            case .rows:
                list

            case .loading:
                LoadingView(title: "Loading traffic…")

            case let .disconnected(reason):
                PlaceholderView(
                    symbol: "bolt.horizontal.circle",
                    title: "Not connected to the core",
                    detail: reason ?? "The ProxyLab core is not answering.",
                    action: ("Try again", { Task { await model.retryConnection() } })
                )

            case let .failed(reason):
                PlaceholderView(
                    symbol: "exclamationmark.triangle",
                    title: "Traffic could not be loaded",
                    detail: reason,
                    action: ("Try again", { Task { await model.loadFirstPage() } })
                )

            case .noTraffic:
                PlaceholderView(
                    symbol: "arrow.left.arrow.right",
                    title: "No traffic yet",
                    detail: "Requests that go through the proxy appear here as they finish."
                )

            case .noMatches:
                PlaceholderView(
                    symbol: "line.3.horizontal.decrease.circle",
                    title: "No requests match your filters",
                    detail: "The core has traffic, but none of it matches what you are "
                        + "looking for.",
                    action: ("Clear filters", { Task { await model.clearFilters() } })
                )

            case let .noHostTraffic(host):
                // A quiet domain, not a filter that went wrong. The domain
                // stays pinned and the way out is to widen, not to clear.
                PlaceholderView(
                    symbol: "pin",
                    title: "No traffic for \(host)",
                    detail: "Nothing has gone through the proxy for this domain yet. "
                        + "It stays pinned either way.",
                    action: ("Show all traffic", { Task { await model.setHost(nil) } })
                )
            }
        }
        .frame(minWidth: 300)
    }

    private var list: some View {
        List(selection: $selection) {
            ForEach(Array(model.exchanges.enumerated()), id: \.element.id) { index, summary in
                TrafficRow(summary: summary)
                    .tag(summary.id)
                    .contextMenu { contextMenu(for: summary) }
                    .onAppear {
                        // Paging happens as the end comes into view, so
                        // scrolling does not stop at a spinner. The model
                        // refuses a page it is already fetching.
                        guard model.shouldLoadMore(atRow: index) else { return }

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
    }

    @ViewBuilder
    private func contextMenu(for summary: ExchangeSummary) -> some View {
        // The destination the proxy connected to, which is the column the
        // core's host filter compares — see `TrafficDisplay.filterHost`.
        if let host = TrafficDisplay.filterHost(summary) {
            Button(pinned.isPinned(host) ? "Unpin \(host)" : "Pin \(host)") {
                pinned.toggle(host)
            }

            Divider()

            Button("Filter by \(host)") {
                Task { await model.setHost(host) }
            }
        }

        Button("Filter by \(summary.method)") {
            Task { await model.setMethod(summary.method) }
        }

        if let status = summary.status, let group = StatusClass(rawValue: Int(status) / 100) {
            Button("Filter by \(group.label)") {
                Task { await model.setStatus(group) }
            }
        }
    }
}
