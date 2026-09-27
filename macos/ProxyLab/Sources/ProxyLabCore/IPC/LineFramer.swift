//  Turning a stream of bytes back into messages.
//
//  The protocol is one JSON object per line, and a socket does not deliver
//  lines: a read may bring half a message, three messages, or three and a
//  half. So everything read goes through here, which holds what is incomplete
//  and hands back what is whole.
//
//  The limit is not decoration. A server that never sends a newline would
//  otherwise grow this buffer until the app ran out of memory, so a line past
//  the limit is a failure rather than something to keep collecting.

import Foundation

/// Splits a byte stream into newline-delimited messages.
struct LineFramer {
    /// The longest line this client will buffer, in bytes.
    ///
    /// The largest legitimate message is one exchange: two bodies at the
    /// core's capture limit of 1 MiB each, base64-encoded, plus headers. That
    /// is a few megabytes, so 32 MiB is room to spare many times over while
    /// still bounding what a broken server can make the app hold.
    static let maximumLineLength = 32 * 1024 * 1024

    private var buffer = Data()
    private let limit: Int

    init(limit: Int = LineFramer.maximumLineLength) {
        self.limit = limit
    }

    /// Adds what was just read, and returns every complete line it finished.
    ///
    /// Empty lines are dropped rather than returned: they carry no message.
    mutating func append(_ data: Data) throws -> [Data] {
        buffer.append(data)

        var lines: [Data] = []
        while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
            let line = buffer[buffer.startIndex..<newline]
            // Re-based rather than left as a slice: a `Data` slice keeps
            // the original index base, and an offset that only shows up
            // after the first line is a bug that hides.
            buffer = Data(buffer[buffer.index(after: newline)...])

            if !line.isEmpty {
                lines.append(Data(line))
            }
        }

        // Only what is left unfinished is measured: a run of complete lines
        // longer than the limit is not a problem, because none of it is held.
        guard buffer.count <= limit else {
            buffer = Data()
            throw IPCError.messageTooLarge
        }

        return lines
    }
}
