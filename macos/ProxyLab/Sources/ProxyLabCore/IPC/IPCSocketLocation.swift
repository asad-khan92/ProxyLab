//  Where the core's socket is.
//
//      $PROXYLAB_IPC_SOCKET          if it is set and not empty
//      ~/.proxylab/run/proxylab.sock  otherwise
//
//  The same two rules, in the same order, as `IpcConfig::default_socket` in
//  `core/ipc/src/socket.rs`. This is the only place in the app that knows
//  them: everything else asks here, so the app and the core cannot drift apart
//  about where to meet.
//
//  Nothing writes a shell configuration file to make this work. The variable
//  is read from the process environment, which is what Xcode's scheme editor
//  sets and what `PROXYLAB_IPC_SOCKET=… swift run ProxyLab` sets.

import Foundation

/// Resolves the socket the core listens on.
public enum IPCSocketLocation {
    /// The variable that names a socket other than the default.
    public static let environmentVariable = "PROXYLAB_IPC_SOCKET"

    /// ProxyLab's directory under the home directory.
    static let stateDirectory = ".proxylab"

    /// The directory the socket lives in, under that.
    static let runtimeDirectory = "run"

    /// The socket's name inside it.
    static let socketFile = "proxylab.sock"

    /// The longest socket path macOS will accept.
    ///
    /// A Unix socket address is a fixed-size buffer, and a path that does not
    /// fit produces an error from `connect` that says nothing about why.
    public static let maximumPathLength = 103

    /// The path the core is expected to be listening on.
    ///
    /// - Parameter environment: the process environment, which a test may
    ///   supply instead.
    public static func resolve(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws -> String {
        if let configured = environment[environmentVariable], !configured.isEmpty {
            return configured
        }

        let home = [environment["HOME"], environment["USERPROFILE"]]
            .compactMap { $0 }
            .first { !$0.isEmpty }

        guard let home else { throw IPCError.noHomeDirectory }

        return URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent(stateDirectory, isDirectory: true)
            .appendingPathComponent(runtimeDirectory, isDirectory: true)
            .appendingPathComponent(socketFile)
            .path
    }
}
