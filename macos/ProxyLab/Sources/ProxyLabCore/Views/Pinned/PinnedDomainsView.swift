//  The sidebar: all traffic, and the domains worth coming back to.
//
//      ┌─────────────────┐
//      │ ✓ All Traffic   │
//      │                 │
//      │ FILTERING       │
//      │   staging.ex…  ⊕│   the host filter, not yet pinned
//      │                 │
//      │ PINNED          │
//      │   api.example…  │
//      │   auth.exampl…  │
//      └─────────────────┘
//
//  Selecting a row sets the traffic query's host filter and nothing else, so a
//  search, a method, a status, and a sort all survive moving between domains —
//  and the list that comes back is the same indexed query the core already
//  answers. There is no second way of fetching traffic here.
//
//  # Two selections, and they are not the same one
//
//  This list's selection says which host the traffic is narrowed to. The
//  traffic list's selection says which exchange the detail pane is showing. A
//  developer reading one request while filtered to a domain is using both, so
//  they are separate bindings onto separate state and neither disturbs the
//  other.

import SwiftUI

/// All traffic, the host being filtered on, and the pinned domains.
struct PinnedDomainsView: View {
    let model: TrafficInspectorModel
    let pinned: PinnedDomainsStore

    var body: some View {
        List(selection: selection) {
            Section {
                Label("All Traffic", systemImage: "arrow.left.arrow.right")
                    .tag(PinnedDomainSelection.allTraffic)
                    .accessibilityLabel("All traffic")
                    .accessibilityHint("Show traffic for every host")
            }

            if let host = unpinnedFilterHost {
                Section("Filtering") {
                    UnpinnedHostRow(host: host, pinned: pinned)
                        .tag(PinnedDomainSelection.domain(host))
                }
            }

            Section("Pinned") {
                if pinned.domains.isEmpty {
                    emptyNote
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
        }
        .listStyle(.sidebar)
        .accessibilityLabel("Pinned domains")
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
                Task { await model.setHost(choice?.host) }
            }
        )
    }

    /// The host the list is filtered to, when it is one nobody has pinned.
    ///
    /// Shown so that the sidebar always says what is being looked at — a
    /// filter set from the Host menu would otherwise leave every row
    /// unselected and nothing to explain why — and so there is somewhere
    /// obvious to pin it from.
    private var unpinnedFilterHost: String? {
        guard let host = model.filters.host, !pinned.isPinned(host) else { return nil }

        return host
    }

    private var emptyNote: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("No pinned domains")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
            Text("Pin a domain from a traffic request to access it quickly here.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "No pinned domains. Pin a domain from a traffic request to access it quickly here."
        )
    }
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
                .truncationMode(.head)

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
                .truncationMode(.head)

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
