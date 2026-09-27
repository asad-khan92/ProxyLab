//  A request somebody kept, so they can send it again tomorrow.
//
//      a captured exchange ──▶ Save Request ──▶ SavedRequest ──▶ replay_request
//                                                    ▲
//                                          the editor, where it can be changed
//
//  A Swift mirror of `proxy-storage`'s `SavedRequest`, field for field and
//  name for name — the same rule the traffic model and the rule model follow.
//  The app never decides what a URL, a method, or a header may be: it sends
//  what it has and the core refuses what it will not keep.
//
//  # It is not a captured exchange
//
//  There is no response, no timing, and no record of what happened when it was
//  last sent. What happened is an exchange in the traffic list with an id of
//  its own, because replaying a saved request is the replay every other
//  request gets — see ``ReplayRequest``.
//
//  # The body may be less than the request sent
//
//  A request saved out of captured traffic carries the captured body, which
//  the core's capture limit may have cut short. So the body is a
//  ``BodyCapture`` rather than plain `Data`: it says how long the body was as
//  well as how much of it is here, and the interface says so rather than
//  quietly sending less than the original did.
//
//  # Credentials
//
//  A saved request carries whatever the request carried: `Authorization`, a
//  `Cookie`, an API key. They are kept exactly, because a saved request that
//  dropped the credential would reproduce nothing — and nothing here writes
//  one to a log.

import Foundation

/// One request the core is keeping.
public struct SavedRequest: Codable, Hashable, Sendable, Identifiable {
    /// Which saved request this is.
    public let id: String
    /// What a person calls it.
    public let name: String
    /// The method it will be sent with.
    public let method: String
    /// The absolute URL it will be sent to.
    public let url: String
    /// The headers, in the order they will be sent.
    public let headers: Headers
    /// The body, and how much of one there was.
    public let body: BodyCapture
    /// When it was saved, in milliseconds since the Unix epoch.
    public let createdAtMs: UInt64
    /// When it was last changed, in milliseconds since the Unix epoch.
    public let updatedAtMs: UInt64

    public init(
        id: String,
        name: String,
        method: String,
        url: String,
        headers: Headers = Headers(),
        body: BodyCapture = .empty,
        createdAtMs: UInt64 = 0,
        updatedAtMs: UInt64 = 0
    ) {
        self.id = id
        self.name = name
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
        self.createdAtMs = createdAtMs
        self.updatedAtMs = updatedAtMs
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case name
        case method
        case url
        case headers
        case body
        case createdAtMs = "created_at_ms"
        case updatedAtMs = "updated_at_ms"
    }

    /// The request this describes, ready to be sent again.
    ///
    /// As much of the body as the core kept: a request saved from a capture
    /// that was cut short sends the captured bytes, because the rest was
    /// forwarded and never stored. ``sendsLessThanItWas`` is how the
    /// interface says so.
    public var replayRequest: ReplayRequest {
        ReplayRequest(method: method, url: url, headers: headers, body: body.data)
    }

    /// Whether replaying this sends less than the original request did.
    public var sendsLessThanItWas: Bool { body.truncated }
}

/// A request that has not been saved yet.
///
/// What the app sends to save one: everything but the id and the timestamps,
/// which are the core's to mint. The body is plain `Data` rather than a
/// ``BodyCapture`` because the app cannot be the one to say how long a body
/// *was* — a request being saved carries the body it carries, and the only
/// place a truncated one comes from is a capture the core already holds.
public struct NewSavedRequest: Codable, Hashable, Sendable {
    /// What a person calls it.
    public var name: String
    /// The method it will be sent with.
    public var method: String
    /// The absolute URL it will be sent to.
    public var url: String
    /// The headers, in the order they will be sent.
    public var headers: Headers
    /// The body, which is bytes and need not be text.
    public var body: Data

    public init(
        name: String,
        method: String,
        url: String,
        headers: Headers = Headers(),
        body: Data = Data()
    ) {
        self.name = name
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
    }

    /// The request `request` describes, under `name`.
    public init(name: String, request: ReplayRequest) {
        self.init(
            name: name,
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.body
        )
    }
}

/// A change to a request that is already saved.
///
/// Every field is optional, and one that is not named is not sent — so a
/// rename is one field rather than the whole request read back and written
/// again, which would overwrite anything changed elsewhere in the meantime.
public struct SavedRequestUpdate: Codable, Hashable, Sendable {
    /// What a person calls it.
    public var name: String?
    /// The method it will be sent with.
    public var method: String?
    /// The absolute URL it will be sent to.
    public var url: String?
    /// The headers, in the order they will be sent.
    public var headers: Headers?
    /// The body, which is bytes and need not be text.
    public var body: Data?

    public init(
        name: String? = nil,
        method: String? = nil,
        url: String? = nil,
        headers: Headers? = nil,
        body: Data? = nil
    ) {
        self.name = name
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
    }

    /// An update that only renames.
    public static func rename(_ name: String) -> SavedRequestUpdate {
        SavedRequestUpdate(name: name)
    }

    /// An update that replaces everything but the name.
    public static func request(_ request: ReplayRequest) -> SavedRequestUpdate {
        SavedRequestUpdate(
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.body
        )
    }

    /// Whether this update changes anything at all.
    ///
    /// The core refuses one that does not, so the app does not send one.
    public var isEmpty: Bool {
        name == nil && method == nil && url == nil && headers == nil && body == nil
    }
}

/// Every saved request the core is keeping.
public struct SavedRequestListResult: Codable, Hashable, Sendable {
    /// The requests, in the order they were saved.
    public let savedRequests: [SavedRequest]

    public init(savedRequests: [SavedRequest]) {
        self.savedRequests = savedRequests
    }

    private enum CodingKeys: String, CodingKey {
        case savedRequests = "saved_requests"
    }
}

/// The result of removing one saved request.
public struct SavedRequestDeletedResult: Codable, Hashable, Sendable {
    /// Which request.
    public let savedRequestID: String
    /// Whether there was one to remove.
    public let deleted: Bool

    public init(savedRequestID: String, deleted: Bool) {
        self.savedRequestID = savedRequestID
        self.deleted = deleted
    }

    private enum CodingKeys: String, CodingKey {
        case savedRequestID = "saved_request_id"
        case deleted
    }
}
