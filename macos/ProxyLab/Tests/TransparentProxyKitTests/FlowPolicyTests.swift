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

    private func flow(_ host: String, _ port: UInt16, from source: SourceProcess? = nil) -> FlowCandidate {
        FlowCandidate(host: host, port: port, source: source ?? chrome)
    }

    @Test("HTTP and HTTPS to the outside are handed to the core")
    func bridgesWeb() {
        let policy = FlowPolicy()

        #expect(policy.decide(flow("93.184.215.14", 443)) == .bridge)
        #expect(policy.decide(flow("93.184.215.14", 80)) == .bridge)
        #expect(policy.decide(flow("2606:2800:220:1::248", 443)) == .bridge)
        #expect(policy.decide(flow("example.com", 443)) == .bridge)
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
        #expect(status.counters == .init(bridged: 1, passedThrough: 2, proxyLab: 1))
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
