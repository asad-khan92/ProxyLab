//  A WebSocket message or SSE event, as the core spells it on the wire.
//
//  Summaries travel on the event stream and on a list page. The payload is
//  asked for by id, the same split the traffic list already has, so a busy
//  socket cannot fill the IPC pipe with megabytes.

import Foundation

/// Which way a stream message travelled.
public enum StreamDirection: String, Codable, Sendable, Equatable {
    /// Client → server.
    case clientToServer = "client_to_server"
    /// Server → client.
    case serverToClient = "server_to_client"
}

/// What kind of stream message this is.
public enum StreamKind: String, Codable, Sendable, Equatable {
    case websocketText = "websocket_text"
    case websocketBinary = "websocket_binary"
    case sse
}

/// A stream message without its payload, for a list or a live event.
public struct StreamMessageSummary: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public let exchangeID: String
    public let sequence: UInt64
    public let capturedAt: Timestamp
    public let direction: StreamDirection
    public let kind: StreamKind
    public let eventName: String?
    public let eventID: String?
    public let retryMs: UInt64?
    public let payloadSize: UInt64
    public let payloadCapturedLen: UInt64
    public let payloadTruncated: Bool

    public init(
        id: String,
        exchangeID: String,
        sequence: UInt64,
        capturedAt: Timestamp,
        direction: StreamDirection,
        kind: StreamKind,
        eventName: String? = nil,
        eventID: String? = nil,
        retryMs: UInt64? = nil,
        payloadSize: UInt64,
        payloadCapturedLen: UInt64,
        payloadTruncated: Bool
    ) {
        self.id = id
        self.exchangeID = exchangeID
        self.sequence = sequence
        self.capturedAt = capturedAt
        self.direction = direction
        self.kind = kind
        self.eventName = eventName
        self.eventID = eventID
        self.retryMs = retryMs
        self.payloadSize = payloadSize
        self.payloadCapturedLen = payloadCapturedLen
        self.payloadTruncated = payloadTruncated
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case exchangeID = "exchange_id"
        case sequence
        case capturedAt = "captured_at"
        case direction
        case kind
        case eventName = "event_name"
        case eventID = "event_id"
        case retryMs = "retry_ms"
        case payloadSize = "payload_size"
        case payloadCapturedLen = "payload_captured_len"
        case payloadTruncated = "payload_truncated"
    }
}

/// A page of stream messages, oldest first.
public struct StreamMessageListResult: Codable, Sendable, Equatable {
    public let messages: [StreamMessageSummary]
    public let nextAfter: UInt64?

    public init(messages: [StreamMessageSummary], nextAfter: UInt64? = nil) {
        self.messages = messages
        self.nextAfter = nextAfter
    }

    private enum CodingKeys: String, CodingKey {
        case messages
        case nextAfter = "next_after"
    }
}

/// One stream message, payload included.
public struct StreamMessage: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public let exchangeID: String
    public let sequence: UInt64
    public let capturedAt: Timestamp
    public let direction: StreamDirection
    public let kind: StreamKind
    public let eventName: String?
    public let eventID: String?
    public let retryMs: UInt64?
    public let payload: BodyCapture

    public init(
        id: String,
        exchangeID: String,
        sequence: UInt64,
        capturedAt: Timestamp,
        direction: StreamDirection,
        kind: StreamKind,
        eventName: String? = nil,
        eventID: String? = nil,
        retryMs: UInt64? = nil,
        payload: BodyCapture
    ) {
        self.id = id
        self.exchangeID = exchangeID
        self.sequence = sequence
        self.capturedAt = capturedAt
        self.direction = direction
        self.kind = kind
        self.eventName = eventName
        self.eventID = eventID
        self.retryMs = retryMs
        self.payload = payload
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case exchangeID = "exchange_id"
        case sequence
        case capturedAt = "captured_at"
        case direction
        case kind
        case eventName = "event_name"
        case eventID = "event_id"
        case retryMs = "retry_ms"
        case payload
    }
}

/// One stream message as `get_stream_message` carries it.
struct StreamMessageDetail: Codable, Sendable, Equatable {
    let message: StreamMessage
}

extension StreamDirection {
    /// What the inspector calls this direction.
    public var title: String {
        switch self {
        case .clientToServer: "Client → Server"
        case .serverToClient: "Server → Client"
        }
    }
}

extension StreamKind {
    /// The shortest true word for this kind of message.
    public var title: String {
        switch self {
        case .websocketText: "Text"
        case .websocketBinary: "Binary"
        case .sse: "Event"
        }
    }
}

extension StreamMessageSummary {
    /// The line a timeline row shows first.
    public var headline: String {
        switch kind {
        case .sse:
            let name = eventName.flatMap { $0.isEmpty ? nil : $0 } ?? "message"

            return name
        case .websocketText, .websocketBinary:
            return kind.title
        }
    }
}

extension TrafficExchange {
    /// Whether this exchange is a WebSocket handshake.
    public var isWebSocketHandshake: Bool {
        response?.status == 101
            && response?.headers.text("upgrade")?.caseInsensitiveCompare("websocket")
                == .orderedSame
    }

    /// Whether this exchange's response is an event stream.
    public var isEventStream: Bool {
        guard let type = response?.headers.text("content-type") else { return false }

        return type.split(separator: ";").first?
            .trimmingCharacters(in: .whitespaces)
            .caseInsensitiveCompare("text/event-stream") == .orderedSame
    }

    /// Whether the inspector should offer a message timeline.
    public var carriesStreamMessages: Bool { isWebSocketHandshake || isEventStream }
}
