//  Which flows macOS hands this provider, and what it does with each one.
//
//  # What is asked for
//
//  Outbound TCP to ports 80 and 443, on IPv4 and IPv6, and nothing on
//  loopback. No UDP, so no QUIC and no DNS, and no other ports. A flow macOS
//  does not hand over is carried by macOS exactly as if this provider were
//  not installed. UDP is left out of the rules rather than accepted and
//  returned: on current macOS, returning false from the UDP flow handler
//  terminates the flow.
//
//  Loopback is excluded for a reason beyond "it never leaves the Mac": the
//  connection this provider opens to the core's bridge is loopback, and so is
//  the explicit proxy on 9090. A provider's own connections are never handed
//  back to it by the system, and `FlowPolicy` refuses loopback again on top of
//  that, so the bridge cannot be handed a flow to itself.
//
//  # What each flow gets
//
//  `FlowPolicy` decides, synchronously, because `handleNewFlow` must answer
//  at once: a flow is either left to macOS (`false`) or claimed (`true`) and
//  given a `FlowRelay`, which does the rest on a queue of its own.

import Foundation
import Network
import NetworkExtension
import TransparentProxyKit
import os

@objc(ProxyLabTransparentProxyProvider)
final class TransparentProxyProvider: NETransparentProxyProvider, @unchecked Sendable {
    private let policy = FlowPolicy()
    private let relays = RelayRegistry()
    private let configuration = OSAllocatedUnfairLock(initialState: ProviderConfiguration())
    private let log = Logger(subsystem: "com.proxylab.ProxyLab.TransparentProxy", category: "provider")

    override func startProxy(
        options: [String: Any]?,
        completionHandler: @escaping (Error?) -> Void
    ) {
        let stored = (protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration
        guard let configuration = ProviderConfiguration(dictionary: stored) else {
            log.error("Refusing to start: the saved configuration names no usable bridge port")
            completionHandler(NEVPNError(.configurationInvalid))
            return
        }
        self.configuration.withLock { $0 = configuration }

        log.notice("Starting; flows go to the core's bridge on port \(configuration.bridgePort)")
        // Called once, by the system, on a queue of its choosing: the
        // framework's handler is not marked sendable, and is safe to call
        // from there.
        nonisolated(unsafe) let started = completionHandler
        setTunnelNetworkSettings(Self.networkSettings()) { error in
            started(error)
        }
    }

    override func stopProxy(
        with reason: NEProviderStopReason,
        completionHandler: @escaping () -> Void
    ) {
        log.notice("Stopping (reason \(reason.rawValue)); closing \(self.relays.count) flow(s)")
        relays.cancelAll()
        completionHandler()
    }

    override func handleNewFlow(_ flow: NEAppProxyFlow) -> Bool {
        let source = flow.metaData.sourceAppAuditToken.flatMap(SourceProcess.init(auditToken:))

        // UDP, including QUIC on 443. Returning false does not close it:
        // for this provider, macOS then carries the flow itself. The flow
        // is not opened, which is what would claim it.
        guard let tcp = flow as? NEAppProxyTCPFlow else {
            _ = policy.decide(FlowCandidate(host: "", port: 443, source: source, transport: .udp))
            return false
        }

        guard let destination = FlowDestination(of: tcp) else {
            _ = policy.decide(FlowCandidate(host: "", port: 0, source: source, transport: .tcp))
            return false
        }

        let candidate = FlowCandidate(
            host: destination.host,
            port: destination.port,
            source: source,
            transport: .tcp
        )

        guard policy.decide(candidate) == .bridge else { return false }

        let relay = FlowRelay(
            flow: tcp,
            destination: destination,
            source: source,
            bridgePort: configuration.withLock { $0.bridgePort },
            policy: policy,
            registry: relays
        )
        relay.start()

        return true
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        guard ProviderMessage.decode(messageData) == .status else {
            completionHandler?(nil)
            return
        }

        completionHandler?(policy.status().encoded)
    }

    /// The flows `FlowPolicy` names, on IPv4 and IPv6, and not on loopback.
    static func networkSettings() -> NETransparentProxyNetworkSettings {
        let settings = NETransparentProxyNetworkSettings(
            tunnelRemoteAddress: ProviderConfiguration.bridgeHost
        )

        settings.includedNetworkRules = FlowPolicy.deliveredRules.flatMap { delivered in
            FlowPolicy.includedNetworks.map { network in
                rule(network: network.address, prefix: network.prefix, port: delivered.port)
            }
        }
        settings.excludedNetworkRules = FlowPolicy.excludedLoopback.map { network in
            rule(network: network.address, prefix: network.prefix, port: nil)
        }

        return settings
    }

    /// Outbound TCP to `network`/`prefix`, on `port` or on any port.
    ///
    /// macOS 15 replaced the endpoint type these rules are written in. The
    /// older one is still what macOS 14 has, and Swift reaches it under the
    /// name the SDK keeps it at. The protocol is TCP: UDP is not asked for.
    private static func rule(network: String, prefix: Int, port: UInt16?) -> NENetworkRule {
        if #available(macOS 15.0, *) {
            return NENetworkRule(
                remoteNetworkEndpoint: .hostPort(
                    host: NWEndpoint.Host(network),
                    port: port.flatMap(NWEndpoint.Port.init(rawValue:)) ?? .any
                ),
                remotePrefix: prefix,
                localNetworkEndpoint: nil,
                localPrefix: 0,
                protocol: .TCP,
                direction: .outbound
            )
        }

        return NENetworkRule(
            __remoteNetwork: __NWHostEndpoint(hostname: network, port: port.map(String.init) ?? "0"),
            remotePrefix: prefix,
            localNetwork: nil,
            localPrefix: 0,
            protocol: .TCP,
            direction: .outbound
        )
    }
}

/// Where a flow was going, as the system reports it.
struct FlowDestination: Sendable {
    let host: String
    let port: UInt16

    /// Read from the flow's remote endpoint: an address almost always, since
    /// the application resolved the name before it connected, and a name
    /// when it connected through an API that takes one.
    init?(of flow: NEAppProxyTCPFlow) {
        let host: String?
        let port: UInt16?

        if #available(macOS 15.0, *) {
            guard case let .hostPort(endpointHost, endpointPort) = flow.remoteFlowEndpoint else {
                return nil
            }
            host = Self.text(endpointHost)
            port = endpointPort.rawValue
        } else {
            let endpoint = flow.__remoteEndpoint as? __NWHostEndpoint
            host = endpoint?.hostname
            port = endpoint.flatMap { UInt16($0.port) }
        }

        guard let host, !host.isEmpty, let port, port != 0 else { return nil }

        self.host = host
        self.port = port
    }

    /// A host as text: a name as it was given, an address in its usual
    /// notation and without the interface an IPv6 one may be scoped to.
    @available(macOS 15.0, *)
    private static func text(_ host: NWEndpoint.Host) -> String? {
        switch host {
        case let .name(name, _):
            return name
        case let .ipv4(address):
            return Self.presentation(address.rawValue, family: AF_INET)
        case let .ipv6(address):
            return Self.presentation(address.rawValue, family: AF_INET6)
        @unknown default:
            return nil
        }
    }

    /// `inet_ntop`, which is what writes an address the way the core's parser
    /// reads one.
    private static func presentation(_ bytes: Data, family: Int32) -> String? {
        var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
        let written = bytes.withUnsafeBytes { raw in
            inet_ntop(family, raw.baseAddress, &buffer, socklen_t(buffer.count))
        }
        guard written != nil else { return nil }

        let text = buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
        return String(decoding: text, as: UTF8.self)
    }
}
