//  The hosts the traffic sidebar can browse by.
//
//      loaded pages ──▶ observe host ──▶ Domains
//      traffic_added ──▶ observe host ──▶ Domains, even when the list is filtered
//
//  A pin is a preference. A domain in this list is a destination the proxy has
//  already captured and this window has already seen: a row on a loaded page,
//  or a `traffic_added` event. Selecting one sets `TrafficFilters.host`, which
//  is the same host filter the list has always had. There is no second query
//  and no host index in the core.
//
//  The list is kept when the traffic list is narrowed to one host, so choosing
//  a domain does not hide the others. It is cleared when captured traffic is
//  cleared. A host that has not been on a loaded page, and has not arrived as
//  a live event, is not here yet — paging through All Traffic, or a new
//  request, is what adds it.

import Foundation

/// Destination hosts worth browsing, in the spelling they were first seen.
public struct TrafficDomainCatalog: Equatable, Sendable {
    private var entries: [String] = []

    public init() {}

    /// The hosts, in alphabetical order, compared the way the sidebar reads them.
    public var hosts: [String] {
        entries.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
    }

    /// Whether `host` is already listed, compared the way the core compares hosts.
    public func contains(_ host: String) -> Bool {
        guard let key = PinnedDomainsStore.key(host) else { return false }

        return entries.contains { PinnedDomainsStore.key($0) == key }
    }

    /// The hosts whose names contain `query`.
    ///
    /// An empty query is the whole list. This narrows the sidebar, not the
    /// traffic query: the host filter is still the only thing the list is
    /// asked with.
    public func matching(_ query: String) -> [String] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return hosts }

        return hosts.filter { $0.localizedCaseInsensitiveContains(query) }
    }

    /// Remembers `host` if it is a host and is not listed already.
    ///
    /// The first spelling wins, for the same reason a pin keeps the spelling
    /// that was pinned: the folded key is for comparing, not for reading.
    public mutating func observe(_ host: String?) {
        guard let host else { return }

        let trimmed = host.trimmingCharacters(in: .whitespacesAndNewlines)
        guard PinnedDomainsStore.key(trimmed) != nil, !contains(trimmed) else { return }

        entries.append(trimmed)
    }

    /// Remembers the destination host of each summary.
    public mutating func observe(_ summaries: [ExchangeSummary]) {
        for summary in summaries {
            observe(summary.destination?.host)
        }
    }

    /// Forgets every host. Called when captured traffic is cleared.
    public mutating func clear() {
        entries = []
    }
}
