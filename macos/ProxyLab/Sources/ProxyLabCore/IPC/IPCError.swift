//  What can go wrong between the app and the core, in words a person can act on.
//
//  Every case here has a message written for someone looking at a debugging
//  proxy that is not showing them traffic. None of them carries a stack trace,
//  a syscall name, or anything the server sent: the server's own error
//  messages are fixed phrases it chose, and those are the only server text
//  that is ever shown.

import Foundation

/// A failure of the IPC boundary.
public enum IPCError: Error, Equatable, Sendable {
    /// The socket could not be reached. Usually: the core is not running.
    case cannotConnect(path: String)
    /// There is no home directory to resolve the default socket path in.
    case noHomeDirectory
    /// The socket path is longer than the operating system allows.
    case socketPathTooLong(path: String, limit: Int)
    /// A request was made while the client was not connected.
    case notConnected
    /// The connection ended while requests were outstanding.
    case connectionClosed
    /// The socket failed while being read or written.
    case transportFailure(String)
    /// A line from the server was not a message this client can read.
    case malformedMessage
    /// The server spoke a protocol version this build does not.
    case unsupportedProtocolVersion(Int)
    /// A line from the server was longer than this client will buffer.
    case messageTooLarge
    /// The server refused the request, and said why.
    case server(IPCErrorBody)

    /// The whole of what a person is shown.
    public var message: String {
        switch self {
        case let .cannotConnect(path):
            "Could not reach the ProxyLab core at \(path). Is it running?"
        case .noHomeDirectory:
            "There is no home directory to find the ProxyLab socket in."
        case let .socketPathTooLong(path, limit):
            "The socket path is longer than the \(limit) characters macOS allows: \(path)"
        case .notConnected:
            "Not connected to the ProxyLab core."
        case .connectionClosed:
            "The connection to the ProxyLab core closed."
        case let .transportFailure(reason):
            "The connection to the ProxyLab core failed: \(reason)"
        case .malformedMessage:
            "The ProxyLab core sent something this app could not read."
        case let .unsupportedProtocolVersion(version):
            "The ProxyLab core speaks protocol version \(version); this app speaks "
                + "\(IPCProtocolVersion.current). Both need to be the same build."
        case .messageTooLarge:
            "The ProxyLab core sent a message larger than this app will read."
        case let .server(body):
            body.message
        }
    }

    /// The code the server refused with, when the server refused it.
    public var serverCode: IPCErrorCode? {
        guard case let .server(body) = self else { return nil }

        return body.code
    }
}
