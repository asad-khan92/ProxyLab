//  What the saved requests interface is allowed to ask the core.
//
//  The five saved-request operations the IPC protocol has, plus the one that
//  keeps a captured exchange's request. It sits beside ``TrafficSession`` and
//  ``RulesSession`` rather than inside either, for the reason those two are
//  apart: this is one small store with a lifetime of its own, and neither of
//  the others needs its reach.
//
//  Replaying a saved request is deliberately *not* here. A replay is
//  ``TrafficSession/replay(_:)`` — the one every request goes through — and a
//  second way in would be a second replay path to keep in step with the rules,
//  the capture, and the storage. What this protocol offers is the request; what
//  sends it is the traffic session.
//
//  A protocol for the same reason the others are: the model's behaviour is
//  testable without a socket, a server, or a database.

import Foundation

/// The core's saved requests, as the interface uses them.
public protocol SavedRequestsSession: Sendable {
    /// Every saved request, in the order they were saved.
    func listSavedRequests() async throws -> [SavedRequest]
    /// One saved request, as the core holds it now.
    func savedRequest(id: String) async throws -> SavedRequest
    /// Keeps a request this app has described.
    func createSavedRequest(_ request: NewSavedRequest) async throws -> SavedRequest
    /// Keeps the request of one captured exchange, under `name`.
    func saveRequest(fromExchange id: String, name: String) async throws -> SavedRequest
    /// Changes the fields `update` names, leaving the rest as they were.
    func updateSavedRequest(id: String, _ update: SavedRequestUpdate) async throws -> SavedRequest
    /// Removes one saved request.
    func deleteSavedRequest(id: String) async throws -> SavedRequestDeletedResult
}

extension IPCClient: SavedRequestsSession {}
