//  The rule JSON the core actually sends, and the rules behind it.
//
//  Written out by hand, in the shape `core/ipc` writes it: a test that built
//  its input with the encoder under test would agree with itself no matter what
//  the core sent.

import Foundation

@testable import ProxyLabCore

enum RuleFixtures {
    /// A block rule as `list_rules` and `create_rule` carry it.
    static let blockRuleJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000010a",\
        "enabled":true,\
        "name":"block analytics",\
        "matcher":{"host":{"exact":"analytics.example.com"},\
        "path":{"prefix":"/collect"},\
        "method":{"exact":"POST"}},\
        "action":{"type":"block","status":403},\
        "priority":10,\
        "created_at_ms":1700000000000,\
        "updated_at_ms":1700000000000}
        """

    /// A map-local rule, with the content type the core may work out itself
    /// left off entirely rather than sent as null.
    static let mapLocalRuleJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000010b",\
        "enabled":false,\
        "name":"mock the config",\
        "matcher":{"host":{"exact":"api.example.com"},"path":{"exact":"/config"}},\
        "action":{"type":"map_local","path":"/Users/dev/mocks/config.json","status":200},\
        "priority":20,\
        "created_at_ms":1700000001000,\
        "updated_at_ms":1700000002000}
        """

    /// A map-remote rule, whose destination arrives already in parts.
    static let mapRemoteRuleJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000010c",\
        "enabled":true,\
        "name":"point at staging",\
        "matcher":{"host":{"exact":"api.example.com"}},\
        "action":{"type":"map_remote",\
        "target":{"scheme":"https","host":"staging.example.com","port":8443,\
        "path":"/v2","query":"debug=1"}},\
        "priority":30,\
        "created_at_ms":1700000003000,\
        "updated_at_ms":1700000003000}
        """

    /// An allow rule, which carries no configuration of its own.
    static let allowRuleJSON = """
        {"id":"0199c0de-0000-7000-8000-00000000010d",\
        "enabled":true,\
        "name":"except the health check",\
        "matcher":{"path":{"exact":"/health"}},\
        "action":{"type":"allow"},\
        "priority":5,\
        "created_at_ms":1700000004000,\
        "updated_at_ms":1700000004000}
        """

    /// A successful `list_rules` response carrying `rules`.
    static func listResponse(requestID: String, rules: [String] = [blockRuleJSON]) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"list_rules",\
        "result":{"rules":[\(rules.joined(separator: ","))]}}
        """
    }

    /// A successful response carrying one rule, for whichever operation.
    static func ruleResponse(
        requestID: String,
        type: String,
        rule: String = blockRuleJSON
    ) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"\(type)",\
        "result":{"rule":\(rule)}}
        """
    }

    /// A successful `delete_rule` response.
    static func deleteResponse(requestID: String, id: String, deleted: Bool = true) -> String {
        """
        {"version":1,"request_id":"\(requestID)","ok":true,"type":"delete_rule",\
        "result":{"rule_id":"\(id)","deleted":\(deleted)}}
        """
    }

    // MARK: - Rules built here

    /// The rule a create request carries, matching ``blockRuleJSON``.
    static let newBlockRule = NewRule(
        name: "block analytics",
        matcher: RuleMatcher(
            host: .exact("analytics.example.com"),
            path: .prefix("/collect"),
            method: .exact("POST")
        ),
        action: .block(BlockAction()),
        priority: 10
    )


    /// A rule with everything but the parts a test cares about fixed.
    static func rule(
        id: String,
        name: String = "a rule",
        enabled: Bool = true,
        priority: Int32 = 0,
        matcher: RuleMatcher = .any,
        action: RuleAction = .block(BlockAction())
    ) -> Rule {
        Rule(
            id: id,
            enabled: enabled,
            name: name,
            matcher: matcher,
            action: action,
            priority: priority,
            createdAtMs: 1_700_000_000_000,
            updatedAtMs: 1_700_000_000_000
        )
    }

    /// Three rules at spread-out priorities, in the order the core lists them.
    static func ordered() -> [Rule] {
        [
            rule(id: "rule-a", name: "first", priority: 0),
            rule(id: "rule-b", name: "second", priority: 10),
            rule(id: "rule-c", name: "third", priority: 20),
        ]
    }

    /// A refusal of a rule, in the core's own words.
    static func refusal(_ message: String, code: IPCErrorCode = .invalidRule) -> IPCError {
        .server(IPCErrorBody(code: code, message: message))
    }

    /// The refusal the core sends when a rule is not there.
    static let notFound = IPCError.server(
        IPCErrorBody(code: .notFound, message: "there is no rule with that id")
    )
}
