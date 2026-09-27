//  The Unix domain socket the core listens on.
//
//      ~/.proxylab/run/proxylab.sock     0600, in a directory that is 0700
//
//  A Unix socket and never TCP, because everything ProxyLab has ever captured
//  is reachable through it: credentials, cookies, and bodies. A socket in a
//  private directory cannot be reached from another machine by construction,
//  and its file permissions are the access control.
//
//  # Nothing blocks a thread that matters
//
//  Reads and writes go through `DispatchIO`, which does the waiting in the
//  kernel and calls back on a queue of this object's own. Nothing here runs on
//  the main thread and nothing parks a Swift Concurrency cooperative thread on
//  a blocking syscall: `connect` is the one syscall that could wait, and it is
//  made on that same background queue.
//
//  `DispatchIO` also settles the question of when the descriptor may be
//  closed. Its cleanup handler runs after every read and write it owns has
//  finished, which is the only moment at which closing cannot race a write in
//  flight and hand the descriptor number to something else.

import Dispatch
import Foundation

#if canImport(Darwin)
    import Darwin
#endif

/// A connection to the core over its Unix domain socket.
public actor UnixSocketTransport: IPCTransport {
    /// The socket to connect to.
    public let path: String

    /// Where the connect syscall and every I/O callback happen.
    private let queue: DispatchQueue

    private var channel: DispatchIO?
    private var incoming: AsyncThrowingStream<Data, any Error>.Continuation?

    /// A transport for the socket at `path`.
    public init(path: String) {
        self.path = path
        self.queue = DispatchQueue(label: "com.proxylab.ipc.socket", qos: .userInitiated)
    }

    public func open() async throws -> AsyncThrowingStream<Data, any Error> {
        guard channel == nil else { throw IPCError.transportFailure("already open") }

        let descriptor = try await connect()
        let (stream, continuation) = AsyncThrowingStream<Data, any Error>.makeStream()

        // The cleanup handler is the one place the descriptor is closed, and
        // it runs only once every operation the channel owns has ended. That
        // is what makes closing safe: nothing can be reading or writing a
        // descriptor number that has been handed back to the system.
        let channel = DispatchIO(
            type: .stream,
            fileDescriptor: descriptor,
            queue: queue,
            cleanupHandler: { _ in Darwin.close(descriptor) }
        )

        // Deliver whatever has arrived rather than waiting for a buffer to
        // fill: a response to one request is a message the app is waiting on.
        channel.setLimit(lowWater: 1)

        self.channel = channel
        self.incoming = continuation

        channel.read(offset: 0, length: Int.max, queue: queue) { done, data, error in
            if let data, !data.isEmpty {
                continuation.yield(Data(data))
            }

            guard done else { return }

            if error == 0 {
                continuation.finish()
            } else {
                continuation.finish(throwing: IPCError.transportFailure(describe(error)))
            }
        }

        return stream
    }

    public func send(_ data: Data) async throws {
        guard let channel else { throw IPCError.notConnected }

        let payload = data.withUnsafeBytes { DispatchData(bytes: $0) }

        try await withCheckedThrowingContinuation { (resume: CheckedContinuation<Void, any Error>) in
            channel.write(offset: 0, data: payload, queue: queue) { done, _, error in
                guard done else { return }

                if error == 0 {
                    resume.resume()
                } else {
                    resume.resume(throwing: IPCError.transportFailure(describe(error)))
                }
            }
        }
    }

    public func close() async {
        // `.stop` rather than a graceful close: a pending read is a read that
        // will never finish on its own, because the server has no reason to
        // send anything more.
        channel?.close(flags: .stop)
        channel = nil

        incoming?.finish()
        incoming = nil
    }

    /// Opens the socket and connects it, on the background queue.
    private func connect() async throws -> Int32 {
        let path = self.path

        return try await withCheckedThrowingContinuation { resume in
            queue.async {
                do {
                    resume.resume(returning: try connectToUnixSocket(at: path))
                } catch {
                    resume.resume(throwing: error)
                }
            }
        }
    }
}

/// Connects a new socket to `path`, or says why it could not be.
///
/// Blocking, and called only on the transport's own queue. Connecting a Unix
/// socket does not wait on a network: it succeeds or fails against a path on
/// this machine.
private func connectToUnixSocket(at path: String) throws -> Int32 {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)

    let pathBytes = Array(path.utf8)
    let capacity = MemoryLayout.size(ofValue: address.sun_path)
    guard pathBytes.count < capacity else {
        throw IPCError.socketPathTooLong(path: path, limit: capacity - 1)
    }

    withUnsafeMutableBytes(of: &address.sun_path) { destination in
        destination.copyBytes(from: pathBytes)
        destination[pathBytes.count] = 0
    }
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

    let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
    guard descriptor >= 0 else { throw IPCError.cannotConnect(path: path) }

    let connected = withUnsafePointer(to: &address) { stored in
        stored.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
            Darwin.connect(descriptor, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }

    guard connected == 0 else {
        Darwin.close(descriptor)
        // Every reason a connect to a Unix socket fails comes to the same
        // thing for a person: nothing is listening there.
        throw IPCError.cannotConnect(path: path)
    }

    return descriptor
}

/// What an `errno` means, as a phrase rather than a number.
private func describe(_ code: Int32) -> String {
    String(cString: strerror(code))
}
