//  The client against the real core.
//
//  Everything else in this package runs with nothing installed. This one talks
//  to a running `proxy-engine` over its actual socket, and is skipped unless
//  one is pointed at:
//
//      PROXYLAB_IPC_SOCKET=~/.proxylab/run/proxylab.sock swift test
//
//  It is deliberately read-only except for one delete of an exchange it
//  created itself. A test that cleared a developer's captured traffic to prove
//  that clearing works would be a test that costs more than it is worth; the
//  clear operation is covered against a scripted server instead.

import Foundation
import Testing

@testable import ProxyLabCore

/// The socket a running core is listening on, if a test run named one.
private let runningCoreSocket: String? = {
    guard
        let path = ProcessInfo.processInfo.environment[IPCSocketLocation.environmentVariable],
        !path.isEmpty,
        FileManager.default.fileExists(atPath: (path as NSString).expandingTildeInPath)
    else {
        return nil
    }

    return (path as NSString).expandingTildeInPath
}()

@Suite(
    "Core integration",
    .enabled(if: runningCoreSocket != nil, "set PROXYLAB_IPC_SOCKET to a running core's socket"),
    .serialized
)
struct CoreIntegrationTests {
    private func connectedClient() async throws -> IPCClient {
        let client = IPCClient.overSocket(at: try #require(runningCoreSocket))
        try await client.connect()

        return client
    }

    @Test("the real core answers a list request with a page this app can read")
    func listTraffic() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        let page = try await client.listTraffic(ListTrafficParameters(limit: 5))

        #expect(page.exchanges.count <= 5)
        #expect(page.total >= UInt64(page.exchanges.count))
    }

    @Test("the real core answers a subscription, and events decode")
    func subscribe() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        try await client.subscribe()
    }

    @Test("a real stored exchange decodes whole")
    func getTraffic() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        let page = try await client.listTraffic(ListTrafficParameters(limit: 1))
        guard let summary = page.exchanges.first else {
            // Nothing captured yet; there is nothing to fetch and that is not
            // a failure of the client.
            return
        }

        let exchange = try await client.traffic(id: summary.id)

        #expect(exchange.id == summary.id)
        #expect(exchange.request.method == summary.method)
        #expect(exchange.request.url == summary.url)
        #expect(exchange.response?.status == summary.status)
    }

    @Test("an exchange that is not there is refused with not_found")
    func missingExchange() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        do {
            _ = try await client.traffic(id: "00000000-0000-7000-8000-000000000000")
            Issue.record("an exchange that was never stored should not be found")
        } catch let error as IPCError {
            #expect(error.serverCode == .notFound)
        }
    }

    @Test("an exchange id that is not an id is refused before it reaches storage")
    func invalidExchangeID() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        do {
            _ = try await client.traffic(id: "not-an-id")
            Issue.record("a malformed id should be refused")
        } catch let error as IPCError {
            #expect(error.serverCode == .invalidExchangeID)
        }
    }

    @Test("the real core lists its rules in the order it consults them")
    func listRules() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        let rules = try await client.listRules()

        // Priority ascending, then id: the order the core consults them in is
        // the order it sends them in, and the list must not need sorting here.
        #expect(rules == rules.sorted { ($0.priority, $0.id) < ($1.priority, $1.id) })
    }

    @Test("a rule written here is read back, changed, and removed again")
    func ruleLifecycle() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        // A rule for a host nothing resolves, disabled from the moment it
        // exists, so a developer's proxy is never changed by a test run.
        let created = try await client.createRule(
            NewRule(
                enabled: false,
                name: "proxylab swift integration test",
                matcher: RuleMatcher(host: .exact("integration-test.invalid")),
                action: .block(BlockAction(status: 451)),
                priority: 30_000
            )
        )

        defer { Task { _ = try? await client.deleteRule(id: created.id) } }

        #expect(!created.enabled)
        #expect(created.action == .block(BlockAction(status: 451)))
        #expect(created.matcher.host == .exact("integration-test.invalid"))

        let read = try await client.rule(id: created.id)
        #expect(read == created)

        let renamed = try await client.updateRule(
            id: created.id,
            RuleUpdate(name: "proxylab swift integration test (renamed)")
        )
        #expect(renamed.name == "proxylab swift integration test (renamed)")
        // A field the update did not name is a field the core left alone.
        #expect(renamed.matcher == created.matcher)
        #expect(renamed.action == created.action)
        #expect(renamed.priority == created.priority)

        let deleted = try await client.deleteRule(id: created.id)
        #expect(deleted.deleted)
        #expect(try await client.listRules().contains { $0.id == created.id } == false)
    }

    @Test("a map-remote destination the core cannot speak to is refused when it is written")
    func refusedRule() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        do {
            _ = try await client.createRule(
                NewRule(
                    name: "proxylab swift integration test",
                    action: .mapRemote(RemoteTarget(scheme: .https, host: ""))
                )
            )
            Issue.record("a destination with no host should be refused")
        } catch let error as IPCError {
            #expect(error.serverCode == .invalidRule)
        }
    }

    @Test("a rule that is not there is refused with not_found")
    func missingRule() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        do {
            _ = try await client.rule(id: "00000000-0000-7000-8000-000000000000")
            Issue.record("a rule that was never stored should not be found")
        } catch let error as IPCError {
            #expect(error.serverCode == .notFound)
        }
    }

    @Test("a rule id that is not an id is refused before it reaches storage")
    func invalidRuleID() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        do {
            _ = try await client.rule(id: "not-an-id")
            Issue.record("a malformed rule id should be refused")
        } catch let error as IPCError {
            #expect(error.serverCode == .invalidRuleID)
        }
    }

    @Test("two subscriptions on one connection are refused, as the protocol says")
    func doubleSubscribe() async throws {
        let client = try await connectedClient()
        defer { Task { await client.disconnect() } }

        try await client.subscribe()

        do {
            try await client.subscribe()
            Issue.record("a second subscription on one connection should be refused")
        } catch let error as IPCError {
            #expect(error.serverCode == .alreadySubscribed)
        }
    }
}
