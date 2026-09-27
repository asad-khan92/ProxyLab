//  The rule half of the wire protocol, in both directions.
//
//  What a rule request must carry, and what a rule response must be read as.
//  The shapes here are the ones `core/rules` serializes and `core/ipc` parses:
//  an externally tagged matcher, an internally tagged action, and a destination
//  in parts rather than as a URL.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Rule request encoding")
struct RuleRequestEncodingTests {
    private let encoder = JSONEncoder()

    private func encoded(_ operation: IPCOperation, id: String = "r1") throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: id, operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("the rule operation names are the ones the server implements")
    func operationNames() {
        #expect(IPCOperation.listRules.name == "list_rules")
        #expect(IPCOperation.getRule(ruleID: "x").name == "get_rule")
        #expect(IPCOperation.createRule(RuleFixtures.newBlockRule).name == "create_rule")
        #expect(IPCOperation.updateRule(ruleID: "x", RuleUpdate()).name == "update_rule")
        #expect(IPCOperation.deleteRule(ruleID: "x").name == "delete_rule")
    }

    @Test("listing rules takes no parameters, so none are sent")
    func listTakesNothing() throws {
        // The server refuses a parameter it does not know, and a key whose
        // value is null is still a key.
        #expect(try Set(encoded(.listRules).keys) == ["version", "request_id", "type"])
    }

    @Test("a request naming one rule sends its id and nothing else")
    func ruleID() throws {
        for operation in [
            IPCOperation.getRule(ruleID: "0199c0de-0000-7000-8000-00000000010a"),
            .deleteRule(ruleID: "0199c0de-0000-7000-8000-00000000010a"),
        ] {
            let json = try encoded(operation)

            #expect(Set(json.keys) == ["version", "request_id", "type", "rule_id"])
            #expect(json["rule_id"] as? String == "0199c0de-0000-7000-8000-00000000010a")
        }
    }

    @Test("a created rule is sent flattened, as the server parses it")
    func createFlattened() throws {
        let json = try encoded(.createRule(RuleFixtures.newBlockRule))

        #expect(
            Set(json.keys)
                == ["version", "request_id", "type", "enabled", "name", "matcher", "action",
                    "priority"]
        )
        #expect(json["name"] as? String == "block analytics")
        #expect(json["priority"] as? Int == 10)
        #expect(json["enabled"] as? Bool == true)
    }

    @Test("a matcher is sent with one key per criterion that is set")
    func matcherEncoding() throws {
        let json = try encoded(.createRule(RuleFixtures.newBlockRule))
        let matcher = try #require(json["matcher"] as? [String: Any])

        #expect(matcher["host"] as? [String: String] == ["exact": "analytics.example.com"])
        #expect(matcher["path"] as? [String: String] == ["prefix": "/collect"])
        #expect(matcher["method"] as? [String: String] == ["exact": "POST"])
    }

    @Test("a criterion that is not set is not sent at all")
    func matcherOmitsUnset() throws {
        let rule = NewRule(name: "any", matcher: .any, action: .allow)
        let json = try encoded(.createRule(rule))
        let matcher = try #require(json["matcher"] as? [String: Any])

        #expect(matcher.isEmpty)
    }

    @Test("an action is sent internally tagged, with its own fields beside the tag")
    func actionEncoding() throws {
        let cases: [(RuleAction, [String: Any])] = [
            (.allow, ["type": "allow"]),
            (.block(BlockAction(status: 503)), ["type": "block", "status": 503]),
        ]

        for (action, expected) in cases {
            let json = try encoded(.createRule(NewRule(name: "r", action: action)))
            let encoded = try #require(json["action"] as? [String: Any])

            #expect(encoded["type"] as? String == expected["type"] as? String)
            if let status = expected["status"] as? Int {
                #expect(encoded["status"] as? Int == status)
            }
        }
    }

    @Test("a map-local action sends the path and omits a content type it has none of")
    func mapLocalEncoding() throws {
        let action = RuleAction.mapLocal(MapLocalAction(path: "/tmp/fixture.json"))
        let json = try encoded(.createRule(NewRule(name: "r", action: action)))
        let encoded = try #require(json["action"] as? [String: Any])

        #expect(Set(encoded.keys) == ["type", "path", "status"])
        #expect(encoded["path"] as? String == "/tmp/fixture.json")
        #expect(encoded["status"] as? Int == 200)
    }

    @Test("a map-remote action sends the destination in parts, never as a URL")
    func mapRemoteEncoding() throws {
        let target = try RemoteTarget.parse("https://staging.example.com:8443/v2?debug=1")
        let json = try encoded(.createRule(NewRule(name: "r", action: .mapRemote(target))))
        let action = try #require(json["action"] as? [String: Any])
        let encoded = try #require(action["target"] as? [String: Any])

        #expect(action["type"] as? String == "map_remote")
        #expect(encoded["scheme"] as? String == "https")
        #expect(encoded["host"] as? String == "staging.example.com")
        #expect(encoded["port"] as? Int == 8443)
        #expect(encoded["path"] as? String == "/v2")
        #expect(encoded["query"] as? String == "debug=1")
    }

    @Test("an update sends the rule id and only the fields it changes")
    func updateSendsOnlyChanges() throws {
        let json = try encoded(.updateRule(ruleID: "r", RuleUpdate(enabled: false)))

        #expect(Set(json.keys) == ["version", "request_id", "type", "rule_id", "enabled"])
        #expect(json["enabled"] as? Bool == false)
    }

    @Test("an update carrying several fields sends all of them and no others")
    func updateSendsSeveral() throws {
        let update = RuleUpdate(name: "renamed", priority: 3)
        let json = try encoded(.updateRule(ruleID: "r", update))

        #expect(
            Set(json.keys) == ["version", "request_id", "type", "rule_id", "name", "priority"]
        )
        #expect(json["name"] as? String == "renamed")
        #expect(json["priority"] as? Int == 3)
    }
}

@Suite("Rule response decoding")
struct RuleResponseDecodingTests {
    private let decoder = JSONDecoder()

    private func message(_ json: String) throws -> IPCMessage {
        try decoder.decode(IPCMessage.self, from: Data(json.utf8))
    }

    private func result(_ json: String) throws -> IPCResult {
        guard case let .response(response) = try message(json), let result = response.result else {
            throw DecodingFailure.notAResult
        }

        return result
    }

    private enum DecodingFailure: Error { case notAResult }

    @Test("a rule list decodes in the order the core sent it")
    func listDecoding() throws {
        let json = RuleFixtures.listResponse(
            requestID: "r1",
            rules: [
                RuleFixtures.allowRuleJSON,
                RuleFixtures.blockRuleJSON,
                RuleFixtures.mapLocalRuleJSON,
            ]
        )

        guard case let .listRules(list) = try result(json) else {
            Issue.record("expected a rule list")
            return
        }

        #expect(list.rules.map(\.name) == ["except the health check", "block analytics", "mock the config"])
        #expect(list.rules.map(\.priority) == [5, 10, 20])
    }

    @Test("an empty rule list is a list and not a failure")
    func emptyList() throws {
        guard case let .listRules(list) = try result(RuleFixtures.listResponse(requestID: "r1", rules: []))
        else {
            Issue.record("expected a rule list")
            return
        }

        #expect(list.rules.isEmpty)
    }

    @Test("a block rule decodes every field the core sent")
    func blockDecoding() throws {
        guard
            case let .getRule(rule) = try result(
                RuleFixtures.ruleResponse(requestID: "r1", type: "get_rule")
            )
        else {
            Issue.record("expected a rule")
            return
        }

        #expect(rule.id == "0199c0de-0000-7000-8000-00000000010a")
        #expect(rule.name == "block analytics")
        #expect(rule.enabled)
        #expect(rule.priority == 10)
        #expect(rule.matcher.host == .exact("analytics.example.com"))
        #expect(rule.matcher.path == .prefix("/collect"))
        #expect(rule.matcher.method == .exact("POST"))
        #expect(rule.action == .block(BlockAction(status: 403)))
        #expect(rule.createdAtMs == 1_700_000_000_000)
        #expect(rule.updatedAtMs == 1_700_000_000_000)
    }

    @Test("a map-local rule with no content type decodes as having none")
    func mapLocalDecoding() throws {
        guard
            case let .createRule(rule) = try result(
                RuleFixtures.ruleResponse(
                    requestID: "r1",
                    type: "create_rule",
                    rule: RuleFixtures.mapLocalRuleJSON
                )
            )
        else {
            Issue.record("expected a rule")
            return
        }

        #expect(!rule.enabled)
        #expect(
            rule.action
                == .mapLocal(MapLocalAction(path: "/Users/dev/mocks/config.json", status: 200))
        )
        guard case let .mapLocal(map) = rule.action else {
            Issue.record("expected a map-local action")
            return
        }

        #expect(map.contentType == nil)
    }

    @Test("a map-remote rule decodes the destination the core keeps in parts")
    func mapRemoteDecoding() throws {
        guard
            case let .updateRule(rule) = try result(
                RuleFixtures.ruleResponse(
                    requestID: "r1",
                    type: "update_rule",
                    rule: RuleFixtures.mapRemoteRuleJSON
                )
            )
        else {
            Issue.record("expected a rule")
            return
        }

        guard case let .mapRemote(target) = rule.action else {
            Issue.record("expected a map-remote action")
            return
        }

        #expect(target.scheme == .https)
        #expect(target.host == "staging.example.com")
        #expect(target.port == 8443)
        #expect(target.path == "/v2")
        #expect(target.query == "debug=1")
        #expect(target.url == "https://staging.example.com:8443/v2?debug=1")
    }

    @Test("an allow rule decodes, so a rule list holding one can still be shown")
    func allowDecoding() throws {
        guard
            case let .getRule(rule) = try result(
                RuleFixtures.ruleResponse(
                    requestID: "r1",
                    type: "get_rule",
                    rule: RuleFixtures.allowRuleJSON
                )
            )
        else {
            Issue.record("expected a rule")
            return
        }

        #expect(rule.action == .allow)
        #expect(rule.matcher.host == nil)
        #expect(rule.matcher.method == nil)
    }

    @Test("a delete result says which rule, and whether there was one")
    func deleteDecoding() throws {
        guard
            case let .deleteRule(deleted) = try result(
                RuleFixtures.deleteResponse(requestID: "r1", id: "rule-a", deleted: false)
            )
        else {
            Issue.record("expected a delete result")
            return
        }

        #expect(deleted.ruleID == "rule-a")
        #expect(!deleted.deleted)
    }

    @Test("a rule refusal carries the code the client matches on")
    func refusalDecoding() throws {
        let json = Fixtures.errorResponse(
            requestID: "r1",
            code: "invalid_rule",
            message: "a map-remote destination must be http or https"
        )

        guard case let .response(response) = try message(json) else {
            Issue.record("expected a response")
            return
        }

        #expect(response.error?.code == .invalidRule)
        #expect(response.error?.message == "a map-remote destination must be http or https")
    }

    @Test("an action this build cannot carry out is a protocol disagreement")
    func unknownAction() {
        let rule = """
            {"id":"0199c0de-0000-7000-8000-00000000010a","enabled":true,"name":"n",\
            "matcher":{},"action":{"type":"rewrite_headers"},"priority":0,\
            "created_at_ms":1,"updated_at_ms":1}
            """

        #expect(throws: (any Error).self) {
            try result(RuleFixtures.ruleResponse(requestID: "r1", type: "get_rule", rule: rule))
        }
    }

    @Test("a matcher shape this build cannot read is refused rather than guessed at")
    func unknownMatcher() {
        let rule = """
            {"id":"0199c0de-0000-7000-8000-00000000010a","enabled":true,"name":"n",\
            "matcher":{"host":{"regex":"api\\\\..*"}},"action":{"type":"allow"},"priority":0,\
            "created_at_ms":1,"updated_at_ms":1}
            """

        #expect(throws: (any Error).self) {
            try result(RuleFixtures.ruleResponse(requestID: "r1", type: "get_rule", rule: rule))
        }
    }

    @Test("a rule survives a round trip through the encoder and back")
    func roundTrip() throws {
        for json in [
            RuleFixtures.blockRuleJSON,
            RuleFixtures.mapLocalRuleJSON,
            RuleFixtures.mapRemoteRuleJSON,
            RuleFixtures.allowRuleJSON,
        ] {
            let rule = try JSONDecoder().decode(Rule.self, from: Data(json.utf8))
            let encoded = try JSONEncoder().encode(rule)
            let again = try JSONDecoder().decode(Rule.self, from: encoded)

            #expect(rule == again)
        }
    }
}
