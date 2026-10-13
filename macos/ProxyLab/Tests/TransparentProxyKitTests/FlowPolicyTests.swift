//  Which flows the provider hands to the core.
//
//  The two properties that matter most are both about what is *not* handed
//  over: nothing on loopback, so the bridge can never be asked to carry a
//  flow to itself, and nothing from a process the core has said is its own,
//  so ProxyLab's outbound connections go straight out. The third is the Mac
//  keeping its network when the core is not running.

import Foundation
import Testing

@testable import TransparentProxyKit

@Suite("The flow policy")
struct FlowPolicyTests {
    private let chrome = SourceProcess(pid: 501, pidVersion: 3)
    private let core = SourceProcess(pid: 812, pidVersion: 1)

    private func flow(
        _ host: String,
        _ port: UInt16,
        from source: SourceProcess? = nil,
        transport: FlowTransport = .tcp
    ) -> FlowCandidate {
        FlowCandidate(host: host, port: port, source: source ?? chrome, transport: transport)
    }

    @Test("HTTP and HTTPS to the outside are handed to the core")
    func bridgesWeb() {
        let policy = FlowPolicy()

        #expect(policy.decide(flow("93.184.215.14", 443)) == .bridge)
        #expect(policy.decide(flow("93.184.215.14", 80)) == .bridge)
        #expect(policy.decide(flow("2606:2800:220:1::248", 443)) == .bridge)
        #expect(policy.decide(flow("example.com", 443)) == .bridge)
    }

    @Test("TCP 80 and TCP 443 are claimed, and other TCP ports are not")
    func claimedPortsAreCounted() {
        let policy = FlowPolicy()

        #expect(policy.decide(flow("93.184.215.14", 80)) == .bridge)
        #expect(policy.decide(flow("93.184.215.14", 443)) == .bridge)
        #expect(policy.decide(flow("93.184.215.14", 8443)) == .leave(.port))

        let counters = policy.status().counters
        #expect(counters.claimedTcp80 == 1)
        #expect(counters.claimedTcp443 == 1)
        #expect(counters.bypassedPort == 1)
        #expect(counters.bypassedUdp == 0)
    }

    @Test("UDP, including QUIC on 443, is left to macOS and counted")
    func udpIsNotClaimed() {
        let policy = FlowPolicy()

        #expect(policy.decide(flow("93.184.215.14", 443, transport: .udp)) == .leave(.udp))
        #expect(policy.decide(flow("93.184.215.14", 80, transport: .udp)) == .leave(.udp))
        #expect(policy.decide(flow("2001:db8::1", 443, transport: .udp)) == .leave(.udp))
        // Loopback is still loopback when it is UDP.
        #expect(policy.decide(flow("127.0.0.1", 443, transport: .udp)) == .leave(.loopback))

        let counters = policy.status().counters
        #expect(counters.bypassedUdp == 3)
        #expect(counters.bypassedLoopback == 1)
        #expect(counters.claimedTcp443 == 0)
        #expect(counters.bridged == 0)
    }

    @Test("the rules claim TCP 80 and 443 on IPv4 and IPv6, and do not ask for UDP")
    func deliveredRules() {
        #expect(FlowPolicy.deliveredRules == [
            TransparentNetworkRule(transport: .tcp, port: 80, claimed: true),
            TransparentNetworkRule(transport: .tcp, port: 443, claimed: true),
        ])
        #expect(FlowPolicy.deliveredRules.allSatisfy { $0.transport == .tcp && $0.claimed })
        #expect(FlowPolicy.includedNetworks.map(\.address) == ["0.0.0.0", "::"])
        #expect(FlowPolicy.excludedLoopback.map(\.address) == ["127.0.0.0", "::1"])
        let claimed = Set(FlowPolicy.deliveredRules.filter(\.claimed).map(\.port))
        #expect(claimed == FlowPolicy.interceptedPorts)
    }

    @Test("other ports are left to macOS")
    func otherPorts() {
        let policy = FlowPolicy()

        for port: UInt16 in [22, 25, 993, 5223, 8080, 8443] {
            #expect(policy.decide(flow("93.184.215.14", port)) == .leave(.port), "\(port)")
        }
    }

    @Test("loopback is never handed over, however it is spelled")
    func loopback() {
        let policy = FlowPolicy()

        for host in [
            "127.0.0.1", "127.0.0.53", "::1", "[::1]", "localhost", "LOCALHOST",
            "api.localhost", "0.0.0.0", "::", "::ffff:127.0.0.1",
        ] {
            #expect(policy.decide(flow(host, 443)) == .leave(.loopback), "\(host)")
        }
    }

    @Test("an address that only looks like loopback is not")
    func notLoopback() {
        let policy = FlowPolicy()

        for host in ["128.0.0.1", "10.127.0.1", "localhost.example.com", "1127.0.0.1"] {
            #expect(policy.decide(flow(host, 443)) == .bridge, "\(host)")
        }
    }

    @Test("a flow with no destination is left alone")
    func unreadable() {
        #expect(FlowPolicy().decide(flow("", 443)) == .leave(.unreadable))
    }

    @Test("an explicit proxy connection is not claimed, and the core's later upstream is not claimed again")
    func explicitProxyDoesNotRecurseOrDuplicate() {
        let policy = FlowPolicy()

        // The client connects to 127.0.0.1:9090. Loopback is excluded, so the
        // provider never sees that flow and cannot capture it a second time.
        #expect(policy.decide(flow("127.0.0.1", 9_090)) == .leave(.loopback))
        #expect(policy.decide(flow("127.0.0.1", 9_091)) == .leave(.loopback))
        // A phone connects to the LAN listener. Port 9090 is not 80 or 443.
        #expect(policy.decide(flow("192.168.1.25", 9_090)) == .leave(.port))

        // The core's first upstream connection is offered once. The bridge
        // answers 508 before it connects anywhere, and the provider learns
        // the process. Later upstream connections are not claimed.
        #expect(policy.decide(flow("93.184.215.14", 443, from: core)) == .bridge)
        policy.learnProxyLab(core)
        #expect(policy.decide(flow("93.184.215.14", 443, from: core)) == .leave(.proxyLab))
        #expect(policy.decide(flow("93.184.215.14", 80, from: core)) == .leave(.proxyLab))

        // Some other app that ignored the system proxy is still captured once.
        #expect(policy.decide(flow("93.184.215.14", 443, from: chrome)) == .bridge)
    }

    @Test("a process the core called its own is never offered again")
    func learnsTheCore() {
        let policy = FlowPolicy()

        #expect(policy.decide(flow("93.184.215.14", 443, from: core)) == .bridge)
        policy.learnProxyLab(core)

        #expect(policy.decide(flow("93.184.215.14", 443, from: core)) == .leave(.proxyLab))
        #expect(policy.decide(flow("93.184.215.14", 80, from: core)) == .leave(.proxyLab))
        // Nobody else is affected.
        #expect(policy.decide(flow("93.184.215.14", 443, from: chrome)) == .bridge)
    }

    @Test("a reused pid is a different process, and is offered")
    func pidReuse() {
        let policy = FlowPolicy()
        policy.learnProxyLab(core)

        let reused = SourceProcess(pid: core.pid, pidVersion: core.pidVersion + 1)

        #expect(policy.decide(flow("93.184.215.14", 443, from: reused)) == .bridge)
    }

    @Test("while the core is unreachable nothing is claimed, and afterwards it is tried again")
    func coreDown() {
        let policy = FlowPolicy()
        let then = Date(timeIntervalSince1970: 1_000)

        policy.coreUnreachable("connection refused", now: then)

        #expect(policy.decide(flow("93.184.215.14", 443), now: then.addingTimeInterval(1)) == .leave(.coreUnavailable))
        #expect(policy.decide(
            flow("93.184.215.14", 443),
            now: then.addingTimeInterval(FlowPolicy.coreRetryInterval + 0.1)
        ) == .bridge)
    }

    @Test("reaching the core ends the wait at once")
    func coreBack() {
        let policy = FlowPolicy()
        let then = Date(timeIntervalSince1970: 1_000)

        policy.coreUnreachable("connection refused", now: then)
        policy.coreReached()

        #expect(policy.decide(flow("93.184.215.14", 443), now: then) == .bridge)
        #expect(policy.status().bridge == .reachable)
    }

    @Test("the status counts where flows went and says why the core was not there")
    func status() {
        let policy = FlowPolicy()
        let then = Date(timeIntervalSince1970: 1_000)

        _ = policy.decide(flow("93.184.215.14", 443), now: then)
        _ = policy.decide(flow("93.184.215.14", 443, from: core), now: then)
        policy.learnProxyLab(core)
        _ = policy.decide(flow("93.184.215.14", 443), now: then)
        policy.coreUnreachable("connection refused", now: then)
        _ = policy.decide(flow("93.184.215.14", 443), now: then)

        let status = policy.status()
        #expect(status.counters == .init(
            bridged: 1,
            passedThrough: 2,
            proxyLab: 1,
            claimedTcp443: 3,
            bypassedProxyLab: 1,
            bridgeFallback: 2
        ))
        #expect(status.bridge == .unreachable)
        #expect(status.lastBridgeFailure == "connection refused")
    }

    @Test("an audit token's pid and pid version are its sixth and eighth words")
    func auditToken() throws {
        let words: [UInt32] = [0, 501, 20, 501, 20, 812, 100_004, 7]
        let token = words.withUnsafeBytes { Data($0) }

        let source = try #require(SourceProcess(auditToken: token))

        #expect(source == SourceProcess(pid: 812, pidVersion: 7))
    }

    @Test("something that is not a token names no process")
    func notAToken() {
        #expect(SourceProcess(auditToken: Data()) == nil)
        #expect(SourceProcess(auditToken: Data(count: 31)) == nil)
    }
}
