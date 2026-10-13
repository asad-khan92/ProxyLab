//  Removing a declared Content-Encoding before a body is read.
//
//      captured bytes ──▶ Content-Encoding ──▶ the bytes a reading uses
//
//  The proxy stores and forwards a response as it arrived. A gzip body stays
//  gzip in the capture, in storage, and on the IPC socket, where JSON carries
//  those bytes as base64 because JSON has no byte string. That base64 is the
//  transport. It is decoded back to the captured bytes before anything here
//  runs, and it is not a property of the body: a body is not inspected by
//  looking at how its characters happen to start.
//
//  This is the only place a content coding is removed. `BodyInspection` asks
//  for it, the view draws that reading, and a save writes the capture
//  unchanged. An encoding this platform cannot inflate, or a stream that does
//  not finish, is left as it was captured. Part of an inflated body is not
//  the body the server sent.
//
//  gzip and deflate are the same DEFLATE stream in different wrappers, and
//  both are inflated with the Compression framework's raw DEFLATE decoder —
//  the format `compression.h` documents for `COMPRESSION_ZLIB`, which is
//  RFC 1951 rather than the zlib wrapper. Brotli is `COMPRESSION_BROTLI`.
//  No compression library is added for any of them.

import Compression
import Foundation

/// A declared `Content-Encoding`, removed for a reading of a captured body.
enum ContentCoding {
    /// How far a captured body may expand while it is being read.
    ///
    /// The capture is already bounded. This stops a small compressed body from
    /// expanding without limit on the way to the inspector. A stream that does
    /// not finish inside the cap is left compressed.
    static let maxDecodedBytes = 8 * 1024 * 1024

    /// What removing `header` did to `data`.
    enum Removal: Equatable {
        /// The message declared no coding, or only `identity`.
        case notEncoded
        /// A declared coding was removed, and `Data` is the result.
        case decoded(Data)
        /// A declared coding could not be removed. The capture is kept.
        case failed
    }

    /// Removes `header` from `data`.
    ///
    /// `truncated` is a capture that does not hold the whole body. A content
    /// coding of a prefix is not inflated: the stream cannot be checked to
    /// its end, and a partial result would be shown as if it were complete.
    /// Codings are listed in the order they were applied, so they are removed
    /// from the outside in. `limit` is ``maxDecodedBytes`` except in tests.
    static func remove(
        _ header: String?,
        from data: Data,
        truncated: Bool,
        limit: Int = maxDecodedBytes
    ) -> Removal {
        guard let header else { return .notEncoded }
        guard let tokens = tokens(in: header) else { return .failed }
        guard !tokens.allSatisfy({ $0 == "identity" }) else { return .notEncoded }
        guard !truncated, !data.isEmpty, limit > 0 else { return .failed }

        var current = data
        for token in tokens.reversed() {
            if token == "identity" { continue }
            guard let next = decode(current, coding: token, limit: limit) else { return .failed }

            current = next
        }

        return .decoded(current)
    }

    // MARK: - The header

    /// The coding tokens in `header`, or `nil` when the list is not one.
    ///
    /// An empty token — a trailing comma, a blank value — is not a coding
    /// this can skip. The whole header is refused and the capture is kept.
    private static func tokens(in header: String) -> [String]? {
        let tokens = header.split(separator: ",", omittingEmptySubsequences: false).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        }
        guard !tokens.isEmpty, tokens.allSatisfy({ !$0.isEmpty }) else { return nil }

        return tokens
    }

    /// `data` with one coding removed, or `nil` if it is not a finished stream
    /// of that coding.
    private static func decode(_ data: Data, coding: String, limit: Int) -> Data? {
        switch coding {
        case "gzip", "x-gzip":
            return decodeGzip(data, limit: limit)
        case "deflate":
            return decodeDeflate(data, limit: limit)
        case "br":
            return inflate(data, algorithm: COMPRESSION_BROTLI, limit: limit)
        default:
            return nil
        }
    }

    // MARK: - gzip

    /// One gzip member: a header, a raw DEFLATE stream, and an eight-byte trailer.
    private struct GzipMember {
        let deflate: Data
        let crc: UInt32
        let isize: UInt32
    }

    /// `data` inflated as gzip, when the header, the stream, and the trailer agree.
    private static func decodeGzip(_ data: Data, limit: Int) -> Data? {
        guard let member = gzipMember(data),
            let inflated = inflate(member.deflate, algorithm: COMPRESSION_ZLIB, limit: limit),
            crc32(inflated) == member.crc,
            UInt64(inflated.count) % (1 << 32) == UInt64(member.isize)
        else {
            return nil
        }

        return inflated
    }

    /// The one gzip member `data` is, or `nil` when the header is not gzip.
    ///
    /// HTTP responses carry one member. A second member concatenated after it
    /// would leave bytes the DEFLATE stream did not consume, and that is
    /// refused rather than returned as the first member alone.
    private static func gzipMember(_ data: Data) -> GzipMember? {
        let bytes = [UInt8](data)
        guard bytes.count >= 18, bytes[0] == 0x1F, bytes[1] == 0x8B, bytes[2] == 8 else {
            return nil
        }

        let flags = bytes[3]
        guard flags & 0xE0 == 0 else { return nil }

        var index = 10
        if flags & 0x04 != 0 {
            guard index + 2 <= bytes.count else { return nil }

            let extra = Int(bytes[index]) | (Int(bytes[index + 1]) << 8)
            index += 2 + extra
        }
        if flags & 0x08 != 0 {
            guard let end = bytes[index...].firstIndex(of: 0) else { return nil }

            index = end + 1
        }
        if flags & 0x10 != 0 {
            guard index <= bytes.count, let end = bytes[index...].firstIndex(of: 0) else {
                return nil
            }

            index = end + 1
        }
        if flags & 0x02 != 0 {
            index += 2
        }
        guard index + 8 <= bytes.count else { return nil }

        let trailer = bytes.count - 8
        guard trailer >= index else { return nil }

        return GzipMember(
            deflate: Data(bytes[index..<trailer]),
            crc: littleEndian(bytes, at: trailer),
            isize: littleEndian(bytes, at: trailer + 4)
        )
    }

    /// Four little-endian bytes of `bytes` at `offset`.
    private static func littleEndian(_ bytes: [UInt8], at offset: Int) -> UInt32 {
        UInt32(bytes[offset])
            | (UInt32(bytes[offset + 1]) << 8)
            | (UInt32(bytes[offset + 2]) << 16)
            | (UInt32(bytes[offset + 3]) << 24)
    }

    // MARK: - deflate

    /// `data` inflated as HTTP `deflate`.
    ///
    /// The coding names the zlib wrapper (RFC 1950). Servers also send the
    /// raw DEFLATE stream the wrapper contains (RFC 1951), and have for as
    /// long as the coding has existed. The wrapper is tried first, and the
    /// raw stream only when the bytes are not that wrapper: a checksum is
    /// what decides the wrapper, not a guess about the payload.
    private static func decodeDeflate(_ data: Data, limit: Int) -> Data? {
        if let wrapped = decodeZlibWrapped(data, limit: limit) { return wrapped }

        return inflate(data, algorithm: COMPRESSION_ZLIB, limit: limit)
    }

    /// `data` inflated as a zlib wrapper, or `nil` when it is not one.
    private static func decodeZlibWrapped(_ data: Data, limit: Int) -> Data? {
        guard data.count >= 8 else { return nil }

        let cmf = UInt(data[data.startIndex])
        let flg = UInt(data[data.index(after: data.startIndex)])
        guard cmf & 0x0F == 8, (cmf * 256 + flg) % 31 == 0, flg & 0x20 == 0 else { return nil }

        let start = data.index(data.startIndex, offsetBy: 2)
        let end = data.index(data.endIndex, offsetBy: -4)
        guard start < end,
            let inflated = inflate(Data(data[start..<end]), algorithm: COMPRESSION_ZLIB, limit: limit)
        else {
            return nil
        }

        var checksum: UInt32 = 0
        for byte in data.suffix(4) {
            checksum = (checksum << 8) | UInt32(byte)
        }
        guard checksum == adler32(inflated) else { return nil }

        return inflated
    }

    // MARK: - DEFLATE and brotli

    /// `source` inflated with `algorithm`, or `nil` unless the stream finishes
    /// inside `limit` and consumes every byte.
    ///
    /// The buffer decoder is not used. It reports a full destination both when
    /// the output fitted exactly and when it was cut off, and those two are
    /// not the same reading.
    private static func inflate(
        _ source: Data,
        algorithm: compression_algorithm,
        limit: Int
    ) -> Data? {
        guard !source.isEmpty, limit > 0 else { return nil }

        let stream = UnsafeMutablePointer<compression_stream>.allocate(capacity: 1)
        defer { stream.deallocate() }
        guard
            compression_stream_init(stream, COMPRESSION_STREAM_DECODE, algorithm)
                == COMPRESSION_STATUS_OK
        else {
            return nil
        }
        defer { compression_stream_destroy(stream) }

        var output = Data()
        let chunk = 16 * 1024
        var destination = [UInt8](repeating: 0, count: chunk)

        let complete = source.withUnsafeBytes { raw -> Bool in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return false }

            stream.pointee.src_ptr = base
            stream.pointee.src_size = source.count

            while output.count < limit {
                let room = min(chunk, limit - output.count)
                let sourceBefore = stream.pointee.src_size
                let status: compression_status = destination.withUnsafeMutableBytes { dst in
                    guard let dstBase = dst.bindMemory(to: UInt8.self).baseAddress else {
                        return COMPRESSION_STATUS_ERROR
                    }

                    stream.pointee.dst_ptr = dstBase
                    stream.pointee.dst_size = room

                    return compression_stream_process(
                        stream,
                        Int32(COMPRESSION_STREAM_FINALIZE.rawValue)
                    )
                }
                let produced = room - stream.pointee.dst_size
                if produced > 0 {
                    output.append(contentsOf: destination.prefix(produced))
                }

                switch status {
                case COMPRESSION_STATUS_END:
                    return stream.pointee.src_size == 0
                case COMPRESSION_STATUS_OK:
                    let consumed = stream.pointee.src_size < sourceBefore
                    if produced == 0, !consumed { return false }
                default:
                    return false
                }
            }

            return false
        }

        return complete ? output : nil
    }

    /// CRC-32 of `data`, as a gzip trailer stores it.
    private static func crc32(_ data: Data) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in data {
            crc ^= UInt32(byte)
            for _ in 0..<8 {
                crc = (crc & 1) == 1 ? (0xEDB8_8320 ^ (crc >> 1)) : (crc >> 1)
            }
        }

        return ~crc
    }

    /// Adler-32 of `data`, as a zlib trailer stores it.
    private static func adler32(_ data: Data) -> UInt32 {
        var a: UInt32 = 1
        var b: UInt32 = 0
        for byte in data {
            a = (a + UInt32(byte)) % 65521
            b = (b + a) % 65521
        }

        return (b << 16) | a
    }
}
