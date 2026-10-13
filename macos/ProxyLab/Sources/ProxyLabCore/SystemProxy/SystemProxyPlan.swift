//  What to change, decided before anything is written.
//
//  Start, a network change, and Stop each produce a plan: which services to
//  write, which previous settings to keep, and which services to leave alone
//  because they no longer hold the configuration ProxyLab set. The plan is a
//  value. Applying it — authorization, the preferences database, the file —
//  is someone else's job, and a test never has to do it.
//
//  # Ownership
//
//  A service is owned while its HTTP and HTTPS proxy are the explicit
//  listener this record names. Stop writes the saved settings back only
//  then. Anything else means someone changed the service after ProxyLab took
//  it, and the plan does not write over that.
//
//  # The active service
//
//  The service to configure is the one marked primary. The mark comes from
//  the system's primary service id. A service named Wi-Fi is not preferred,
//  and a service that is not primary is not configured until it becomes so.

import Foundation
import SystemConfiguration

/// Why a plan cannot claim a proxy.
enum SystemProxyFailure: Equatable, Sendable {
    /// The system did not name a primary network service.
    case noActiveService
    /// The service already points at ProxyLab and no baseline was saved.
    /// Recording that address as the previous proxy would make Stop restore
    /// ProxyLab to itself.
    case ownedWithoutBaseline
}

/// One look at the preferences database and the live proxy route.
struct SystemProxyView: Equatable, Sendable {
    /// `nil` when the preferences database could not be read. An empty list
    /// is a successful read of a Mac with no services.
    var services: [NetworkServiceSnapshot]?
    /// The effective HTTP and HTTPS proxy, when the dynamic store answered.
    var effective: SystemProxySettings?
    /// A per-interface entry in that answer still points at ProxyLab.
    var scopedOwned: Bool
    /// The dynamic store answered with a dictionary we could read.
    var effectiveReadable: Bool
}

/// Whether Stop may drop the ownership file and stop the proxy engine.
struct SystemProxyAssessment: Equatable, Sendable {
    var writes: [SystemProxyWrite]
    var leftInPlace: [String]
    var vanished: [String]
    /// Preferences and the live route both show that ProxyLab's endpoint is gone.
    var released: Bool
    var message: String?
}

/// What Start should persist and write.
struct SystemProxyClaimPlan: Equatable, Sendable {
    var record: SystemProxyRecord?
    var writes: [SystemProxyWrite]
    /// Said when a previous session's saved settings were not restored
    /// because the service no longer held them.
    var notice: String?
    var failure: SystemProxyFailure?
}

/// What a network change should do while a claim is in force.
struct SystemProxyAdoptPlan: Equatable, Sendable {
    var record: SystemProxyRecord
    var writes: [SystemProxyWrite]
    /// Services that no longer hold ProxyLab's proxy, and were not written.
    var ownershipLost: [String]
}

/// What Stop should write back.
struct SystemProxyRestorePlan: Equatable, Sendable {
    var writes: [SystemProxyWrite]
    /// Services whose settings were not ProxyLab's, and were not written.
    var leftInPlace: [String]
    /// Services that were in the record and are no longer on the system.
    var vanished: [String]
}

enum SystemProxyPlan {
    /// The explicit listener's host. Not a LAN address, and not the bridge.
    static let ownedHost = "127.0.0.1"

    static func owned(port: Int) -> SystemProxySettings {
        .owned(host: ownedHost, port: port)
    }

    /// The service the system named, if it is in `services`.
    static func primary(in services: [NetworkServiceSnapshot]) -> NetworkServiceSnapshot? {
        services.first { $0.isPrimary }
    }

    /// Marks `isPrimary` from a service id. The name is not consulted.
    static func markPrimary(
        _ services: [NetworkServiceSnapshot],
        primaryID: String?
    ) -> [NetworkServiceSnapshot] {
        services.map { service in
            var service = service
            service.isPrimary = primaryID != nil && service.id == primaryID
            return service
        }
    }

    /// Start: remember what the primary service has now, then point it at the
    /// explicit listener.
    ///
    /// When the service already points at that listener and a record already
    /// knows the previous settings, those previous settings are kept. Saving
    /// the current value there would make Stop restore ProxyLab to itself.
    static func claim(
        record: SystemProxyRecord?,
        services: [NetworkServiceSnapshot],
        port: Int
    ) -> SystemProxyClaimPlan {
        guard let primary = primary(in: services) else {
            return SystemProxyClaimPlan(record: record, writes: [], notice: nil, failure: .noActiveService)
        }

        let owned = owned(port: port)
        if primary.settings.matchesOwned(host: ownedHost, port: port), record?.stored(primary.id) == nil {
            return SystemProxyClaimPlan(record: record, writes: [], notice: nil, failure: .ownedWithoutBaseline)
        }

        var notice: String?
        let previous = baseline(current: primary.settings, stored: record?.stored(primary.id), owned: owned, notice: &notice)
        var servicesRecord = record?.services ?? []
        if let index = servicesRecord.firstIndex(where: { $0.id == primary.id }) {
            servicesRecord[index].name = primary.name
            servicesRecord[index].previous = previous
        } else {
            servicesRecord.append(
                StoredProxyService(id: primary.id, name: primary.name, previous: previous)
            )
        }

        let updated = SystemProxyRecord(version: 1, host: ownedHost, port: port, services: servicesRecord)
        let writes = writesNeeded(on: primary, settings: owned)
        return SystemProxyClaimPlan(record: updated, writes: writes, notice: notice, failure: nil)
    }

    /// The active service changed while ProxyLab holds a claim.
    ///
    /// A service already in the record is written again only when it still
    /// has the settings we saved and the owned proxy never landed. A third
    /// value is an external change: it is reported and not overwritten. A
    /// service that was not in the record is a newly active one; its current
    /// settings are saved and then replaced.
    static func adopt(
        record: SystemProxyRecord,
        services: [NetworkServiceSnapshot]
    ) -> SystemProxyAdoptPlan {
        guard let primary = primary(in: services) else {
            return SystemProxyAdoptPlan(record: record, writes: [], ownershipLost: [])
        }

        let owned = record.owned
        if let stored = record.stored(primary.id) {
            if primary.settings.matchesOwned(host: record.host, port: record.port) {
                return SystemProxyAdoptPlan(record: record, writes: [], ownershipLost: [])
            }
            if primary.settings == stored.previous {
                var record = record
                if let index = record.services.firstIndex(where: { $0.id == primary.id }) {
                    record.services[index].name = primary.name
                }
                return SystemProxyAdoptPlan(
                    record: record,
                    writes: [SystemProxyWrite(serviceID: primary.id, serviceName: primary.name, settings: owned)],
                    ownershipLost: []
                )
            }
            return SystemProxyAdoptPlan(record: record, writes: [], ownershipLost: [primary.name])
        }

        var record = record
        record.services.append(
            StoredProxyService(id: primary.id, name: primary.name, previous: primary.settings)
        )
        return SystemProxyAdoptPlan(
            record: record,
            writes: writesNeeded(on: primary, settings: owned),
            ownershipLost: []
        )
    }

    /// Stop: put back every saved service that still has ProxyLab's proxy.
    ///
    /// A service that does not is named in `leftInPlace` and is not written.
    /// There is no step that sets a service to "direct" because that is not
    /// what was saved.
    static func restore(
        record: SystemProxyRecord,
        services: [NetworkServiceSnapshot]
    ) -> SystemProxyRestorePlan {
        var writes: [SystemProxyWrite] = []
        var leftInPlace: [String] = []
        var vanished: [String] = []

        for stored in record.services {
            guard let service = services.first(where: { $0.id == stored.id }) else {
                vanished.append(stored.name)
                continue
            }
            if service.settings == stored.previous {
                // The saved settings are already there. Either the write never
                // landed, or a restore already did. Nothing is put back, and
                // this is not an external change.
                continue
            }
            if service.settings.matchesOwned(host: record.host, port: record.port) {
                writes.append(
                    SystemProxyWrite(
                        serviceID: stored.id,
                        serviceName: service.name,
                        settings: stored.previous
                    )
                )
            } else {
                leftInPlace.append(service.name)
            }
        }

        return SystemProxyRestorePlan(writes: writes, leftInPlace: leftInPlace, vanished: vanished)
    }

    /// Stop's decision, from the preferences and the live route together.
    ///
    /// A preferences read that matches the saved baseline is not enough. The
    /// dynamic store is what applications are using. When that store still
    /// names the owned endpoint, or when either source cannot be read, the
    /// claim stays. A service whose settings are neither the baseline nor
    /// the owned endpoint is not written.
    static func assess(record: SystemProxyRecord, view: SystemProxyView) -> SystemProxyAssessment {
        let effectiveOwned = view.effectiveReadable && (
            view.scopedOwned
                || (view.effective?.matchesOwned(host: record.host, port: record.port) ?? false)
        )

        guard let services = view.services else {
            return SystemProxyAssessment(
                writes: [],
                leftInPlace: [],
                vanished: [],
                released: false,
                message: unverifiedMessage(host: record.host, port: record.port)
            )
        }

        var writes: [SystemProxyWrite] = []
        var leftInPlace: [String] = []
        var vanished: [String] = []
        var preferencesOwned = false

        for stored in record.services {
            guard let service = services.first(where: { $0.id == stored.id }) else {
                vanished.append(stored.name)
                continue
            }
            if service.settings.matchesOwned(host: record.host, port: record.port) {
                preferencesOwned = true
                appendRestore(&writes, stored: stored, service: service, record: record)
            } else if service.settings == stored.previous {
                if effectiveOwned {
                    appendRestore(&writes, stored: stored, service: service, record: record)
                }
            } else {
                leftInPlace.append(service.name)
            }
        }

        let released = view.effectiveReadable && !effectiveOwned && !preferencesOwned && writes.isEmpty
        return SystemProxyAssessment(
            writes: writes,
            leftInPlace: leftInPlace,
            vanished: vanished,
            released: released,
            message: assessmentMessage(
                record: record,
                released: released,
                effectiveReadable: view.effectiveReadable,
                effectiveOwned: effectiveOwned,
                preferencesOwned: preferencesOwned,
                writes: writes,
                leftInPlace: leftInPlace,
                vanished: vanished
            )
        )
    }

    /// The dynamic store's proxy dictionary, including per-interface entries.
    ///
    /// `nil` when a value is present but not a shape we can trust. Missing
    /// keys are a proxy that is off, which is a real reading.
    static func parseEffective(
        _ proxies: [String: Any],
        host: String,
        port: Int
    ) -> (settings: SystemProxySettings, scopedOwned: Bool)? {
        guard let settings = proxySettings(from: proxies) else { return nil }
        var scopedOwned = settings.matchesOwned(host: host, port: port)
        if let scoped = proxies["__SCOPED__"] {
            guard let interfaces = scoped as? [String: Any] else { return nil }
            for value in interfaces.values {
                guard let dictionary = value as? [String: Any] else { return nil }
                guard let parsed = proxySettings(from: dictionary) else { return nil }
                if parsed.matchesOwned(host: host, port: port) {
                    scopedOwned = true
                }
            }
        }
        return (settings, scopedOwned)
    }

    /// HTTP and HTTPS from one proxy dictionary.
    ///
    /// `nil` when an enable, host, or port value is present in a form that
    /// is not a number or a string. That is not "direct": treating it as
    /// direct would look like the baseline on a Mac that had no proxy.
    static func proxySettings(from configuration: [String: Any]) -> SystemProxySettings? {
        guard
            let http = protocolSetting(
                configuration,
                enable: kSCPropNetProxiesHTTPEnable,
                host: kSCPropNetProxiesHTTPProxy,
                port: kSCPropNetProxiesHTTPPort
            ),
            let https = protocolSetting(
                configuration,
                enable: kSCPropNetProxiesHTTPSEnable,
                host: kSCPropNetProxiesHTTPSProxy,
                port: kSCPropNetProxiesHTTPSPort
            )
        else { return nil }
        return SystemProxySettings(http: http, https: https)
    }

    static func stillOwnedMessage(host: String, port: Int) -> String {
        "This Mac is still sending HTTP and HTTPS through ProxyLab at \(host):\(port). "
            + "The proxy engine was left running."
    }

    static func unverifiedMessage(host: String, port: Int) -> String {
        "ProxyLab could not verify this Mac's HTTP and HTTPS proxy. "
            + "It may still be \(host):\(port), so the proxy engine was left running."
    }

    static func authorizationBlockedMessage(host: String, port: Int) -> String {
        "This Mac is still sending HTTP and HTTPS through ProxyLab at \(host):\(port). "
            + "ProxyLab needs an administrator's approval to restore the previous proxy. "
            + "Restore it before quitting. The proxy engine was left running."
    }

    /// Applies `assess` to fresh reads. The ownership file is the caller's:
    /// this returns `stillClaimed: false` only after two reads agree the
    /// endpoint is gone. A commit that returns and a following read that
    /// still shows the endpoint is not a release.
    ///
    /// `pause` runs after a commit, and again while the route still looks
    /// owned. The dynamic store keeps the previous proxy for a moment after
    /// the preference write; reading it in that moment is not proof the
    /// write failed. The default pause does not wait, so a test whose read
    /// never changes still finishes immediately.
    @MainActor
    static func performRestore(
        record: SystemProxyRecord,
        read: () -> SystemProxyView,
        commit: ([SystemProxyWrite]) throws -> Void,
        pause: () -> Void = {}
    ) -> SystemProxyRestoreReport {
        var commits = 0
        var confirmedRelease = false
        var restored: [String] = []
        var stalledReads = 0
        var waitForStore = false

        while true {
            if waitForStore {
                waitForStore = false
                pause()
            }

            let assessment = assess(record: record, view: read())
            if assessment.released {
                stalledReads = 0
                if confirmedRelease {
                    return SystemProxyRestoreReport(
                        restored: restored,
                        leftInPlace: assessment.leftInPlace,
                        vanished: assessment.vanished,
                        stillClaimed: false,
                        message: assessment.message
                    )
                }
                confirmedRelease = true
                continue
            }

            confirmedRelease = false
            if !assessment.writes.isEmpty, commits < 2 {
                do {
                    try commit(assessment.writes)
                } catch {
                    return held(record: record, assessment: assessment, error: error)
                }
                restored.append(contentsOf: assessment.writes.map(\.serviceName))
                commits += 1
                stalledReads = 0
                waitForStore = true
                continue
            }

            if stalledReads < 8 {
                stalledReads += 1
                waitForStore = true
                continue
            }

            return held(record: record, assessment: assessment, error: nil)
        }
    }

    private static func held(
        record: SystemProxyRecord,
        assessment: SystemProxyAssessment,
        error: Error?
    ) -> SystemProxyRestoreReport {
        let message: String
        if let error = error as? SystemProxyError {
            switch error {
            case .authorizationCancelled, .authorizationDenied:
                message = authorizationBlockedMessage(host: record.host, port: record.port)
            default:
                message = error.errorDescription
                    ?? assessment.message
                    ?? stillOwnedMessage(host: record.host, port: record.port)
            }
        } else {
            message = assessment.message ?? stillOwnedMessage(host: record.host, port: record.port)
        }
        return SystemProxyRestoreReport(
            restored: [],
            leftInPlace: assessment.leftInPlace,
            vanished: assessment.vanished,
            stillClaimed: true,
            message: message
        )
    }

    /// The sentence for services that were deliberately not written.
    static func externalChangeMessage(_ names: [String]) -> String? {
        guard !names.isEmpty else { return nil }

        let list = named(names)
        return "The HTTP and HTTPS proxy on \(list) changed after ProxyLab set it. "
            + "ProxyLab left that setting in place."
    }

    static func vanishedMessage(_ names: [String]) -> String? {
        guard !names.isEmpty else { return nil }

        let list = named(names)
        return "\(list) is no longer available, so ProxyLab could not restore the proxy it had saved for \(names.count == 1 ? "it" : "them")."
    }

    // MARK: - Helpers

    private static func baseline(
        current: SystemProxySettings,
        stored: StoredProxyService?,
        owned: SystemProxySettings,
        notice: inout String?
    ) -> SystemProxySettings {
        if current.matchesOwned(host: ownedHost, port: owned.http.port ?? 0), let stored {
            return stored.previous
        }
        if let stored, current != stored.previous, !current.matchesOwned(host: ownedHost, port: owned.http.port ?? 0) {
            notice = "The HTTP and HTTPS proxy on \(stored.name) had changed since ProxyLab set it. "
                + "ProxyLab did not restore the older setting, and saved what is there now."
        }
        return current
    }

    private static func writesNeeded(
        on service: NetworkServiceSnapshot,
        settings: SystemProxySettings
    ) -> [SystemProxyWrite] {
        guard !service.settings.matchesOwned(host: ownedHost, port: settings.http.port ?? 0) else {
            return []
        }
        return [SystemProxyWrite(serviceID: service.id, serviceName: service.name, settings: settings)]
    }

    private static func named(_ names: [String]) -> String {
        if names.count == 1 { return names[0] }
        return names.dropLast().joined(separator: ", ") + " and " + (names.last ?? "")
    }

    /// Writes the saved settings back unless those saved settings are
    /// ProxyLab's own endpoint. A record that says the previous proxy was
    /// itself cannot be restored; the caller keeps the core running.
    private static func appendRestore(
        _ writes: inout [SystemProxyWrite],
        stored: StoredProxyService,
        service: NetworkServiceSnapshot,
        record: SystemProxyRecord
    ) {
        guard !stored.previous.matchesOwned(host: record.host, port: record.port) else { return }
        writes.append(
            SystemProxyWrite(serviceID: stored.id, serviceName: service.name, settings: stored.previous)
        )
    }

    private static func assessmentMessage(
        record: SystemProxyRecord,
        released: Bool,
        effectiveReadable: Bool,
        effectiveOwned: Bool,
        preferencesOwned: Bool,
        writes: [SystemProxyWrite],
        leftInPlace: [String],
        vanished: [String]
    ) -> String? {
        if released {
            return externalChangeMessage(leftInPlace) ?? vanishedMessage(vanished)
        }
        if !effectiveReadable || record.services.isEmpty {
            return unverifiedMessage(host: record.host, port: record.port)
        }
        if effectiveOwned, !preferencesOwned, writes.isEmpty {
            return "The effective HTTP and HTTPS proxy is still \(record.host):\(record.port). "
                + "ProxyLab did not overwrite the other proxy settings, and left its proxy engine running."
        }
        if preferencesOwned || effectiveOwned || !writes.isEmpty {
            return stillOwnedMessage(host: record.host, port: record.port)
        }
        return unverifiedMessage(host: record.host, port: record.port)
    }

    private static func protocolSetting(
        _ configuration: [String: Any],
        enable: CFString,
        host: CFString,
        port: CFString
    ) -> ProxyProtocolSetting? {
        let enableValue = configuration[enable as String]
        let enabled: Bool
        if let enableValue {
            guard let number = configurationInteger(enableValue) else { return nil }
            enabled = number != 0
        } else {
            enabled = false
        }

        let hostValue = configuration[host as String]
        let hostText: String?
        if let hostValue {
            guard let text = hostValue as? String else { return nil }
            hostText = text.isEmpty ? nil : text
        } else {
            hostText = nil
        }

        let portValue = configuration[port as String]
        let portNumber: Int?
        if let portValue {
            guard let number = configurationInteger(portValue) else { return nil }
            portNumber = number
        } else {
            portNumber = nil
        }

        return ProxyProtocolSetting(enabled: enabled, host: hostText, port: portNumber)
    }

    static func configurationInteger(_ value: Any) -> Int? {
        // NSNumber first. A CFBoolean is an NSNumber whose intValue is 0 or 1,
        // and a port is an NSNumber whose intValue is the port. Matching Bool
        // before that turns some integer ports into 1.
        if let number = value as? NSNumber {
            return number.intValue
        }
        if let value = value as? Bool {
            return value ? 1 : 0
        }
        if let value = value as? Int {
            return value
        }
        if let value = value as? Int32 {
            return Int(value)
        }
        if let value = value as? Int64 {
            return Int(value)
        }
        return nil
    }
}
