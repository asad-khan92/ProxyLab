//  The breakpoint model, as `core/ipc` spells it on the wire.
//
//      {"id":"…","enabled":true,"direction":"request",
//       "matcher":{"host":{"exact":"api.example.com"}}}
//
//  Mirrored field for field, because the core sends a breakpoint as its own
//  type and refuses a field it does not know. Nothing here decides anything:
//  what is matched, what is held still, and what an edit does are all settled
//  in Rust, and this is the shape that travels between the two.
//
//  # A breakpoint is not a rule
//
//  A rule is a standing decision the core makes by itself on every matching
//  request. A breakpoint decides nothing: it stops, shows the exchange, and
//  waits to be told. They share ``RuleMatcher`` because "this host, this path,
//  this method" means one thing in ProxyLab, and nothing else.
//
//  # Nothing is kept
//
//  Breakpoints live in the running core's memory and are written nowhere, so a
//  core that has just started is holding none. The app reads the list when it
//  connects and does not cache one across runs.

import Foundation

/// The most of a body the core will show, and the most an edit may carry back.
///
/// A body longer than this is shown truncated and cannot be replaced: the
/// bytes on screen are not all the bytes there are. Continuing without a body
/// edit forwards the original in full, however long it is.
public let maxBreakpointBodyBytes = 32 * 1024

/// Which half of an exchange a breakpoint stops.
public enum BreakpointDirection: String, Codable, Sendable, Equatable, CaseIterable {
    /// Before the request is sent upstream.
    case request
    /// After the response has arrived and before the client is given it.
    case response
    /// A text WebSocket message, before it continues in its direction.
    case websocket
}

/// One configured breakpoint.
public struct Breakpoint: Codable, Sendable, Equatable, Identifiable {
    /// Which breakpoint this is, as the core minted it.
    public let id: String
    /// Whether it is consulted at all.
    public var enabled: Bool
    /// Which half of an exchange it stops.
    public var direction: BreakpointDirection
    /// Which exchanges it is for. A criterion that is not set matches
    /// anything.
    public var matcher: RuleMatcher

    public init(
        id: String,
        enabled: Bool,
        direction: BreakpointDirection,
        matcher: RuleMatcher
    ) {
        self.id = id
        self.enabled = enabled
        self.direction = direction
        self.matcher = matcher
    }
}

/// A breakpoint that has not been configured yet.
///
/// Everything but the id, which is the core's to mint.
public struct NewBreakpoint: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var direction: BreakpointDirection
    public var matcher: RuleMatcher

    public init(
        enabled: Bool = true,
        direction: BreakpointDirection,
        matcher: RuleMatcher = .any
    ) {
        self.enabled = enabled
        self.direction = direction
        self.matcher = matcher
    }
}

/// A change to a breakpoint that already exists.
///
/// Every field is optional and one that is not named is not changed, which is
/// what lets "turn this off" be one field rather than the whole breakpoint
/// read back and sent again.
public struct BreakpointUpdate: Codable, Sendable, Equatable {
    public var enabled: Bool?
    public var direction: BreakpointDirection?
    public var matcher: RuleMatcher?

    public init(
        enabled: Bool? = nil,
        direction: BreakpointDirection? = nil,
        matcher: RuleMatcher? = nil
    ) {
        self.enabled = enabled
        self.direction = direction
        self.matcher = matcher
    }

    /// An update that only turns a breakpoint on or off.
    public static func enabled(_ enabled: Bool) -> BreakpointUpdate {
        BreakpointUpdate(enabled: enabled)
    }

    /// Whether this update changes anything at all.
    public var isEmpty: Bool { enabled == nil && direction == nil && matcher == nil }
}

// MARK: - What is being held

/// One exchange the core is holding still.
public struct PausedExchange: Codable, Sendable, Equatable, Identifiable {
    /// Which pause this is, and what a decision about it names.
    public let pauseID: String
    /// Which breakpoint stopped it.
    public let breakpointID: String
    /// Which half is being held.
    public let direction: BreakpointDirection
    /// The request's method, which labels a paused response too.
    public let method: String
    /// The request's URL, for the same.
    public let url: String
    /// The request as it stands, when the request is what is held.
    public let request: PausedRequest?
    /// The response as the origin sent it, when the response is what is held.
    public let response: PausedResponse?
    /// The text WebSocket message, when a WebSocket message is what is held.
    public let websocket: PausedWebSocketMessage?

    public var id: String { pauseID }

    public init(
        pauseID: String,
        breakpointID: String,
        direction: BreakpointDirection,
        method: String,
        url: String,
        request: PausedRequest? = nil,
        response: PausedResponse? = nil,
        websocket: PausedWebSocketMessage? = nil
    ) {
        self.pauseID = pauseID
        self.breakpointID = breakpointID
        self.direction = direction
        self.method = method
        self.url = url
        self.request = request
        self.response = response
        self.websocket = websocket
    }

    private enum CodingKeys: String, CodingKey {
        case pauseID = "pause_id"
        case breakpointID = "breakpoint_id"
        case direction
        case method
        case url
        case request
        case response
        case websocket
    }
}

/// A request that has not been sent yet.
public struct PausedRequest: Codable, Sendable, Equatable {
    public let method: String
    public let url: String
    public let headers: Headers
    /// As much of the body as the core will show. A truncated body cannot be
    /// edited — see ``maxBreakpointBodyBytes``.
    public let body: BodyCapture

    public init(method: String, url: String, headers: Headers, body: BodyCapture) {
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
    }
}

/// A response that has arrived and has not been delivered.
public struct PausedResponse: Codable, Sendable, Equatable {
    public let status: UInt16
    public let headers: Headers
    /// As much of the body as the core will show. A truncated body cannot be
    /// edited.
    public let body: BodyCapture

    public init(status: UInt16, headers: Headers, body: BodyCapture) {
        self.status = status
        self.headers = headers
        self.body = body
    }
}

/// A text WebSocket message that has not been forwarded yet.
public struct PausedWebSocketMessage: Codable, Sendable, Equatable {
    public let direction: StreamDirection
    public let payload: BodyCapture

    public init(direction: StreamDirection, payload: BodyCapture) {
        self.direction = direction
        self.payload = payload
    }
}

// MARK: - What to change

/// What to change about a paused request before it is sent.
///
/// A field that is not named is not changed. Leaving `body` out forwards the
/// body the client sent, in full — which is the only thing that can be done
/// with a body too long to have been shown.
public struct RequestEdit: Codable, Sendable, Equatable {
    public var method: String?
    public var url: String?
    public var headers: Headers?
    public var body: Data?

    public init(
        method: String? = nil,
        url: String? = nil,
        headers: Headers? = nil,
        body: Data? = nil
    ) {
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
    }

    /// Whether this edit changes anything at all.
    public var isEmpty: Bool {
        method == nil && url == nil && headers == nil && body == nil
    }
}

/// What to change about a paused response before it is delivered.
public struct ResponseEdit: Codable, Sendable, Equatable {
    public var status: UInt16?
    public var headers: Headers?
    public var body: Data?

    public init(status: UInt16? = nil, headers: Headers? = nil, body: Data? = nil) {
        self.status = status
        self.headers = headers
        self.body = body
    }

    /// Whether this edit changes anything at all.
    public var isEmpty: Bool { status == nil && headers == nil && body == nil }
}

/// What to change about a paused text WebSocket message before it continues.
public struct WebSocketEdit: Codable, Sendable, Equatable {
    public var payload: Data?

    public init(payload: Data? = nil) {
        self.payload = payload
    }

    public var isEmpty: Bool { payload == nil }
}

/// Why a pause ended.
public enum ReleaseReason: String, Codable, Sendable, Equatable {
    /// A client said to carry on.
    case continued
    /// A client said to stop it.
    case cancelled
    /// Nobody was left to answer, so it carried on unchanged.
    case abandoned
    /// Nobody answered in time, so it carried on unchanged.
    case timedOut = "timed_out"
}

// MARK: - Results

/// Every breakpoint there is, in the order they are consulted.
public struct BreakpointListResult: Codable, Sendable, Equatable {
    public let breakpoints: [Breakpoint]

    public init(breakpoints: [Breakpoint]) {
        self.breakpoints = breakpoints
    }
}

/// The result of removing one breakpoint.
public struct BreakpointDeletedResult: Codable, Sendable, Equatable {
    public let breakpointID: String
    public let deleted: Bool

    public init(breakpointID: String, deleted: Bool) {
        self.breakpointID = breakpointID
        self.deleted = deleted
    }

    private enum CodingKeys: String, CodingKey {
        case breakpointID = "breakpoint_id"
        case deleted
    }
}

/// The result of deciding what happens to a paused exchange.
///
/// `resolved` is false for a pause that was already over — one whose deadline
/// passed, or one released because the last watcher went away. Not an error:
/// there was simply nothing left to decide about.
public struct PauseResolvedResult: Codable, Sendable, Equatable {
    public let pauseID: String
    public let resolved: Bool

    public init(pauseID: String, resolved: Bool) {
        self.pauseID = pauseID
        self.resolved = resolved
    }

    private enum CodingKeys: String, CodingKey {
        case pauseID = "pause_id"
        case resolved
    }
}
