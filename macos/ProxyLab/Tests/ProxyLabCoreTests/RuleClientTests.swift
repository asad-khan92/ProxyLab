//  Rule operations across the client, with a test on the other end of the
//  socket.
//
//  Everything between the model and the wire: the request that actually goes
//  out, the answer that comes back, a refusal that becomes an error the model
//  can match on, and a connection that ends while a rule request is waiting.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Rules over the client")
struct RuleClientTests {
    private func connected() async throws -> (IPCClient, FakeTransport) {
        let transport = FakeTransport()
        let client = IPCClient(makeTransport: { transport })
        try await client.connect()

        return (client, transport)
    }

    /// The line the client wrote, as the server would read it.
    private func sent(_ transport: FakeTransport) async throws -> [String: Any] {
        let line = await transport.nextSent()
        let object = try JSONSerialization.jsonObject(with: line)

        return try #require(object as? [String: Any])
    }

    @Test("listing rules writes one line and reads the rules back out of it")
    func listRules() async throws {
        let (client, transport) = try await connected()

        async let listing = client.listRules()
        let request = try await sent(transport)
        let id = try #require(request["request_id"] as? String)
        await transport.deliver(
            RuleFixtures.listResponse(
                requestID: id,
                rules: [RuleFixtures.blockRuleJSON, RuleFixtures.mapRemoteRuleJSON]
            )
        )

        let rules = try await listing

        #expect(request["type"] as? String == "list_rules")
        #expect(request["version"] as? Int == 1)
        #expect(rules.map(\.name) == ["block analytics", "point at staging"])
    }

    @Test("creating a rule sends its fields and gives back the one the core minted")
    func createRule() async throws {
        let (client, transport) = try await connected()

        async let creating = client.createRule(RuleFixtures.newBlockRule)
        let request = try await sent(transport)
        let id = try #require(request["request_id"] as? String)
        await transport.deliver(RuleFixtures.ruleResponse(requestID: id, type: "create_rule"))

        let created = try await creating

        #expect(request["type"] as? String == "create_rule")
        #expect(request["name"] as? String == "block analytics")
        #expect(created.id == "0199c0de-0000-7000-8000-00000000010a")
        #expect(created.action == .block(BlockAction(status: 403)))
    }

    @Test("updating a rule sends its id and the one field being changed")
    func updateRule() async throws {
        let (client, transport) = try await connected()

        async let updating = client.updateRule(id: "rule-a", RuleUpdate(enabled: false))
        let request = try await sent(transport)
        let id = try #require(request["request_id"] as? String)
        await transport.deliver(RuleFixtures.ruleResponse(requestID: id, type: "update_rule"))

        _ = try await updating

        #expect(request["type"] as? String == "update_rule")
        #expect(request["rule_id"] as? String == "rule-a")
        #expect(request["enabled"] as? Bool == false)
        #expect(request["name"] == nil)
        #expect(request["priority"] == nil)
    }

    @Test("deleting a rule says which rule, and whether there was one")
    func deleteRule() async throws {
        let (client, transport) = try await connected()

        async let deleting = client.deleteRule(id: "rule-a")
        let request = try await sent(transport)
        let id = try #require(request["request_id"] as? String)
        await transport.deliver(
            RuleFixtures.deleteResponse(requestID: id, id: "rule-a", deleted: true)
        )

        let deleted = try await deleting

        #expect(request["type"] as? String == "delete_rule")
        #expect(deleted.ruleID == "rule-a")
        #expect(deleted.deleted)
    }

    @Test("one rule is read back by its id")
    func getRule() async throws {
        let (client, transport) = try await connected()

        async let getting = client.rule(id: "0199c0de-0000-7000-8000-00000000010a")
        let request = try await sent(transport)
        let id = try #require(request["request_id"] as? String)
        await transport.deliver(RuleFixtures.ruleResponse(requestID: id, type: "get_rule"))

        #expect(try await getting.name == "block analytics")
        #expect(request["rule_id"] as? String == "0199c0de-0000-7000-8000-00000000010a")
    }

    @Test("a rule the core refused comes back as an error carrying its code")
    func refusal() async throws {
        let (client, transport) = try await connected()

        async let creating = client.createRule(RuleFixtures.newBlockRule)
        let id = try await transport.nextRequestID()
        await transport.deliver(
            Fixtures.errorResponse(
                requestID: id,
                code: "invalid_rule",
                message: "a map-remote destination must be http or https"
            )
        )

        do {
            _ = try await creating
            Issue.record("expected a refusal")
        } catch let error as IPCError {
            #expect(error.serverCode == .invalidRule)
            #expect(error.message == "a map-remote destination must be http or https")
        }
    }

    @Test("a rule that is not there comes back as not found, for the model to act on")
    func notFound() async throws {
        let (client, transport) = try await connected()

        async let updating = client.updateRule(id: "rule-a", RuleUpdate(enabled: true))
        let id = try await transport.nextRequestID()
        await transport.deliver(
            Fixtures.errorResponse(
                requestID: id,
                code: "not_found",
                message: "there is no rule with that id"
            )
        )

        do {
            _ = try await updating
            Issue.record("expected a refusal")
        } catch let error as IPCError {
            #expect(error.serverCode == .notFound)
        }
    }

    @Test("a core that goes away leaves no rule request waiting for an answer")
    func disconnected() async throws {
        let (client, transport) = try await connected()

        async let listing = client.listRules()
        _ = try await transport.nextRequestID()
        await transport.hangUp()

        do {
            _ = try await listing
            Issue.record("expected the request to fail")
        } catch let error as IPCError {
            #expect(error == .connectionClosed)
        }
    }

    @Test("a rule request made with no connection is refused rather than hanging")
    func notConnected() async {
        let client = IPCClient(makeTransport: { FakeTransport() })

        await #expect(throws: IPCError.notConnected) { _ = try await client.listRules() }
    }

    @Test("a rule response this build cannot read ends the connection rather than being guessed at")
    func malformed() async throws {
        let (client, transport) = try await connected()

        async let listing = client.listRules()
        let id = try await transport.nextRequestID()
        // A rule list carrying an action the pipeline has no answer for: the
        // version said the two builds agreed about the schema, and they do not.
        await transport.deliver(
            """
            {"version":1,"request_id":"\(id)","ok":true,"type":"list_rules",\
            "result":{"rules":[{"id":"x","enabled":true,"name":"n","matcher":{},\
            "action":{"type":"rewrite"},"priority":0,"created_at_ms":1,"updated_at_ms":1}]}}
            """
        )

        do {
            _ = try await listing
            Issue.record("expected the response to be refused")
        } catch let error as IPCError {
            #expect(error == .malformedMessage)
        }
    }

    @Test("a rule event is not a thing: rule changes are answers, not events")
    func noRuleEvents() async throws {
        let (client, transport) = try await connected()

        var notifications = client.notifications.makeAsyncIterator()

        // The core does not send these, and a client that invented a meaning
        // for one would be a client that disagreed with it. An unknown event
        // ends the connection, which is what every other unknown message does.
        await transport.deliver(#"{"version":1,"type":"rule_added","rule":{}}"#)

        guard case let .connectionLost(error) = await notifications.next() else {
            Issue.record("expected the connection to end")
            return
        }

        #expect(error == .malformedMessage)
    }
}
