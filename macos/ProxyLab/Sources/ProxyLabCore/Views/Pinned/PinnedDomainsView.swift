//  The sidebar: all traffic, the domains worth coming back to, and every
//  domain the captured traffic has named.
//
//      ┌─────────────────┐
//      │ ✓ All Traffic   │
//      │                 │
//      │ PINNED          │
//      │   api.example…  │
//      │                 │
//      │ DOMAINS       ▾ │
//      │   api.example…  │
//      │   cdn.example…  │
//      └─────────────────┘
//
//  Selecting a row sets the traffic query's host filter and nothing else, so a
//  search, a method, a status, and a sort all survive moving between domains —
//  and the list that comes back is the same indexed query the core already
//  answers. There is no second way of fetching traffic here.
//
//  The list scrolls. All Traffic, the pins, and the domains are one sidebar
//  list, so a long domain list moves inside the column rather than growing
//  the window.
//
//  # Two selections, and they are not the same one
//
//  This list's selection says which host the traffic is narrowed to. The
//  traffic list's selection says which exchange the detail pane is showing. A
//  developer reading one request while filtered to a domain is using both, so
//  they are separate bindings onto separate state and neither disturbs the
//  other.

import AppKit
import SwiftUI

/// All traffic, pinned domains, and every captured domain.
struct PinnedDomainsView: View {
    let model: TrafficInspectorModel
    let pinned: PinnedDomainsStore

    /// The Domains section starts open. Collapsing it hides the list; it does
    /// not change which host the traffic is filtered to.
    @State private var domainsExpanded = true
    /// Narrows the domain list. Not a traffic filter.
    @State private var domainQuery = ""

    var body: some View {
        List(selection: selection) {
            Section {
                Label("All Traffic", systemImage: "arrow.left.arrow.right")
                    .tag(PinnedDomainSelection.allTraffic)
                    .accessibilityLabel("All traffic")
                    .accessibilityHint("Show traffic for every host")
            }

            Section("Pinned") {
                if pinned.domains.isEmpty {
                    emptyPins
                } else {
                    ForEach(pinned.domains, id: \.self) { domain in
                        PinnedDomainRow(domain: domain, pinned: pinned)
                            .tag(PinnedDomainSelection.domain(domain))
                            .contextMenu {
                                Button("Unpin \(domain)") { pinned.unpin(domain) }
                            }
                    }
                }
            }

            Section("Domains", isExpanded: $domainsExpanded) {
                if model.knownHosts.count > 8 {
                    TextField("Filter domains", text: $domainQuery)
                        .textFieldStyle(.roundedBorder)
                        .controlSize(.small)
                        .selectionDisabled()
                        .accessibilityLabel("Filter domains")
                }

                if capturedDomains.isEmpty {
                    emptyDomains
                        .selectionDisabled()
                } else {
                    ForEach(capturedDomains) { domain in
                        CapturedDomainRow(domain: domain.host, pinned: pinned)
                            .tag(PinnedDomainSelection.domain(domain.host))
                            .contextMenu {
                                Button(pinned.isPinned(domain.host) ? "Unpin \(domain.host)" : "Pin \(domain.host)") {
                                    pinned.toggle(domain.host)
                                }
                            }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityLabel("Domains")
    }

    /// What the sidebar has selected, which is the host filter by another name.
    ///
    /// Reading it from the model rather than holding a copy is what keeps the
    /// sidebar honest when the host is changed from somewhere else — the Host
    /// menu, or a row's context menu.
    private var selection: Binding<PinnedDomainSelection?> {
        Binding(
            get: { PinnedDomainSelection(host: model.filters.host) },
            set: { choice in
                // The list also writes this while domain rows are inserted.
                // That write is not a click or a key, and applying it narrows
                // the traffic list to a host nobody chose. A tap gesture on
                // the row does the same thing when the row appears, so the
                // click has to come through this setter.
                let host = choice?.host
                guard host != model.filters.host else { return }
                guard SidebarHostSelection.shouldCommit(NSApp.currentEvent?.type) else { return }

                Task { await model.setHost(host) }
            }
        )
    }

    /// Captured hosts, after the sidebar's own filter, each with an identity
    /// that is not a pin's. The same host can sit in both sections.
    ///
    /// ``TrafficInspectorModel/knownHosts`` rather than the catalog alone, so a
    /// host the list is filtered to still appears when traffic has not named
    /// it yet.
    private var capturedDomains: [SidebarDomain] {
        let query = domainQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        let hosts = query.isEmpty
            ? model.knownHosts
            : model.knownHosts.filter { $0.localizedCaseInsensitiveContains(query) }

        return hosts.map { host in
            SidebarDomain(host: host, id: "captured:\(PinnedDomainsStore.key(host) ?? host)")
        }
    }

    private var emptyPins: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("No pinned domains")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
            Text("Pin a domain to keep it here.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("No pinned domains. Pin a domain to keep it here.")
    }

    private var emptyDomains: some View {
        Text(domainQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? DomainEmptyCopy.noneCaptured
            : DomainEmptyCopy.noMatches)
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, 4)
    }
}

/// One captured host in the Domains section.
private struct SidebarDomain: Identifiable, Hashable {
    var host: String
    var id: String
}

/// One pinned domain, with the way to unpin it.
///
/// The button appears on hover so a short list of domains reads as a list of
/// domains rather than a list of buttons; the context menu is always there, so
/// unpinning never depends on a pointer being in the right place.
struct PinnedDomainRow: View {
    let domain: String
    let pinned: PinnedDomainsStore

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "pin.fill")
                .font(.system(size: 9))
                .foregroundStyle(.tertiary)

            Text(domain)
                .font(.system(size: 12))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 4)

            if hovering {
                Button {
                    pinned.unpin(domain)
                } label: {
                    Image(systemName: "pin.slash")
                        .font(.system(size: 10))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Unpin \(domain)")
                .accessibilityLabel("Unpin \(domain)")
            }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .accessibilityLabel("Pinned domain \(domain)")
        .accessibilityHint("Show traffic for \(domain)")
    }
}

/// One captured domain, with the way to pin it.
struct CapturedDomainRow: View {
    let domain: String
    let pinned: PinnedDomainsStore

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: pinned.isPinned(domain) ? "pin.fill" : "globe")
                .font(.system(size: 9))
                .foregroundStyle(.tertiary)

            Text(domain)
                .font(.system(size: 12))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 4)

            if hovering {
                Button {
                    pinned.toggle(domain)
                } label: {
                    Image(systemName: pinned.isPinned(domain) ? "pin.slash" : "pin")
                        .font(.system(size: 10))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help(pinned.isPinned(domain) ? "Unpin \(domain)" : "Pin \(domain)")
                .accessibilityLabel(pinned.isPinned(domain) ? "Unpin \(domain)" : "Pin \(domain)")
            }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .accessibilityLabel("Domain \(domain)")
        .accessibilityHint("Show traffic for \(domain)")
    }
}

/// The host the list is filtered to, which nobody has pinned.
struct UnpinnedHostRow: View {
    let host: String
    let pinned: PinnedDomainsStore

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "line.3.horizontal.decrease.circle")
                .font(.system(size: 9))
                .foregroundStyle(.tertiary)

            Text(host)
                .font(.system(size: 12))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 4)

            Button {
                pinned.pin(host)
            } label: {
                Image(systemName: "pin")
                    .font(.system(size: 10))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Pin \(host)")
            .accessibilityLabel("Pin \(host)")
        }
        .contentShape(Rectangle())
        .accessibilityLabel("Filtering by \(host), not pinned")
        .accessibilityHint("Show traffic for \(host)")
    }
}

/// Which list-selection writes become a host filter.
///
/// A click or a key is the person choosing a row. `List` inside a
/// `NavigationSplitView` also writes the selection while rows are inserted,
/// and that write carries no event. Treating it as a choice narrows traffic
/// to a host nobody picked.
enum SidebarHostSelection {
    static func shouldCommit(_ type: NSEvent.EventType?) -> Bool {
        switch type {
        case .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .keyDown, .keyUp:
            true
        default:
            false
        }
    }
}
