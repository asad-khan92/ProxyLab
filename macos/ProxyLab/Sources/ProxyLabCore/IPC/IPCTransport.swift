//  The socket, behind a door the tests can also open.
//
//  Everything above this protocol — correlation, framing, decoding, the view
//  model — is exercised without a socket at all, because a transport is the
//  only part that needs one. `UnixSocketTransport` is the real one.
//
//  A transport carries bytes and knows nothing about the protocol: no lines,
//  no JSON, no request ids. What it hands back from `open` is whatever the
//  peer sent, in whatever sized pieces it arrived in.

import Foundation

/// A byte stream to the core, opened once and read until it ends.
public protocol IPCTransport: Sendable {
    /// Connects, and returns the bytes the peer sends.
    ///
    /// The stream finishes when the peer closes the connection, and throws
    /// when the connection fails.
    func open() async throws -> AsyncThrowingStream<Data, any Error>

    /// Writes `data` in full.
    func send(_ data: Data) async throws

    /// Closes the connection, ending the stream `open` returned.
    func close() async
}
