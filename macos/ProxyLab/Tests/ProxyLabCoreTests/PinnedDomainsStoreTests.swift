//  What a pin is, and what it survives.
//
//  A pin is a host name in this application's preferences. These tests are
//  about the three things that makes it: it is one entry per domain however
//  many times it is pinned, it comes back in the order it was pinned, and it
//  outlives the store that wrote it. Nothing here touches the real
//  `UserDefaults` — see `FakePinnedDomainsStorage`.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Pinned domains")
@MainActor
struct PinnedDomainsStoreTests {
    private func store(_ stored: [String] = []) -> (PinnedDomainsStore, FakePinnedDomainsStorage) {
        let storage = FakePinnedDomainsStorage(stored)

        return (PinnedDomainsStore(storage: storage), storage)
    }

    // MARK: - The empty state

    @Test("a store with nothing saved has nothing pinned")
    func startsEmpty() {
        let (pinned, _) = store()

        #expect(pinned.domains.isEmpty)
        #expect(!pinned.isPinned("api.example.com"))
    }

    // MARK: - Pinning

    @Test("a pinned domain is held and written")
    func pinning() {
        let (pinned, storage) = store()

        pinned.pin("api.example.com")

        #expect(pinned.domains == ["api.example.com"])
        #expect(pinned.isPinned("api.example.com"))
        #expect(storage.stored == ["api.example.com"])
        #expect(storage.saveCount == 1)
    }

    @Test("pinning the same domain twice does not pin it twice")
    func duplicatePin() {
        let (pinned, storage) = store()

        pinned.pin("api.example.com")
        pinned.pin("api.example.com")
        pinned.pin("api.example.com")

        #expect(pinned.domains == ["api.example.com"])
        // And the second and third asked for nothing to be written.
        #expect(storage.saveCount == 1)
    }

    @Test("a domain differing only in case is the same pin")
    func caseFolding() {
        let (pinned, _) = store()

        pinned.pin("api.example.com")
        pinned.pin("API.Example.COM")

        // One entry, spelled the way it was first pinned — which is what the
        // core's own `COLLATE NOCASE` column does with the value it stored.
        #expect(pinned.domains == ["api.example.com"])
        #expect(pinned.isPinned("API.EXAMPLE.COM"))
        #expect(pinned.isPinned("Api.Example.Com"))
    }

    @Test("surrounding whitespace is not part of a domain")
    func trimming() {
        let (pinned, _) = store()

        pinned.pin("  api.example.com \n")

        #expect(pinned.domains == ["api.example.com"])
        #expect(pinned.isPinned("api.example.com"))
    }

    @Test("something that is not a host is not pinned")
    func blanks() {
        let (pinned, storage) = store()

        pinned.pin("")
        pinned.pin("   ")

        #expect(pinned.domains.isEmpty)
        #expect(storage.saveCount == 0)
    }

    @Test("subdomains are domains of their own")
    func subdomains() {
        let (pinned, _) = store()

        pinned.pin("api.example.com")
        pinned.pin("auth.api.example.com")

        #expect(pinned.domains == ["api.example.com", "auth.api.example.com"])
        #expect(pinned.isPinned("auth.api.example.com"))
        // Pinning a subdomain does not pin its parent, or the other way round.
        #expect(!pinned.isPinned("example.com"))
    }

    @Test("an address is a domain like any other")
    func addresses() {
        let (pinned, _) = store()

        // IPv6 without brackets, which is how `Destination.host` spells one.
        pinned.pin("127.0.0.1")
        pinned.pin("::1")

        #expect(pinned.domains == ["127.0.0.1", "::1"])
        #expect(pinned.isPinned("127.0.0.1"))
        #expect(pinned.isPinned("::1"))
        #expect(!pinned.isPinned("[::1]"))
    }

    // MARK: - Unpinning

    @Test("an unpinned domain is gone and the rest are not")
    func unpinning() {
        let (pinned, storage) = store()
        pinned.pin("api.example.com")
        pinned.pin("auth.example.com")
        pinned.pin("cdn.example.com")

        pinned.unpin("auth.example.com")

        #expect(pinned.domains == ["api.example.com", "cdn.example.com"])
        #expect(!pinned.isPinned("auth.example.com"))
        #expect(storage.stored == ["api.example.com", "cdn.example.com"])
    }

    @Test("unpinning a domain that is not pinned changes nothing")
    func unpinningNothing() {
        let (pinned, storage) = store()
        pinned.pin("api.example.com")
        let writes = storage.saveCount

        pinned.unpin("other.example.com")

        #expect(pinned.domains == ["api.example.com"])
        #expect(storage.saveCount == writes)
    }

    @Test("unpinning folds case the same way pinning does")
    func unpinningCase() {
        let (pinned, _) = store()
        pinned.pin("api.example.com")

        pinned.unpin("API.EXAMPLE.COM")

        #expect(pinned.domains.isEmpty)
    }

    @Test("toggling pins what is not pinned and unpins what is")
    func toggling() {
        let (pinned, _) = store()

        pinned.toggle("api.example.com")
        #expect(pinned.isPinned("api.example.com"))

        pinned.toggle("api.example.com")
        #expect(!pinned.isPinned("api.example.com"))
    }

    // MARK: - Ordering and persistence

    @Test("domains come back in the order they were pinned")
    func pinOrder() {
        let (pinned, _) = store()

        pinned.pin("zeta.example.com")
        pinned.pin("alpha.example.com")
        pinned.pin("middle.example.com")

        // Pin order, deliberately not alphabetical: a domain stays where it
        // was put.
        #expect(pinned.domains == ["zeta.example.com", "alpha.example.com", "middle.example.com"])
    }

    @Test("a relaunch comes back with the same domains in the same order")
    func survivesRelaunch() {
        let storage = FakePinnedDomainsStorage()
        let first = PinnedDomainsStore(storage: storage)
        first.pin("zeta.example.com")
        first.pin("alpha.example.com")

        // A second store over the same storage is what a relaunch is.
        let second = PinnedDomainsStore(storage: storage)

        #expect(second.domains == ["zeta.example.com", "alpha.example.com"])
        #expect(second.isPinned("alpha.example.com"))
    }

    @Test("an unpin survives a relaunch too")
    func unpinSurvivesRelaunch() {
        let storage = FakePinnedDomainsStorage()
        let first = PinnedDomainsStore(storage: storage)
        first.pin("api.example.com")
        first.pin("cdn.example.com")
        first.unpin("api.example.com")

        let second = PinnedDomainsStore(storage: storage)

        #expect(second.domains == ["cdn.example.com"])
    }

    @Test("a stored list that was edited by hand is tidied rather than trusted")
    func tidiesStoredList() {
        // Blanks, whitespace, and the same domain twice in two spellings.
        let (pinned, _) = store(["", "  api.example.com  ", "API.EXAMPLE.COM", "   ", "cdn.example.com"])

        #expect(pinned.domains == ["api.example.com", "cdn.example.com"])
        #expect(pinned.isPinned("cdn.example.com"))
    }

    @Test("reading pinned domains writes nothing")
    func readingDoesNotWrite() {
        let (pinned, storage) = store(["api.example.com"])

        _ = pinned.domains
        _ = pinned.isPinned("api.example.com")
        _ = pinned.isPinned("other.example.com")

        #expect(storage.saveCount == 0)
    }

    // MARK: - What the sidebar selects

    @Test("a selection is the host filter by another name")
    func selection() {
        #expect(PinnedDomainSelection(host: nil) == .allTraffic)
        #expect(PinnedDomainSelection(host: "api.example.com") == .domain("api.example.com"))
        #expect(PinnedDomainSelection.allTraffic.host == nil)
        #expect(PinnedDomainSelection.domain("api.example.com").host == "api.example.com")
        #expect(PinnedDomainSelection.domain("API.example.com") == .domain("api.example.com"))
    }
}
