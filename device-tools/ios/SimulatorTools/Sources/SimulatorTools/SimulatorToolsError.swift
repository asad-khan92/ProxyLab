//  What can go wrong reaching a simulator, said in a sentence a developer can
//  act on.
//
//  Every case here names the thing to do next. "No iOS Simulator is booted" is
//  not an error state the interface has to translate; it is the message.
//
//  Nothing a command printed is repeated back verbatim except `simctl`'s own
//  standard error, which is Apple's text about Apple's tool and is the most
//  useful thing there is when a command fails for a reason this module did not
//  anticipate.

import Foundation

/// A failure reaching, reading, or configuring a simulator.
public enum SimulatorToolsError: Error, Equatable, Sendable {
    /// `xcrun` could not be run at all.
    case commandLineToolsUnavailable(String)
    /// A `simctl` command ran and failed.
    case commandFailed(command: String, status: Int32, reason: String)
    /// `simctl list` answered something that is not the device list.
    case deviceListUnreadable
    /// Nothing is booted, so there is nothing to connect.
    case noBootedSimulator
    /// The CA certificate is not where it was expected.
    case certificateMissing(path: String)
    /// The Mac's proxy configuration could not be read.
    case proxyRouteUnreadable

    /// The sentence the interface shows.
    public var message: String {
        switch self {
        case let .commandLineToolsUnavailable(reason):
            "The Xcode command line tools could not be run: \(reason). "
                + "Install Xcode, then run xcode-select --install."
        case let .commandFailed(command, status, reason) where reason.isEmpty:
            "simctl \(command) failed with status \(status)."
        case let .commandFailed(command, _, reason):
            "simctl \(command) failed: \(reason)"
        case .deviceListUnreadable:
            "The list of simulators could not be read. "
                + "Check that xcrun simctl list devices booted runs in a terminal."
        case .noBootedSimulator:
            "No iOS Simulator is booted. Boot one from Xcode or the Simulator app, then refresh."
        case let .certificateMissing(path):
            "ProxyLab's CA certificate is not at \(path). "
                + "Run the ProxyLab core once — it creates the CA the first time it starts."
        case .proxyRouteUnreadable:
            "This Mac's proxy configuration could not be read, "
                + "so there is no way to tell what the simulator will connect through."
        }
    }
}
