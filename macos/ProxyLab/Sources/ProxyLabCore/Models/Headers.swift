//  The headers of a captured message.
//
//  A list of pairs, not a dictionary. A message may carry the same header name
//  more than once — `Set-Cookie` routinely does — and which value came first
//  can be the thing being debugged, so order and repetition are preserved
//  exactly as `proxy-storage` recorded them.
//
//  A value is text when it was valid UTF-8 and raw bytes when it was not. The
//  core makes that distinction on the way in; nothing here re-decodes a value
//  that was already decided to be bytes.
//
//  Header values are where credentials live. They are shown, because that is
//  what a debugging proxy is for, and they are never written to a log.

import Foundation

/// The header section of a captured request or response.
///
/// Transparent on the wire: the JSON is the array of headers itself, which is
/// how `proxy-storage` spells it.
public struct Headers: Codable, Hashable, Sendable {
    /// Every header, in the order the message carried them.
    public let entries: [Header]

    public init(_ entries: [Header] = []) {
        self.entries = entries
    }

    /// How many headers the message carried, repetitions counted separately.
    public var count: Int { entries.count }

    /// Whether the message carried no headers at all.
    public var isEmpty: Bool { entries.isEmpty }

    /// The first value of `name` as text, matched without regard to case.
    ///
    /// Header names are case-insensitive and a captured message spells them
    /// however the sender did.
    public func text(_ name: String) -> String? {
        entries.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value.text
    }

    public init(from decoder: any Decoder) throws {
        entries = try decoder.singleValueContainer().decode([Header].self)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(entries)
    }
}

extension Headers: RandomAccessCollection {
    public var startIndex: Int { entries.startIndex }
    public var endIndex: Int { entries.endIndex }
    public subscript(position: Int) -> Header { entries[position] }
}

/// One header of a captured message.
public struct Header: Codable, Hashable, Sendable {
    /// The name, spelled as the sender spelled it.
    public let name: String
    /// The value, exactly as it arrived.
    public let value: HeaderValue

    public init(name: String, value: HeaderValue) {
        self.name = name
        self.value = value
    }
}

/// The value of one header.
public enum HeaderValue: Codable, Hashable, Sendable {
    /// A value that was valid UTF-8.
    case text(String)
    /// A value that was not, kept as it arrived.
    case bytes(Data)

    /// The value as text, or `nil` when it is not text.
    public var text: String? {
        switch self {
        case let .text(text): text
        case .bytes: nil
        }
    }

    /// How many bytes the value is.
    public var byteCount: Int {
        switch self {
        case let .text(text): text.utf8.count
        case let .bytes(bytes): bytes.count
        }
    }

    private enum CodingKeys: String, CodingKey {
        case text
        case bytes
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        if let text = try container.decodeIfPresent(String.self, forKey: .text) {
            self = .text(text)
        } else if let bytes = try container.decodeIfPresent(Data.self, forKey: .bytes) {
            self = .bytes(bytes)
        } else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a header value is either text or bytes"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .text(text): try container.encode(text, forKey: .text)
        case let .bytes(bytes): try container.encode(bytes, forKey: .bytes)
        }
    }
}
