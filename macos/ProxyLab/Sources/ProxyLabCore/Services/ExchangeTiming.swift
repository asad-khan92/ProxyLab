//  The clock the core kept, presented as the clock the core kept.
//
//      started ─────────────────────────────────────────────▶
//              │ connect        │ head                │ completed
//              └── to head ─────┴──── body ───────────┘
//
//  Three marks are recorded by `proxy-storage` and three are shown: when the
//  proxy began opening its upstream connection, when the origin's response head
//  arrived, and when the last of the body reached the client. Every one of them
//  is a measurement and every one of them is optional.
//
//  There are no DNS, TCP, or TLS phases here, because the core does not measure
//  them. A phase bar with invented segments in it would read exactly like one
//  with measured segments in it, which is the reason this file would rather
//  show three rows than five.

import Foundation

/// The timing of one exchange, as far as it was measured.
public enum ExchangeTiming {
    /// A moment the core recorded, as an offset from the request arriving.
    public struct Mark: Equatable, Sendable, Identifiable {
        /// What happened.
        public let name: String
        /// How long after the request arrived it happened.
        public let micros: UInt64

        public init(name: String, micros: UInt64) {
            self.name = name
            self.micros = micros
        }

        public var id: String { name }
    }

    /// A stretch of time between two marks.
    public struct Span: Equatable, Sendable, Identifiable {
        /// What was happening.
        public let name: String
        /// How long it took.
        public let micros: UInt64

        public init(name: String, micros: UInt64) {
            self.name = name
            self.micros = micros
        }

        public var id: String { name }
    }

    /// The moments of `timing`, in the order they happened.
    ///
    /// An intercepted HTTPS request has no upstream connect: its connection was
    /// opened by the `CONNECT` that came before it, and that mark is left out
    /// rather than reported as zero.
    public static func marks(of timing: Timing) -> [Mark] {
        var marks: [Mark] = []

        if let connect = timing.upstreamConnectMicros {
            marks.append(Mark(name: "Upstream connect", micros: connect))
        }
        if let head = timing.upstreamResponseMicros {
            marks.append(Mark(name: "Response head", micros: head))
        }
        if let completed = timing.completedMicros {
            marks.append(Mark(name: "Completed", micros: completed))
        }

        return marks
    }

    /// The stretches of `timing` that were measured at both ends.
    ///
    /// Two at most: everything up to the response head, and the body after it.
    /// An exchange that never got a response head has neither, and is left
    /// with its total alone rather than with one span covering everything.
    public static func spans(of timing: Timing) -> [Span] {
        guard let head = timing.upstreamResponseMicros else { return [] }

        var spans = [Span(name: "To response head", micros: head)]

        if let completed = timing.completedMicros, completed >= head {
            spans.append(Span(name: "Response body", micros: completed - head))
        }

        return spans
    }

    /// How long the whole exchange took, if it finished.
    public static func total(of timing: Timing) -> UInt64? { timing.completedMicros }

    /// A duration in microseconds, at a scale a person reads.
    public static func duration(micros: UInt64) -> String {
        TrafficDisplay.duration(milliseconds: Double(micros) / 1_000) ?? "—"
    }
}
