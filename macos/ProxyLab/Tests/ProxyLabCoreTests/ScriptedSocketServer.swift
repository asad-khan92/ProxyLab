//  A Unix socket with a script on the other end.
//
//  Enough of a server to exercise the real transport: it binds a socket in a
//  temporary directory, reads lines, and answers them from a closure the test
//  supplies. It is not the Rust server and does not pretend to be — it is how
//  `UnixSocketTransport` gets tested without one, which is what keeps the
//  package's tests runnable with nothing else installed.
//
//  Blocking reads and writes, on queues of its own. That is fine here: this is
//  a test fixture, and being able to read it straight through is worth more
//  than being asynchronous.

import Dispatch
import Foundation

#if canImport(Darwin)
    import Darwin
#endif

/// What the server does with a line it was sent.
enum ServerAction: Sendable {
    /// Send these lines back.
    case reply([String])
    /// Say nothing.
    case silence
    /// Close the connection.
    case hangUp
}

/// A Unix socket server that answers lines from a closure.
final class ScriptedSocketServer: @unchecked Sendable {
    /// Where it listens.
    let path: String

    private let directory: URL
    private let accepting = DispatchQueue(label: "test.proxylab.accept")
    private let serving = DispatchQueue(label: "test.proxylab.serve")
    private let lock = NSLock()

    private var listener: Int32 = -1
    private var connections: [Int32] = []
    private var stopped = false

    /// A server on a socket of its own, in a directory removed with it.
    init() throws {
        directory = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("proxylab-tests-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        path = directory.appendingPathComponent("proxylab.sock").path
    }

    deinit {
        stop()
        try? FileManager.default.removeItem(at: directory)
    }

    /// Binds, listens, and answers every line with `responder`.
    func start(responder: @escaping @Sendable (String) -> ServerAction) throws {
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: bytes)
            destination[bytes.count] = 0
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)

        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw ServerError.couldNotBind }

        let bound = withUnsafePointer(to: &address) { stored in
            stored.withMemoryRebound(to: sockaddr.self, capacity: 1) { generic in
                bind(descriptor, generic, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(descriptor, 8) == 0 else {
            Darwin.close(descriptor)
            throw ServerError.couldNotBind
        }

        lock.withLock { listener = descriptor }

        accepting.async { [weak self] in
            while let self {
                let connection = accept(descriptor, nil, nil)
                guard connection >= 0 else { return }

                self.lock.withLock { self.connections.append(connection) }
                self.serving.async { self.serve(connection, responder: responder) }
            }
        }
    }

    /// Closes every client connection, as a core being stopped would.
    func hangUpOnClients() {
        let open = lock.withLock {
            let open = connections
            connections = []

            return open
        }

        for connection in open {
            shutdown(connection, SHUT_RDWR)
            Darwin.close(connection)
        }
    }

    /// Stops listening and closes everything.
    func stop() {
        let descriptor: Int32 = lock.withLock {
            guard !stopped else { return -1 }
            stopped = true
            let descriptor = listener
            listener = -1

            return descriptor
        }

        if descriptor >= 0 {
            shutdown(descriptor, SHUT_RDWR)
            Darwin.close(descriptor)
        }
        hangUpOnClients()
        unlink(path)
    }

    /// Reads lines from one client until it goes away.
    private func serve(_ connection: Int32, responder: @escaping @Sendable (String) -> ServerAction)
    {
        var pending = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)

        while true {
            let count = read(connection, &buffer, buffer.count)
            guard count > 0 else {
                Darwin.close(connection)
                return
            }

            pending.append(contentsOf: buffer[0..<count])

            while let newline = pending.firstIndex(of: UInt8(ascii: "\n")) {
                let line = String(decoding: pending[pending.startIndex..<newline], as: UTF8.self)
                pending = Data(pending[pending.index(after: newline)...])

                switch responder(line) {
                case let .reply(lines):
                    for reply in lines {
                        var out = Array((reply + "\n").utf8)
                        _ = write(connection, &out, out.count)
                    }
                case .silence:
                    break
                case .hangUp:
                    shutdown(connection, SHUT_RDWR)
                    Darwin.close(connection)

                    return
                }
            }
        }
    }

    enum ServerError: Error {
        case couldNotBind
    }
}
