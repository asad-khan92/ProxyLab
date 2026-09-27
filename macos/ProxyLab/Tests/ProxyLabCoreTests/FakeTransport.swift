//  A transport with a test on the other end of it.
//
//  Everything above the socket — correlation, framing, decoding, failing the
//  right request — is tested through this, and none of it needs a server, a
//  database, or a delay. A test writes the bytes the core would have written,
//  at the moment it chooses to.
//
//  There is no sleeping anywhere in these tests. `nextSent` is how a test
//  waits for the client to have written its request: it resolves when the
//  write happens and not before, so the answer is never sent into a void.

import Foundation

@testable import ProxyLabCore

/// A transport whose peer is the test.
actor FakeTransport: IPCTransport {
    /// Whether `open` should fail, and with what.
    private let failure: IPCError?

    private var continuation: AsyncThrowingStream<Data, any Error>.Continuation?
    private var written: [Data] = []
    private var waiting: [CheckedContinuation<Data, Never>] = []

    private(set) var openCount = 0
    private(set) var closeCount = 0

    init(failingWith failure: IPCError? = nil) {
        self.failure = failure
    }

    func open() async throws -> AsyncThrowingStream<Data, any Error> {
        openCount += 1
        if let failure { throw failure }

        let (stream, continuation) = AsyncThrowingStream<Data, any Error>.makeStream()
        self.continuation = continuation

        return stream
    }

    func send(_ data: Data) async throws {
        guard continuation != nil else { throw IPCError.notConnected }

        if waiting.isEmpty {
            written.append(data)
        } else {
            waiting.removeFirst().resume(returning: data)
        }
    }

    func close() async {
        closeCount += 1
        continuation?.finish()
        continuation = nil
    }

    // MARK: - The test's side

    /// The next line the client wrote, waiting for it if it has not yet.
    func nextSent() async -> Data {
        if !written.isEmpty { return written.removeFirst() }

        return await withCheckedContinuation { waiting.append($0) }
    }

    /// The request id of the next line the client wrote.
    func nextRequestID() async throws -> String {
        let line = await nextSent()
        let object = try JSONSerialization.jsonObject(with: line) as? [String: Any]

        return object?["request_id"] as? String ?? ""
    }

    /// Sends `text` to the client, as a whole line.
    func deliver(_ text: String) {
        continuation?.yield(Data((text + "\n").utf8))
    }

    /// Sends bytes exactly as given, newline or not.
    func deliverRaw(_ text: String) {
        continuation?.yield(Data(text.utf8))
    }

    /// Ends the stream, as a server that hung up would.
    func hangUp() {
        continuation?.finish()
        continuation = nil
    }

    /// Ends the stream with a failure, as a broken socket would.
    func fail(_ error: IPCError) {
        continuation?.finish(throwing: error)
        continuation = nil
    }
}
