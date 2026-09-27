//  The storage that actually ships.
//
//  `PinnedDomainsStoreTests` covers the rules a pin follows against an
//  in-memory double. This covers the one thing a double cannot: that the
//  implementation the app runs really does write to `UserDefaults` and really
//  does read it back.
//
//  It writes into one preference domain of its own — never
//  `UserDefaults.standard` — named for what it is, emptied before and after
//  every test, and reused rather than made unique per run. macOS leaves the
//  (empty) plist file behind when a domain is removed, so a unique name per
//  run would mean a new file in `~/Library/Preferences` every time the suite
//  is run; one fixed name means one file, holding nothing.
//
//  The tests are serialized because they share that one domain.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Pinned domains in preferences", .serialized)
@MainActor
struct PinnedDomainsDefaultsTests {
    /// The one domain this suite ever writes to.
    private static let domain = "com.proxylab.tests.pinned-domains"

    /// Runs `body` against this suite's own preferences, empty to start with
    /// and emptied again afterwards.
    ///
    /// Emptied explicitly rather than in a `deinit`: a deferred removal is one
    /// that may not have happened by the time the process ends, and a suite
    /// that leaves a developer's pins lying about is exactly what this file
    /// exists to avoid.
    private func withOwnPreferences(_ body: (String, UserDefaults) throws -> Void) rethrows {
        let name = Self.domain
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)

        defer {
            defaults.removePersistentDomain(forName: name)
            // Proved, because a leak here is silent and permanent.
            #expect(
                UserDefaults.standard.persistentDomain(forName: name)?.isEmpty ?? true,
                "\(name) was left with something in it"
            )
        }

        try body(name, defaults)
    }

    @Test("nothing saved reads back as nothing pinned")
    func empty() {
        withOwnPreferences { _, defaults in
            let storage = UserDefaultsPinnedDomains(defaults: defaults)

            #expect(storage.load().isEmpty)
            #expect(PinnedDomainsStore(storage: storage).domains.isEmpty)
        }
    }

    @Test("a pin written by one store is read by the next")
    func roundTrip() {
        withOwnPreferences { _, defaults in
            let first = PinnedDomainsStore(storage: UserDefaultsPinnedDomains(defaults: defaults))
            first.pin("api.example.com")
            first.pin("auth.example.com")

            // A fresh store over the same preferences is what a relaunch is.
            let second = PinnedDomainsStore(
                storage: UserDefaultsPinnedDomains(defaults: defaults)
            )

            #expect(second.domains == ["api.example.com", "auth.example.com"])
            #expect(second.isPinned("api.example.com"))
        }
    }

    @Test("unpinning the last domain leaves no key behind")
    func removesTheKeyWhenEmpty() {
        withOwnPreferences { _, defaults in
            let storage = UserDefaultsPinnedDomains(defaults: defaults)
            let store = PinnedDomainsStore(storage: storage)
            store.pin("api.example.com")

            store.unpin("api.example.com")

            #expect(storage.load().isEmpty)
            #expect(defaults.object(forKey: UserDefaultsPinnedDomains.defaultsKey) == nil)
        }
    }

    @Test("a value that is not a list of host names is ignored rather than crashed on")
    func rubbishInPreferences() {
        withOwnPreferences { _, defaults in
            defaults.set(42, forKey: UserDefaultsPinnedDomains.defaultsKey)

            let storage = UserDefaultsPinnedDomains(defaults: defaults)

            #expect(storage.load().isEmpty)
            #expect(PinnedDomainsStore(storage: storage).domains.isEmpty)
        }
    }

    @Test("nothing but host names is written")
    func writesOnlyDomains() {
        withOwnPreferences { name, defaults in
            let store = PinnedDomainsStore(
                storage: UserDefaultsPinnedDomains(defaults: defaults)
            )

            store.pin("api.example.com")

            // A pin is a host name. No URL, no header, no body, no credential,
            // and nothing else under any other key.
            let written = UserDefaults.standard.persistentDomain(forName: name) ?? [:]
            #expect(written.count == 1)
            #expect(
                written[UserDefaultsPinnedDomains.defaultsKey] as? [String] == ["api.example.com"]
            )
        }
    }
}
