//  As much of a body as the core captured, and how much there was.
//
//  A body travelling through the proxy is forwarded in full; the capture is a
//  bounded copy taken on the way past. So `size` is the length of what was
//  forwarded and `data` is what was kept, and the two differ exactly when
//  `truncated` is set.
//
//  The bytes are base64 on the wire because JSON has no way to carry a byte
//  string, and a great many bodies are images, archives, or protocol buffers.
//  Nothing here decodes them as text: see `BodyInspection` for that, which
//  is a display decision and not part of the model.

import Foundation

/// The captured part of one body.
public struct BodyCapture: Codable, Hashable, Sendable {
    /// The bytes that were captured.
    public let data: Data
    /// How long the whole body was, whether or not that much was kept.
    public let size: UInt64
    /// Whether the body was longer than the capture limit.
    public let truncated: Bool

    public init(data: Data, size: UInt64, truncated: Bool) {
        self.data = data
        self.size = size
        self.truncated = truncated
    }

    /// A body with nothing in it.
    public static let empty = BodyCapture(data: Data(), size: 0, truncated: false)

    /// A body captured in full.
    public static func complete(_ data: Data) -> BodyCapture {
        BodyCapture(data: data, size: UInt64(data.count), truncated: false)
    }

    /// Whether there was no body at all.
    public var isEmpty: Bool { size == 0 }

    /// How many bytes of the body were captured.
    public var capturedByteCount: Int { data.count }
}
