//  How slow the core is pretending its own link is.
//
//      {"enabled":true,"latency_ms":200,
//       "download_bytes_per_second":131072,"upload_bytes_per_second":32768}
//
//  Three numbers and a switch, mirrored field for field from `core/ipc`. A
//  rate of `0` is *no ceiling* rather than *no bytes*, which is how one
//  direction is left alone while the other is shaped.
//
//  Held in the running core's memory and written nowhere, so a core that has
//  just started carries traffic at the speed of the network it is on.

import Foundation

/// The longest delay the core will add to an exchange.
public let maxThrottleLatencyMs: UInt64 = 60_000

/// The slowest ceiling the core will keep, below which nothing finishes.
public let minThrottleBytesPerSecond: UInt64 = 256

/// How the core is shaping the traffic it carries.
public struct ThrottleSettings: Codable, Sendable, Equatable {
    /// Whether any of it is being applied.
    ///
    /// Held separately from the numbers so that switching throttling off and
    /// on again does not mean retyping them.
    public var enabled: Bool
    /// How long each exchange waits before its request is sent. `0` for none.
    public var latencyMs: UInt64
    /// The ceiling on a response body coming back. `0` for no ceiling.
    public var downloadBytesPerSecond: UInt64
    /// The ceiling on a request body going upstream. `0` for no ceiling.
    public var uploadBytesPerSecond: UInt64

    public init(
        enabled: Bool = false,
        latencyMs: UInt64 = 0,
        downloadBytesPerSecond: UInt64 = 0,
        uploadBytesPerSecond: UInt64 = 0
    ) {
        self.enabled = enabled
        self.latencyMs = latencyMs
        self.downloadBytesPerSecond = downloadBytesPerSecond
        self.uploadBytesPerSecond = uploadBytesPerSecond
    }

    /// Throttling as a core that has just started has it.
    public static let none = ThrottleSettings()

    /// Whether anything about this shapes traffic at all.
    ///
    /// Switched on with every number at zero is not throttling; it is
    /// throttling that has been asked for and has nothing to do.
    public var isShaping: Bool {
        enabled && (latencyMs > 0 || downloadBytesPerSecond > 0 || uploadBytesPerSecond > 0)
    }

    /// Why the core would refuse these settings, or `nil` if it would not.
    ///
    /// The same bounds the core enforces, checked here so the editor can say
    /// so while the numbers are being typed. It is not what decides: the core
    /// checks them again.
    public var refusal: String? {
        if latencyMs > maxThrottleLatencyMs {
            return "Added latency must be at most \(maxThrottleLatencyMs / 1000) seconds."
        }

        for rate in [downloadBytesPerSecond, uploadBytesPerSecond]
        where rate > 0 && rate < minThrottleBytesPerSecond {
            return "A bandwidth limit must be 0 for unlimited, or at least "
                + "\(minThrottleBytesPerSecond) bytes per second."
        }

        return nil
    }

    private enum CodingKeys: String, CodingKey {
        case enabled
        case latencyMs = "latency_ms"
        case downloadBytesPerSecond = "download_bytes_per_second"
        case uploadBytesPerSecond = "upload_bytes_per_second"
    }
}
