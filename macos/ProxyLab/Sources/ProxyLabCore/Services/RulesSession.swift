//  What the rules interface is allowed to ask the core.
//
//  The five rule operations the IPC protocol has, and nothing else. It sits
//  beside ``TrafficSession`` rather than inside it because the two screens ask
//  the core for quite different things and neither needs the other's reach —
//  but they are the same protocol on the same socket, and `IPCClient`
//  implements both, so the app has one connection and not two.
//
//  A protocol for the same reason the traffic one is: the rules model's
//  behaviour is testable without a socket, a server, or a database.

import Foundation

/// The core's rule management, as the interface uses it.
public protocol RulesSession: Sendable {
    /// Every rule, in the order they are consulted in: priority, then age.
    func listRules() async throws -> [Rule]
    /// One rule, as the core holds it now.
    func rule(id: String) async throws -> Rule
    /// Stores a new rule and gives back the one the core minted.
    func createRule(_ rule: NewRule) async throws -> Rule
    /// Changes the fields `update` names, leaving the rest as they were.
    func updateRule(id: String, _ update: RuleUpdate) async throws -> Rule
    /// Removes one rule.
    func deleteRule(id: String) async throws -> RuleDeletedResult
}

extension IPCClient: RulesSession {}
