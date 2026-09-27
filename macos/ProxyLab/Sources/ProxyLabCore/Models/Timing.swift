//  When an exchange happened, and how long its parts took.
//
//  The Swift mirror of `proxy-storage`'s `Timing` and `Timestamp`. A timestamp
//  is seconds and nanoseconds since the Unix epoch, exactly as the core writes
//  it, so nothing is lost turning it into a `Date` and nothing has to be
//  guessed turning it back.
//
//  Durations are microsecond offsets from the moment the request arrived, and
//  every one of them is optional: an exchange that never reached an origin has
//  no upstream response, and one that was abandoned has no completion.

import Foundation

/// A point on the wall clock, as the core records it.
public struct Timestamp: Codable, Hashable, Sendable {
    /// Whole seconds since 1970-01-01T00:00:00Z.
    public let seconds: UInt64
    /// Nanoseconds within that second.
    public let nanos: UInt32

    public init(seconds: UInt64, nanos: UInt32) {
        self.seconds = seconds
        self.nanos = nanos
    }

    /// The same moment as a `Date`, which is what a view formats.
    public var date: Date {
        Date(timeIntervalSince1970: Double(seconds) + Double(nanos) / 1_000_000_000)
    }

    /// Milliseconds since the epoch, which is how the list is ordered.
    public var milliseconds: UInt64 {
        seconds.multipliedReportingOverflow(by: 1_000).partialValue
            + UInt64(nanos / 1_000_000)
    }
}

/// How long the parts of one exchange took.
public struct Timing: Codable, Hashable, Sendable {
    /// When the proxy received the request.
    public let startedAt: Timestamp
    /// When the proxy began opening its connection to the origin.
    ///
    /// `nil` for an intercepted HTTPS request: its connection was opened by
    /// the `CONNECT` that came before the request.
    public let upstreamConnectMicros: UInt64?
    /// When the origin's response head arrived.
    public let upstreamResponseMicros: UInt64?
    /// When the response body finished being forwarded to the client.
    public let completedMicros: UInt64?

    public init(
        startedAt: Timestamp,
        upstreamConnectMicros: UInt64? = nil,
        upstreamResponseMicros: UInt64? = nil,
        completedMicros: UInt64? = nil
    ) {
        self.startedAt = startedAt
        self.upstreamConnectMicros = upstreamConnectMicros
        self.upstreamResponseMicros = upstreamResponseMicros
        self.completedMicros = completedMicros
    }

    /// How long the whole exchange took, in milliseconds, if it finished.
    public var durationMilliseconds: Double? {
        completedMicros.map { Double($0) / 1_000 }
    }

    /// How long the origin took to start answering, in milliseconds.
    public var upstreamResponseMilliseconds: Double? {
        upstreamResponseMicros.map { Double($0) / 1_000 }
    }

    private enum CodingKeys: String, CodingKey {
        case startedAt = "started_at"
        case upstreamConnectMicros = "upstream_connect_micros"
        case upstreamResponseMicros = "upstream_response_micros"
        case completedMicros = "completed_micros"
    }
}
