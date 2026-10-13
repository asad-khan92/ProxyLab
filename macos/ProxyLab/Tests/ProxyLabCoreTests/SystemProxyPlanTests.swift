//  Which service is configured, what is saved, and what Stop is allowed to
//  write back. No system preferences are opened here.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("System proxy ownership")
struct SystemProxyPlanTests {
    private let ethernet = NetworkServiceSnapshot(
        id: "ethernet-id",
        name: "Ethernet",
        interfaceName: "en7",
        enabled: true,
        isPrimary: true,
        settings: .direct
    )
    private let wifi = NetworkServiceSnapshot(
        id: "wifi-id",
        name: "Wi-Fi",
        interfaceName: "en0",
        enabled: true,
        isPrimary: false,
        settings: SystemProxySettings(
            http: ProxyProtocolSetting(enabled: true, host: "proxy.example", port: 8080),
            https: ProxyProtocolSetting(enabled: true, host: "proxy.example", port: 8080)
        )
    )
    private let usb = NetworkServiceSnapshot(
        id: "usb-id",
        name: "USB 10/100/1000 LAN",
        interfaceName: "en8",
        enabled: true,
        isPrimary: false,
        settings: .direct
    )

    @Test("the primary service is the one whose id the system named, not the one called Wi-Fi")
    func primaryIsNotChosenByName() {
        let marked = SystemProxyPlan.markPrimary(
            [wifi, ethernet],
            primaryID: ethernet.id
        )

        #expect(SystemProxyPlan.primary(in: marked)?.id == ethernet.id)
        #expect(SystemProxyPlan.primary(in: marked)?.name == "Ethernet")
        #expect(marked.first { $0.name == "Wi-Fi" }?.isPrimary == false)
    }

    @Test("a missing primary id marks no service, including one named Wi-Fi")
    func noPrimaryIsNotWiFi() {
        let marked = SystemProxyPlan.markPrimary([wifi], primaryID: nil)

        #expect(SystemProxyPlan.primary(in: marked) == nil)
        #expect(marked.allSatisfy { !$0.isPrimary })
    }

    @Test("start saves the previous proxy and writes only the primary service")
    func claimSavesThePreviousSettings() {
        let plan = SystemProxyPlan.claim(record: nil, services: [wifi, ethernet], port: 9_090)

        #expect(plan.failure == nil)
        #expect(plan.writes.map(\.serviceID) == [ethernet.id])
        #expect(plan.writes.first?.settings == SystemProxyPlan.owned(port: 9_090))
        #expect(plan.record?.services.map(\.id) == [ethernet.id])
        #expect(plan.record?.services.first?.previous == .direct)
        #expect(plan.record?.services.contains { $0.name == "Wi-Fi" } == false)
    }

    @Test("a later start keeps the saved proxy when the service already points at ProxyLab")
    func claimDoesNotRecordItsOwnProxyAsPrevious() {
        let first = SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090)
        var ownedEthernet = ethernet
        ownedEthernet.settings = SystemProxyPlan.owned(port: 9_090)

        let second = SystemProxyPlan.claim(record: first.record, services: [ownedEthernet], port: 9_090)

        #expect(second.writes.isEmpty)
        #expect(second.record?.services.first?.previous == .direct)
    }

    @Test("a newly active service is saved on its own and does not replace another service's previous proxy")
    func adoptKeepsEachService() {
        let claimed = SystemProxyPlan.claim(record: nil, services: [ethernet, wifi], port: 9_090)
        let record = try! #require(claimed.record)
        var nowUSB = usb
        nowUSB.isPrimary = true
        var stillEthernet = ethernet
        stillEthernet.isPrimary = false
        stillEthernet.settings = SystemProxyPlan.owned(port: 9_090)
        var stillWiFi = wifi
        stillWiFi.isPrimary = false

        let adopted = SystemProxyPlan.adopt(
            record: record,
            services: [stillWiFi, stillEthernet, nowUSB]
        )

        #expect(adopted.writes.map(\.serviceID) == [usb.id])
        #expect(adopted.ownershipLost.isEmpty)
        #expect(adopted.record.services.map(\.id) == [ethernet.id, usb.id])
        #expect(adopted.record.stored(ethernet.id)?.previous == .direct)
        #expect(adopted.record.stored(usb.id)?.previous == .direct)

        var ownedUSB = nowUSB
        ownedUSB.settings = SystemProxyPlan.owned(port: 9_090)
        let restored = SystemProxyPlan.restore(
            record: adopted.record,
            services: [stillWiFi, stillEthernet, ownedUSB]
        )
        #expect(restored.writes.map(\.serviceName).sorted() == ["Ethernet", "USB 10/100/1000 LAN"])
        #expect(restored.writes.allSatisfy { $0.settings == .direct })
        #expect(restored.leftInPlace.isEmpty)
    }

    @Test("stop does not write a service whose proxy changed after ProxyLab set it")
    func restoreLeavesAnExternalChange() {
        let claimed = SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090)
        let record = try! #require(claimed.record)
        var changed = ethernet
        changed.settings = wifi.settings

        let restored = SystemProxyPlan.restore(record: record, services: [changed])

        #expect(restored.writes.isEmpty)
        #expect(restored.leftInPlace == ["Ethernet"])
        let message = SystemProxyPlan.externalChangeMessage(restored.leftInPlace)
        #expect(message?.contains("Ethernet") == true)
        #expect(message?.contains("left that setting in place") == true)
    }

    @Test("a service that still has the saved settings is not described as an external change")
    func unrestoredClaimIsNotAnExternalChange() {
        let claimed = SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090)
        let record = try! #require(claimed.record)

        let restored = SystemProxyPlan.restore(record: record, services: [ethernet])

        #expect(restored.writes.isEmpty)
        #expect(restored.leftInPlace.isEmpty)
    }

    @Test("an active service that no longer has ProxyLab's proxy is not overwritten")
    func adoptDoesNotOverwriteAnExternalChange() {
        let claimed = SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090)
        let record = try! #require(claimed.record)
        var changed = ethernet
        changed.settings = SystemProxySettings(
            http: ProxyProtocolSetting(enabled: true, host: "10.1.2.3", port: 8888),
            https: ProxyProtocolSetting(enabled: true, host: "10.1.2.3", port: 8888)
        )

        let adopted = SystemProxyPlan.adopt(record: record, services: [changed])

        #expect(adopted.writes.isEmpty)
        #expect(adopted.ownershipLost == ["Ethernet"])
        #expect(adopted.record.stored(ethernet.id)?.previous == .direct)
    }

    @Test("the ownership record round-trips and does not survive removal")
    func recordFile() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("proxylab-proxy-\(UUID().uuidString)")
            .appendingPathComponent("system-proxy-ownership.json")
        let file = SystemProxyRecordFile(url: url)
        defer { file.remove() }
        let record = SystemProxyRecord(
            version: 1,
            host: "127.0.0.1",
            port: 9_090,
            services: [StoredProxyService(id: ethernet.id, name: ethernet.name, previous: wifi.settings)]
        )

        try file.save(record)

        #expect(file.load() == record)
        let mode = try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int
        #expect(mode == 0o600)
        file.remove()
        #expect(file.load() == nil)
    }

    @Test("a service that already points at ProxyLab is not saved as its own baseline")
    func claimRefusesAnOwnedProxyWithNoBaseline() {
        var owned = ethernet
        owned.settings = SystemProxyPlan.owned(port: 9_090)

        let plan = SystemProxyPlan.claim(record: nil, services: [owned], port: 9_090)

        #expect(plan.failure == .ownedWithoutBaseline)
        #expect(plan.writes.isEmpty)
        #expect(plan.record == nil)
    }

    @Test("settings that already match the baseline are safe to release")
    func baselineAndClearRouteAreReleased() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let view = SystemProxyView(
            services: [ethernet],
            effective: .direct,
            scopedOwned: false,
            effectiveReadable: true
        )

        let assessment = SystemProxyPlan.assess(record: record, view: view)

        #expect(assessment.released)
        #expect(assessment.writes.isEmpty)
        #expect(assessment.leftInPlace.isEmpty)
    }

    @Test("an external proxy is not overwritten when the live route has left ProxyLab")
    func externalProxyIsReleasedWithoutAWrite() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        var changed = ethernet
        changed.settings = wifi.settings
        let view = SystemProxyView(
            services: [changed],
            effective: wifi.settings,
            scopedOwned: false,
            effectiveReadable: true
        )

        let assessment = SystemProxyPlan.assess(record: record, view: view)

        #expect(assessment.released)
        #expect(assessment.writes.isEmpty)
        #expect(assessment.leftInPlace == ["Ethernet"])
    }

    @MainActor
    @Test("a commit that leaves the live route on 127.0.0.1:9090 is not a release")
    func effectiveRouteStillOwnedIsNotReleased() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        var owned = ethernet
        owned.settings = SystemProxyPlan.owned(port: 9_090)
        let stillOwned = SystemProxyView(
            services: [ethernet],
            effective: SystemProxyPlan.owned(port: 9_090),
            scopedOwned: false,
            effectiveReadable: true
        )
        var commits = 0
        let report = SystemProxyPlan.performRestore(record: record, read: { stillOwned }) { writes in
            commits += 1
            #expect(writes.first?.settings == .direct)
        }

        #expect(commits == 2)
        #expect(report.stillClaimed)
        #expect(report.message?.contains("127.0.0.1:9090") == true)
    }

    @Test("preferences that look restored while the live route is still owned are written again")
    func staleBaselineIsNotBelievedWhileTheRouteIsOwned() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let view = SystemProxyView(
            services: [ethernet],
            effective: SystemProxyPlan.owned(port: 9_090),
            scopedOwned: false,
            effectiveReadable: true
        )

        let assessment = SystemProxyPlan.assess(record: record, view: view)

        #expect(!assessment.released)
        #expect(assessment.writes.first?.settings == .direct)
    }

    @MainActor
    @Test("an unreadable live route is not treated as restored")
    func unreadableRouteIsNotReleased() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let view = SystemProxyView(
            services: [ethernet],
            effective: nil,
            scopedOwned: false,
            effectiveReadable: false
        )

        let report = SystemProxyPlan.performRestore(record: record, read: { view }, commit: { _ in })

        #expect(report.stillClaimed)
        #expect(report.message?.contains("could not verify") == true)
    }

    @Test("unreadable preferences are not treated as the baseline")
    func unreadablePreferencesAreNotReleased() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let view = SystemProxyView(
            services: nil,
            effective: .direct,
            scopedOwned: false,
            effectiveReadable: true
        )

        let assessment = SystemProxyPlan.assess(record: record, view: view)

        #expect(!assessment.released)
        #expect(assessment.writes.isEmpty)
    }

    @Test("a scoped interface still pointing at ProxyLab keeps the claim")
    func scopedProxyKeepsTheClaim() {
        let parsed = SystemProxyPlan.parseEffective(
            [
                "HTTPEnable": 0,
                "HTTPSEnable": 0,
                "__SCOPED__": [
                    "en0": [
                        "HTTPEnable": 1,
                        "HTTPProxy": "127.0.0.1",
                        "HTTPPort": 9_090,
                        "HTTPSEnable": 1,
                        "HTTPSProxy": "127.0.0.1",
                        "HTTPSPort": 9_090,
                    ],
                ],
            ],
            host: "127.0.0.1",
            port: 9_090
        )

        let parsedRoute = try! #require(parsed)
        #expect(parsedRoute.settings == .direct)
        #expect(parsedRoute.scopedOwned)

        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let assessment = SystemProxyPlan.assess(
            record: record,
            view: SystemProxyView(
                services: [ethernet],
                effective: parsedRoute.settings,
                scopedOwned: parsedRoute.scopedOwned,
                effectiveReadable: true
            )
        )
        #expect(!assessment.released)
        #expect(assessment.writes.first?.settings == .direct)
    }

    @Test("a proxy value of an unexpected type is not read as a direct connection")
    func malformedProxyDictionaryIsUnreadable() {
        let parsed = SystemProxyPlan.parseEffective(
            ["HTTPEnable": "yes", "HTTPSEnable": 0],
            host: "127.0.0.1",
            port: 9_090
        )

        #expect(parsed == nil)
    }

    @MainActor
    @Test("two agreeing reads release the claim, and a cancelled write does not")
    func performRestoreAgreesBeforeRelease() {
        let record = try! #require(SystemProxyPlan.claim(record: nil, services: [ethernet], port: 9_090).record)
        let clear = SystemProxyView(
            services: [ethernet],
            effective: .direct,
            scopedOwned: false,
            effectiveReadable: true
        )
        let released = SystemProxyPlan.performRestore(record: record, read: { clear }, commit: { _ in })
        #expect(!released.stillClaimed)

        var sample = 0
        let lagged = SystemProxyPlan.performRestore(
            record: record,
            read: {
                sample += 1
                if sample < 4 {
                    var owned = ethernet
                    owned.settings = SystemProxyPlan.owned(port: 9_090)
                    return SystemProxyView(
                        services: [owned],
                        effective: SystemProxyPlan.owned(port: 9_090),
                        scopedOwned: false,
                        effectiveReadable: true
                    )
                }
                return clear
            },
            commit: { _ in },
            pause: {}
        )
        #expect(!lagged.stillClaimed)
        #expect(sample >= 4)

        let cancelled = SystemProxyPlan.performRestore(
            record: record,
            read: {
                var owned = ethernet
                owned.settings = SystemProxyPlan.owned(port: 9_090)
                return SystemProxyView(
                    services: [owned],
                    effective: SystemProxyPlan.owned(port: 9_090),
                    scopedOwned: false,
                    effectiveReadable: true
                )
            },
            commit: { _ in throw SystemProxyError.authorizationCancelled }
        )
        #expect(cancelled.stillClaimed)
        #expect(cancelled.message?.contains("before quitting") == true)
        #expect(cancelled.message?.contains("127.0.0.1:9090") == true)
    }
}
