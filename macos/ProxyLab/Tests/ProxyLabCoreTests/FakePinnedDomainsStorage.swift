//  Somewhere to pin a domain that is not the developer's own preferences.
//
//  A test run must not write into `UserDefaults.standard`: the suite would
//  leave pins behind on the machine that ran it, and two tests would see each
//  other's. This holds the same array in memory and counts the writes, so a
//  test can also assert that pinning something already pinned wrote nothing.

import Foundation

@testable import ProxyLabCore

/// A `PinnedDomainsStorage` that keeps the array in memory.
@MainActor
final class FakePinnedDomainsStorage: PinnedDomainsStorage {
    /// What is stored, which a test may set before the store reads it.
    private(set) var stored: [String]
    /// How many times `save` was called, for the tests about writing.
    private(set) var saveCount = 0

    init(_ stored: [String] = []) {
        self.stored = stored
    }

    func load() -> [String] { stored }

    func save(_ domains: [String]) {
        stored = domains
        saveCount += 1
    }
}
