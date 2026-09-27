//  What the breakpoints interface is allowed to ask the core.
//
//  The breakpoint and throttling operations the IPC protocol has, and nothing
//  else. It sits beside ``TrafficSession``, ``RulesSession`` and
//  ``LanSession`` for the reason they sit beside each other: the screens ask
//  the core for quite different things and none of them needs another's reach
//  — but they are the same protocol on the same socket, and `IPCClient`
//  implements all of them, so the app has one connection and not four.
//
//  A protocol for the same reason the others are: holding an exchange still,
//  editing it, and letting it go are testable without a socket, a server, or
//  any traffic.

import Foundation

/// The core's breakpoints and throttling, as the interface uses them.
public protocol BreakpointsSession: Sendable {
    /// Every configured breakpoint, in the order they are consulted in.
    func listBreakpoints() async throws -> [Breakpoint]
    /// Configures a new breakpoint and gives back the one the core minted.
    func createBreakpoint(_ breakpoint: NewBreakpoint) async throws -> Breakpoint
    /// Changes the fields `update` names, leaving the rest as they were.
    func updateBreakpoint(id: String, _ update: BreakpointUpdate) async throws -> Breakpoint
    /// Removes one breakpoint.
    func deleteBreakpoint(id: String) async throws -> BreakpointDeletedResult

    /// Starts receiving the exchanges breakpoints hold still.
    ///
    /// Until this is asked for, the core pauses nothing at all.
    func watchBreakpoints() async throws
    /// Lets a paused exchange carry on, with whatever it carries changed.
    func continueBreakpoint(
        pauseID: String,
        request: RequestEdit?,
        response: ResponseEdit?,
        websocket: WebSocketEdit?
    ) async throws -> PauseResolvedResult
    /// Stops a paused exchange: nothing is sent, and nothing delivered.
    func cancelBreakpoint(pauseID: String) async throws -> PauseResolvedResult

    /// How the core is shaping the traffic it carries.
    func throttleStatus() async throws -> ThrottleSettings
    /// Shapes traffic this way from the next exchange on.
    func setThrottle(_ settings: ThrottleSettings) async throws -> ThrottleSettings
}

extension BreakpointsSession {
    func continueBreakpoint(
        pauseID: String,
        request: RequestEdit?,
        response: ResponseEdit?
    ) async throws -> PauseResolvedResult {
        try await continueBreakpoint(
            pauseID: pauseID,
            request: request,
            response: response,
            websocket: nil
        )
    }
}

extension IPCClient: BreakpointsSession {}
