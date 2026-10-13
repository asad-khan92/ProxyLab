//  The domains a developer keeps coming back to.
//
//      ┌───────────────┐
//      │ All Traffic   │
//      │ Pinned        │
//      │   api.exa…    │──▶ host filter ──▶ the same list_traffic query
//      │   auth.exa…   │
//      └───────────────┘
//
//  A pin is a preference, not traffic. It is a host name a developer chose to
//  keep in front of them, it lives in this application, and the core neither
//  stores it nor hears about it: selecting one sets the host filter the
//  traffic query already has, and everything below that is unchanged.
//
//  # What a pin is identified by
//
//  The destination host, exactly as the traffic model carries it — no port, no
//  scheme, no path, no query, and no brackets around an IPv6 literal, because
//  that is not how `Destination.host` spells one. Every request to
//  `api.example.com` is one pin however many paths it was for.
//
//  Two spellings that differ only in ASCII case are one pin, because that is
//  how the core's host column compares: it is `COLLATE NOCASE`, which folds
//  the ASCII letters and nothing else. So `API.example.com` cannot be pinned
//  alongside `api.example.com`, and a pin made from one matches rows recorded
//  under the other. What is *shown* is the spelling that was pinned, for the
//  same reason the database shows the spelling that was inserted.

import Foundation
import Observation

/// Where pinned domains are kept between launches.
///
/// A protocol so the store can be tested without a test run writing into the
/// developer's own preferences — the same reason ``Clipboard`` is one.
@MainActor
public protocol PinnedDomainsStorage: AnyObject {
    /// The domains that were saved, in the order they were pinned.
    func load() -> [String]
    /// Replaces what was saved.
    func save(_ domains: [String])
}

/// The real one: a single array under one key in `UserDefaults`.
///
/// One key holding one array of host names. Nothing about the traffic itself
/// is written — no URL, no header, no body, no credential — because a pin is a
/// host name and a host name is all that is needed to ask for it again.
@MainActor
public final class UserDefaultsPinnedDomains: PinnedDomainsStorage {
    /// The key the array is written under.
    public static let defaultsKey = "com.proxylab.pinnedDomains"

    private let defaults: UserDefaults
    private let key: String

    public init(
        defaults: UserDefaults = .standard,
        key: String = UserDefaultsPinnedDomains.defaultsKey
    ) {
        self.defaults = defaults
        self.key = key
    }

    public func load() -> [String] {
        // `stringArray(forKey:)` answers `nil` for anything that is not an
        // array of strings, which is what a hand-edited plist leaves behind.
        defaults.stringArray(forKey: key) ?? []
    }

    public func save(_ domains: [String]) {
        if domains.isEmpty {
            defaults.removeObject(forKey: key)
        } else {
            defaults.set(domains, forKey: key)
        }
    }
}

/// Which domains are pinned, and the only thing that changes them.
///
/// Held by the window and read by the sidebar. It knows nothing about traffic
/// queries: selecting a pinned domain is the interface setting the host filter
/// the traffic model already has, so there is one query and one place that
/// narrows it.
@MainActor
@Observable
public final class PinnedDomainsStore {
    /// The pinned domains, in the order they were pinned.
    ///
    /// Pin order rather than alphabetical: the list is short, a developer put
    /// them there deliberately, and a domain that stays where it was put is
    /// easier to reach for than one that moves when its neighbour is renamed.
    public private(set) var domains: [String] = []

    @ObservationIgnored private let storage: any PinnedDomainsStorage

    /// Reads whatever was pinned last time.
    public init(storage: any PinnedDomainsStorage = UserDefaultsPinnedDomains()) {
        self.storage = storage
        domains = Self.tidied(storage.load())
    }

    /// Whether `host` is pinned, compared the way the core compares hosts.
    public func isPinned(_ host: String) -> Bool {
        guard let key = Self.key(host) else { return false }

        return domains.contains { Self.key($0) == key }
    }

    /// Pins `host`, if it is a host and is not pinned already.
    ///
    /// Pinning something already pinned changes nothing and writes nothing:
    /// there is one entry per domain however many times it is asked for.
    public func pin(_ host: String) {
        guard Self.key(host) != nil, !isPinned(host) else { return }

        // The spelling that was pinned, trimmed — not the folded key, which
        // is for comparing and not for reading.
        domains.append(host.trimmingCharacters(in: .whitespacesAndNewlines))
        persist()
    }

    /// Unpins `host`, if it is pinned.
    public func unpin(_ host: String) {
        guard let key = Self.key(host) else { return }

        let remaining = domains.filter { Self.key($0) != key }
        guard remaining.count != domains.count else { return }

        domains = remaining
        persist()
    }

    /// Pins `host` if it is not pinned, and unpins it if it is.
    public func toggle(_ host: String) {
        isPinned(host) ? unpin(host) : pin(host)
    }

    /// Writes the current list. Called only when it actually changed.
    private func persist() {
        storage.save(domains)
    }

    // MARK: - Identity

    /// What two spellings of one domain have in common.
    ///
    /// Trimmed, and lowered the way `COLLATE NOCASE` lowers: the ASCII
    /// letters and nothing else. `nil` for something that is not a host name
    /// at all, which is what an exchange with no destination has.
    ///
    /// Not isolated to the main actor: it reads no preferences, and the
    /// sidebar's selection and the domain list both compare hosts with it.
    nonisolated static func key(_ host: String) -> String? {
        let trimmed = host.trimmingCharacters(in: .whitespacesAndNewlines)

        return trimmed.isEmpty ? nil : trimmed.asciiLowercased()
    }

    /// A stored list made safe to show: no blanks, and no domain twice.
    ///
    /// What is read back may have been written by an older build or edited by
    /// hand, so it is filtered rather than trusted. The first spelling of a
    /// domain wins, which keeps the order the developer pinned them in.
    static func tidied(_ stored: [String]) -> [String] {
        var seen = Set<String>()
        var tidy: [String] = []

        for entry in stored {
            guard let key = key(entry), seen.insert(key).inserted else { continue }

            tidy.append(entry.trimmingCharacters(in: .whitespacesAndNewlines))
        }

        return tidy
    }
}

/// What the sidebar has selected.
///
/// Deliberately not the traffic list's selection: one says which host the list
/// is narrowed to, the other says which exchange the detail pane is showing,
/// and a developer reading one request while filtered to a domain is using
/// both at once.
public enum PinnedDomainSelection: Hashable, Sendable {
    /// No host filter: everything the other filters allow.
    case allTraffic
    /// One host, which the traffic query is narrowed to.
    case domain(String)

    /// The selection a host filter amounts to.
    public init(host: String?) {
        self = host.map(Self.domain) ?? .allTraffic
    }

    /// The host filter this selection amounts to.
    public var host: String? {
        switch self {
        case .allTraffic: nil
        case let .domain(host): host
        }
    }

    /// Two spellings of one host are one selection, because that is how the
    /// host filter compares them. A pin written `API.example.com` and a
    /// captured row written `api.example.com` highlight together.
    public static func == (lhs: PinnedDomainSelection, rhs: PinnedDomainSelection) -> Bool {
        switch (lhs, rhs) {
        case (.allTraffic, .allTraffic):
            return true
        case let (.domain(left), .domain(right)):
            let leftKey = PinnedDomainsStore.key(left)
            let rightKey = PinnedDomainsStore.key(right)
            if let leftKey, let rightKey { return leftKey == rightKey }

            return left == right
        default:
            return false
        }
    }

    public func hash(into hasher: inout Hasher) {
        switch self {
        case .allTraffic:
            hasher.combine(0)
        case let .domain(host):
            hasher.combine(1)
            hasher.combine(PinnedDomainsStore.key(host) ?? host)
        }
    }
}
