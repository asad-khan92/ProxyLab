//  The canonical record of one HTTP exchange, as the core spells it.
//
//  These are Swift mirrors of `proxy-storage`'s own types, field for field and
//  name for name. They are deliberately not a second model: the IPC protocol
//  carries the traffic model's serialized form, so a change to it is a change
//  to the protocol, and these structures are how that change would show up
//  here as a compile error rather than as a silently missing field.
//
//  Coding keys are written out rather than derived from a key strategy. This
//  is a wire format shared with another language; being able to read the exact
//  spelling of every key beside its Swift name is worth the lines.

import Foundation

/// How the client reached the proxy.
public enum Scheme: String, Codable, Sendable {
    /// A plaintext request, forwarded to the origin.
    case http
    /// A request read out of an intercepted TLS session.
    case https
}

/// The server an exchange was for.
public struct Destination: Codable, Hashable, Sendable {
    /// The host, without the brackets an IPv6 literal wears in a URL.
    public let host: String
    /// The port the proxy connected to.
    public let port: UInt16

    public init(host: String, port: UInt16) {
        self.host = host
        self.port = port
    }
}

/// The HTTP version a message was framed with.
///
/// Decoded leniently: a version this build has no name for becomes
/// ``unknown`` rather than failing the whole exchange, which is the same thing
/// the core does with a version it did not recognise.
public enum HTTPVersion: String, Codable, Sendable {
    case http09 = "HTTP/0.9"
    case http10 = "HTTP/1.0"
    case http11 = "HTTP/1.1"
    case http2 = "HTTP/2.0"
    case http3 = "HTTP/3.0"
    case unknown

    public init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = HTTPVersion(rawValue: raw) ?? .unknown
    }
}

/// What kind of failure ended an exchange.
///
/// A wrapper around the name rather than a closed enum: the core may learn a
/// new kind, and an exchange that cannot be shown because of a name this build
/// has not heard of would be a worse outcome than showing it with its own
/// summary, which is the part a person reads.
public struct TrafficErrorKind: RawRepresentable, Codable, Hashable, Sendable {
    public let rawValue: String

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    public static let requestRejected = TrafficErrorKind(rawValue: "request_rejected")
    public static let proxyFailure = TrafficErrorKind(rawValue: "proxy_failure")
    public static let upstreamConnect = TrafficErrorKind(rawValue: "upstream_connect")
    public static let upstreamTLS = TrafficErrorKind(rawValue: "upstream_tls")
    public static let clientTLS = TrafficErrorKind(rawValue: "client_tls")
    public static let upstreamHTTP = TrafficErrorKind(rawValue: "upstream_http")
    public static let responseForwarding = TrafficErrorKind(rawValue: "response_forwarding")
    public static let clientDisconnected = TrafficErrorKind(rawValue: "client_disconnected")
    public static let incomplete = TrafficErrorKind(rawValue: "incomplete")
}

/// Why an exchange has no response.
public struct TrafficError: Codable, Hashable, Sendable {
    /// What kind of failure it was.
    public let kind: TrafficErrorKind
    /// What happened, in the core's own words. Safe to show.
    public let summary: String

    public init(kind: TrafficErrorKind, summary: String) {
        self.kind = kind
        self.summary = summary
    }
}

/// The connection an exchange happened on.
public struct ConnectionInfo: Codable, Hashable, Sendable {
    /// The address the client connected from.
    public let client: String
    /// Whether the exchange was plaintext or inside a TLS session.
    public let scheme: Scheme
    /// Where the proxy sent it, when that was worked out.
    public let destination: Destination?

    public init(client: String, scheme: Scheme, destination: Destination?) {
        self.client = client
        self.scheme = scheme
        self.destination = destination
    }
}

/// The request half of an exchange.
public struct CapturedRequest: Codable, Hashable, Sendable {
    /// The method, as it was sent.
    public let method: String
    /// The absolute URL the request was for, assembled by the proxy.
    public let url: String
    /// The version the request was framed with.
    public let version: HTTPVersion
    /// The headers, in the order they arrived.
    public let headers: Headers
    /// As much of the body as the capture limit allowed.
    public let body: BodyCapture

    public init(
        method: String,
        url: String,
        version: HTTPVersion,
        headers: Headers = Headers(),
        body: BodyCapture = .empty
    ) {
        self.method = method
        self.url = url
        self.version = version
        self.headers = headers
        self.body = body
    }
}

/// The response half of an exchange.
public struct CapturedResponse: Codable, Hashable, Sendable {
    /// The status code the origin answered with.
    public let status: UInt16
    /// The reason phrase, when there is one to give.
    public let reason: String?
    /// The version the response was framed with.
    public let version: HTTPVersion
    /// The headers, in the order they arrived.
    public let headers: Headers
    /// As much of the body as the capture limit allowed.
    public let body: BodyCapture

    public init(
        status: UInt16,
        reason: String? = nil,
        version: HTTPVersion,
        headers: Headers = Headers(),
        body: BodyCapture = .empty
    ) {
        self.status = status
        self.reason = reason
        self.version = version
        self.headers = headers
        self.body = body
    }
}

/// One complete request/response lifecycle, as the proxy saw it.
public struct TrafficExchange: Codable, Hashable, Sendable, Identifiable {
    /// Identifies this exchange for as long as it is kept.
    public let id: String
    /// The connection it happened on.
    public let connection: ConnectionInfo
    /// What the client asked for.
    public let request: CapturedRequest
    /// What came back, if anything did.
    public let response: CapturedResponse?
    /// Why nothing came back, when nothing did.
    public let error: TrafficError?
    /// When it started and how long its parts took.
    public let timing: Timing

    public init(
        id: String,
        connection: ConnectionInfo,
        request: CapturedRequest,
        response: CapturedResponse? = nil,
        error: TrafficError? = nil,
        timing: Timing
    ) {
        self.id = id
        self.connection = connection
        self.request = request
        self.response = response
        self.error = error
        self.timing = timing
    }
}

/// One exchange as the traffic list shows it.
///
/// Everything a row needs and nothing it does not: no headers and no bodies,
/// because a page of these is what the list draws and an event carrying one is
/// sent for every request the proxy carries.
public struct ExchangeSummary: Codable, Hashable, Sendable, Identifiable {
    /// Which exchange.
    public let id: String
    /// When the proxy received the request.
    public let startedAt: Timestamp
    /// When the response head arrived, if one did.
    public let responseAt: Timestamp?
    /// How long the whole exchange took, if it finished.
    public let durationMicros: UInt64?
    /// The address the client connected from.
    public let client: String
    /// Whether it was plaintext or inside an intercepted TLS session.
    public let scheme: Scheme
    /// Where it was going, when that was worked out.
    public let destination: Destination?
    /// The request's method.
    public let method: String
    /// The request's absolute URL.
    public let url: String
    /// The version the request was framed with.
    public let version: HTTPVersion
    /// The status the origin answered with, if it answered.
    public let status: UInt16?
    /// The reason phrase that came with it.
    public let reason: String?
    /// The version the response was framed with.
    public let responseVersion: HTTPVersion?
    /// The request `Content-Type`, when the message declared one.
    public let requestContentType: String?
    /// The response `Content-Type`, when the message declared one.
    public let responseContentType: String?
    /// Whether the exchange ran to completion.
    public let completed: Bool
    /// Why there is no response, when there is none.
    public let error: TrafficError?

    public init(
        id: String,
        startedAt: Timestamp,
        responseAt: Timestamp? = nil,
        durationMicros: UInt64? = nil,
        client: String,
        scheme: Scheme,
        destination: Destination? = nil,
        method: String,
        url: String,
        version: HTTPVersion,
        status: UInt16? = nil,
        reason: String? = nil,
        responseVersion: HTTPVersion? = nil,
        requestContentType: String? = nil,
        responseContentType: String? = nil,
        completed: Bool,
        error: TrafficError? = nil
    ) {
        self.id = id
        self.startedAt = startedAt
        self.responseAt = responseAt
        self.durationMicros = durationMicros
        self.client = client
        self.scheme = scheme
        self.destination = destination
        self.method = method
        self.url = url
        self.version = version
        self.status = status
        self.reason = reason
        self.responseVersion = responseVersion
        self.requestContentType = requestContentType
        self.responseContentType = responseContentType
        self.completed = completed
        self.error = error
    }

    /// Whether a response came back and nothing went wrong.
    public var succeeded: Bool { status != nil && error == nil }

    /// How long the exchange took, in milliseconds, if it finished.
    public var durationMilliseconds: Double? {
        durationMicros.map { Double($0) / 1_000 }
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case startedAt = "started_at"
        case responseAt = "response_at"
        case durationMicros = "duration_micros"
        case client
        case scheme
        case destination
        case method
        case url
        case version
        case status
        case reason
        case responseVersion = "response_version"
        case requestContentType = "request_content_type"
        case responseContentType = "response_content_type"
        case completed
        case error
    }
}
