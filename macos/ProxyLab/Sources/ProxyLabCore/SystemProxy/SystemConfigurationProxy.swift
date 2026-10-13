//  This Mac's HTTP and HTTPS proxy, changed through System Configuration.
//
//      SCDynamicStore  ──▶  which service is primary (by id, not by name)
//      SCPreferences   ──▶  that service's Proxies dictionary
//                              HTTPEnable / HTTPProxy / HTTPPort
//                              HTTPSEnable / HTTPSProxy / HTTPSPort
//
//  Writing the system preferences needs an administrator's authorization.
//  `system.services.systemconfiguration.network` is granted to root, to a
//  process with Apple's private preference-write entitlement, or to an
//  administrator who authenticates. ProxyLab is none of the first two, so
//  Start and Stop ask with Authorization Services. The system helper
//  (`SCHelper`) performs the write. There is no privileged helper in this
//  app, and no shell.
//
//  The credential is not shared. The system expires it on its own timeout,
//  which this process cannot lengthen. A Stop long after Start asks again.
//  That is the same approval, not a second mechanism. Cancelling it leaves
//  the proxy engine running: there is no way to write the restored settings
//  without it, and stopping the engine while the route still points here
//  would take this Mac off the network.
//
//  Commit and apply are not the proof that the route changed. After they
//  return, the preferences are read again and `SCDynamicStoreCopyProxies`
//  is read for the route applications are actually using, including each
//  interface under `__SCOPED__`. The ownership file is removed only when
//  both say the owned endpoint is gone. A read that fails is not "direct".
//
//  The ownership file is saved before the preferences are committed. If the
//  process dies with the proxy still pointing at 127.0.0.1, the next Start
//  still has the previous settings. Nothing running after SIGKILL, a crash,
//  or a power loss can show the approval dialog. The proxy stays until
//  ProxyLab is opened again, or until somebody turns it off in System
//  Settings. A helper that outlives this process would be required to do
//  that by itself, and this process does not install one.

import Foundation
import Security
import SystemConfiguration

@MainActor
public final class SystemConfigurationProxy: SystemProxyControlling {
    private let file: SystemProxyRecordFile
    private var authorization: AuthorizationRef?

    public init(recordURL: URL) {
        file = SystemProxyRecordFile(url: recordURL)
    }

    /// Beside the IPC socket, which is `~/.proxylab/run/` unless a test or a
    /// developer pointed the socket somewhere else.
    public static func defaultRecordURL() -> URL {
        let socket: String
        if let resolved = try? IPCSocketLocation.resolve() {
            socket = resolved
        } else {
            let home = FileManager.default.homeDirectoryForCurrentUser.path
            socket = URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent(".proxylab", isDirectory: true)
                .appendingPathComponent("run", isDirectory: true)
                .appendingPathComponent("proxylab.sock")
                .path
        }
        return URL(fileURLWithPath: socket)
            .deletingLastPathComponent()
            .appendingPathComponent("system-proxy-ownership.json")
    }

    public var hasClaim: Bool { file.load() != nil }

    public func authorizationNeedsPrompt() -> Bool {
        guard let authorization else { return true }
        return !Self.rightsAreCurrent(authorization)
    }

    public func claim(port: Int) throws {
        let services = try Self.snapshots()
        let usable = usableRecord(file.load(), port: port)
        let plan = SystemProxyPlan.claim(record: usable, services: services, port: port)
        switch plan.failure {
        case .noActiveService:
            throw SystemProxyError.noActiveService
        case .ownedWithoutBaseline:
            throw SystemProxyError.ownedWithoutBaseline
        case nil:
            break
        }
        guard let record = plan.record else { return }
        try file.save(record)
        try commit(plan.writes)
    }

    public func adoptActiveService() throws -> SystemProxyAdoption {
        guard let record = file.load() else { return .none }

        let services = try Self.snapshots()
        let plan = SystemProxyPlan.adopt(record: record, services: services)
        if plan.record != record {
            try file.save(plan.record)
        }
        try commit(plan.writes)
        return SystemProxyAdoption(claimed: plan.writes.map(\.serviceName), ownershipLost: plan.ownershipLost)
    }

    public func restore() throws -> SystemProxyRestoreReport {
        guard let record = file.load() else { return releaseWithoutRecord() }
        return restore(record)
    }

    /// Puts a saved record back, then reads the live route before forgetting it.
    ///
    /// Two reads in a row have to agree that the endpoint is gone. One sample
    /// that looks like the baseline is not a reason to delete the file.
    private func restore(_ record: SystemProxyRecord) -> SystemProxyRestoreReport {
        let report = SystemProxyPlan.performRestore(
            record: record,
            read: { Self.currentView(ownedPort: record.port) },
            commit: { try self.commit($0) },
            pause: {
                // Lets configd publish the write. Sleeping here would also
                // delay that publication, because the store notifies the
                // main run loop.
                RunLoop.main.run(until: Date().addingTimeInterval(0.25))
            }
        )
        if !report.stillClaimed {
            file.remove()
        }
        return report
    }

    /// No ownership file. The live route is still checked: a missing file is
    /// not proof that HTTP and HTTPS are back to what they were.
    private func releaseWithoutRecord() -> SystemProxyRestoreReport {
        let port = CoreLocations.proxyPort
        let view = Self.currentView(ownedPort: port)
        let owned = view.effectiveReadable && (
            view.scopedOwned
                || (view.effective?.matchesOwned(host: SystemProxyPlan.ownedHost, port: port) ?? false)
        )
        if !view.effectiveReadable || owned {
            let message = view.effectiveReadable
                ? SystemProxyPlan.stillOwnedMessage(host: SystemProxyPlan.ownedHost, port: port)
                : SystemProxyPlan.unverifiedMessage(host: SystemProxyPlan.ownedHost, port: port)
            return SystemProxyRestoreReport(
                restored: [],
                leftInPlace: [],
                vanished: [],
                stillClaimed: true,
                message: message
            )
        }
        return .none
    }

    /// Every network service, with the primary one marked from the dynamic store.
    ///
    /// The session is synchronized first. A preferences object cached from
    /// before the authorized write still shows the baseline, and treating
    /// that as "already restored" would delete the ownership file while the
    /// live route still points at the proxy.
    static func snapshots() throws -> [NetworkServiceSnapshot] {
        guard let prefs = SCPreferencesCreate(nil, "ProxyLab" as CFString, nil) else {
            throw SystemProxyError.unreadable
        }
        SCPreferencesSynchronize(prefs)
        guard let services = SCNetworkServiceCopyAll(prefs) as? [SCNetworkService] else {
            throw SystemProxyError.unreadable
        }

        var read: [NetworkServiceSnapshot] = []
        for service in services {
            guard let id = SCNetworkServiceGetServiceID(service) as String? else { continue }
            let name = SCNetworkServiceGetName(service) as String? ?? id
            let interfaceName = SCNetworkServiceGetInterface(service)
                .flatMap { SCNetworkInterfaceGetBSDName($0) as String? }
            read.append(
                NetworkServiceSnapshot(
                    id: id,
                    name: name,
                    interfaceName: interfaceName,
                    enabled: SCNetworkServiceGetEnabled(service),
                    isPrimary: false,
                    settings: try readSettings(service)
                )
            )
        }
        return SystemProxyPlan.markPrimary(read, primaryID: primaryServiceID())
    }

    /// Preferences plus the live route. A failed read stays failed; it is not
    /// filled in as a direct connection.
    static func currentView(ownedPort: Int) -> SystemProxyView {
        let services: [NetworkServiceSnapshot]?
        do {
            services = try snapshots()
        } catch {
            services = nil
        }
        let effective = readEffective(port: ownedPort)
        return SystemProxyView(
            services: services,
            effective: effective.settings,
            scopedOwned: effective.scopedOwned,
            effectiveReadable: effective.readable
        )
    }

    /// The BSD name of the interface the default route is using, if the
    /// system has named one.
    public static func primaryInterfaceName() -> String? {
        dynamicValue("State:/Network/Global/IPv4")?["PrimaryInterface"] as? String
            ?? dynamicValue("State:/Network/Global/IPv6")?["PrimaryInterface"] as? String
    }

    // MARK: - Record

    /// A record for a different endpoint is not this session's previous
    /// settings. It is left on disk only until `claim` replaces it; using it
    /// as a baseline would restore the wrong proxy.
    private func usableRecord(_ record: SystemProxyRecord?, port: Int) -> SystemProxyRecord? {
        guard let record, record.host == SystemProxyPlan.ownedHost, record.port == port else {
            return nil
        }
        return record
    }

    // MARK: - Reading

    private static func readSettings(_ service: SCNetworkService) throws -> SystemProxySettings {
        guard let proto = SCNetworkServiceCopyProtocol(service, kSCNetworkProtocolTypeProxies) else {
            throw SystemProxyError.unreadable
        }
        guard let configuration = SCNetworkProtocolGetConfiguration(proto) else {
            return .direct
        }
        guard let dictionary = configuration as? [String: Any] else {
            throw SystemProxyError.unreadable
        }
        guard let settings = SystemProxyPlan.proxySettings(from: dictionary) else {
            throw SystemProxyError.unreadable
        }
        return settings
    }

    /// The route CFNetwork is using, not the preferences session that was just written.
    private static func readEffective(port: Int) -> (settings: SystemProxySettings?, scopedOwned: Bool, readable: Bool) {
        guard let raw = SCDynamicStoreCopyProxies(nil) as? [String: Any] else {
            return (nil, false, false)
        }
        guard let parsed = SystemProxyPlan.parseEffective(raw, host: SystemProxyPlan.ownedHost, port: port) else {
            return (nil, false, false)
        }
        return (parsed.settings, parsed.scopedOwned, true)
    }

    private static func primaryServiceID() -> String? {
        dynamicValue("State:/Network/Global/IPv4")?["PrimaryService"] as? String
            ?? dynamicValue("State:/Network/Global/IPv6")?["PrimaryService"] as? String
    }

    private static func dynamicValue(_ key: String) -> [String: Any]? {
        guard let store = SCDynamicStoreCreate(nil, "ProxyLab" as CFString, nil, nil) else {
            return nil
        }
        return SCDynamicStoreCopyValue(store, key as CFString) as? [String: Any]
    }

    // MARK: - Writing

    private func commit(_ writes: [SystemProxyWrite]) throws {
        guard !writes.isEmpty else { return }

        let auth = try authorize()
        guard let prefs = SCPreferencesCreateWithAuthorization(nil, "ProxyLab" as CFString, nil, auth) else {
            throw SystemProxyError.unreadable
        }
        guard SCPreferencesLock(prefs, true) else {
            throw SystemProxyError.notApplied("the network configuration is in use")
        }
        defer { _ = SCPreferencesUnlock(prefs) }

        guard let services = SCNetworkServiceCopyAll(prefs) as? [SCNetworkService] else {
            throw SystemProxyError.unreadable
        }
        for write in writes {
            guard let service = services.first(where: {
                (SCNetworkServiceGetServiceID($0) as String?) == write.serviceID
            }) else {
                throw SystemProxyError.notApplied("\(write.serviceName) is no longer a network service")
            }
            try apply(write.settings, to: service, name: write.serviceName)
        }
        guard SCPreferencesCommitChanges(prefs) else {
            throw SystemProxyError.notApplied(Self.errorText())
        }
        guard SCPreferencesApplyChanges(prefs) else {
            throw SystemProxyError.notApplied(Self.errorText())
        }
    }

    private func apply(_ settings: SystemProxySettings, to service: SCNetworkService, name: String) throws {
        guard let proto = SCNetworkServiceCopyProtocol(service, kSCNetworkProtocolTypeProxies) else {
            throw SystemProxyError.notApplied("\(name) has no proxy settings")
        }
        var configuration = SCNetworkProtocolGetConfiguration(proto) as? [String: Any] ?? [:]
        write(
            settings.http,
            enableKey: kSCPropNetProxiesHTTPEnable,
            hostKey: kSCPropNetProxiesHTTPProxy,
            portKey: kSCPropNetProxiesHTTPPort,
            into: &configuration
        )
        write(
            settings.https,
            enableKey: kSCPropNetProxiesHTTPSEnable,
            hostKey: kSCPropNetProxiesHTTPSProxy,
            portKey: kSCPropNetProxiesHTTPSPort,
            into: &configuration
        )
        guard SCNetworkProtocolSetConfiguration(proto, configuration as CFDictionary) else {
            throw SystemProxyError.notApplied("\(name) did not accept the proxy settings")
        }
    }

    private func write(
        _ setting: ProxyProtocolSetting,
        enableKey: CFString,
        hostKey: CFString,
        portKey: CFString,
        into configuration: inout [String: Any]
    ) {
        configuration[enableKey as String] = setting.enabled ? 1 : 0
        if let host = setting.host {
            configuration[hostKey as String] = host
        } else {
            configuration.removeValue(forKey: hostKey as String)
        }
        if let port = setting.port {
            configuration[portKey as String] = port
        } else {
            configuration.removeValue(forKey: portKey as String)
        }
    }

    private static func errorText() -> String {
        let message = String(cString: SCErrorString(SCError()))
        return message.isEmpty ? "the system refused the change" : message
    }

    // MARK: - Authorization

    /// An administrator credential for `system.services.systemconfiguration.network`.
    ///
    /// Reused while it is still valid, so a Start and the Stop that follows
    /// it immediately do not ask twice. When it has expired, macOS asks again.
    private func authorize() throws -> AuthorizationRef {
        if let authorization, Self.rightsAreCurrent(authorization) {
            return authorization
        }
        if let authorization {
            AuthorizationFree(authorization, [])
            self.authorization = nil
        }

        var created: AuthorizationRef?
        let status = AuthorizationCreate(nil, nil, [], &created)
        guard status == errAuthorizationSuccess, let created else {
            throw SystemProxyError.authorizationDenied
        }
        do {
            try Self.copyRights(created, prompt: true)
        } catch {
            AuthorizationFree(created, [])
            throw error
        }
        authorization = created
        return created
    }

    private static func rightsAreCurrent(_ authorization: AuthorizationRef) -> Bool {
        (try? copyRights(authorization, prompt: false)) != nil
    }

    private static func copyRights(_ authorization: AuthorizationRef, prompt: Bool) throws {
        let name = "system.services.systemconfiguration.network"
        let promptText = "ProxyLab sets this Mac's HTTP and HTTPS proxy to 127.0.0.1:9090 while capture is on, and restores the previous proxy when capture stops."
        let status: OSStatus = name.withCString { name in
            promptText.withCString { promptText in
                var rightsItem = AuthorizationItem(name: name, valueLength: 0, value: nil, flags: 0)
                return withUnsafeMutablePointer(to: &rightsItem) { rightsPointer in
                    var rights = AuthorizationRights(count: 1, items: rightsPointer)
                    let flags: AuthorizationFlags = prompt
                        ? [.interactionAllowed, .extendRights]
                        : [.extendRights]
                    if prompt {
                        // `kAuthorizationEnvironmentPrompt` is a String. AuthorizationItem
                        // stores `name` rather than copying it, so the C string has to
                        // live until AuthorizationCopyRights returns. `promptText` is
                        // already that kind of pointer: the withCString above covers
                        // this whole call.
                        return kAuthorizationEnvironmentPrompt.withCString { promptName in
                            var promptItem = AuthorizationItem(
                                name: promptName,
                                valueLength: strlen(promptText),
                                value: UnsafeMutableRawPointer(mutating: promptText),
                                flags: 0
                            )
                            return withUnsafeMutablePointer(to: &promptItem) { promptPointer in
                                var environment = AuthorizationEnvironment(count: 1, items: promptPointer)
                                return AuthorizationCopyRights(authorization, &rights, &environment, flags, nil)
                            }
                        }
                    }
                    return AuthorizationCopyRights(authorization, &rights, nil, flags, nil)
                }
            }
        }
        switch status {
        case errAuthorizationSuccess:
            return
        case errAuthorizationCanceled:
            throw SystemProxyError.authorizationCancelled
        case errAuthorizationInteractionNotAllowed:
            throw SystemProxyError.authorizationDenied
        default:
            throw SystemProxyError.authorizationDenied
        }
    }
}
