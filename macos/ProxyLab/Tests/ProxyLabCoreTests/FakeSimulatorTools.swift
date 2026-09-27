//  A `simctl` and a proxy configuration that the tests decide.
//
//  The same shape as the doubles in `SimulatorTools`' own tests, and here for
//  the same reason: what `SimulatorSessionModel` decides is worth testing, and
//  none of it should need a booted simulator or a particular Mac.

import Foundation
import SimulatorTools

/// A command runner with canned answers, recording what it was asked to run.
final class FakeSimctl: SimulatorCommandRunner, @unchecked Sendable {
    private let lock = NSLock()
    private var _invocations: [[String]] = []
    private let outcome: CommandOutcome

    /// Every argument list it was given, in order.
    var invocations: [[String]] { lock.withLock { _invocations } }

    init(status: Int32 = 0, output: Data = Data(), errors: String = "") {
        outcome = CommandOutcome(status: status, standardOutput: output, standardError: errors)
    }

    /// One that answers `simctl list devices booted -j` with these simulators.
    static func listing(_ devices: [(udid: String, name: String)]) -> FakeSimctl {
        let rows = devices
            .map { #"{"udid":"\#($0.udid)","name":"\#($0.name)","state":"Booted"}"# }
            .joined(separator: ",")

        return FakeSimctl(output: Data(
            #"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-1":[\#(rows)]}}"#.utf8
        ))
    }

    func run(_ arguments: [String]) async throws -> CommandOutcome {
        lock.withLock { _invocations.append(arguments) }

        return outcome
    }
}

/// A proxy configuration that is whatever the test says it is.
struct FakeProxyRoute: ProxyRouteReading {
    let route: ProxyRoute

    func currentRoute() throws -> ProxyRoute { route }
}

/// One that cannot be read at all.
struct UnreadableProxyRoute: ProxyRouteReading {
    func currentRoute() throws -> ProxyRoute {
        throw SimulatorToolsError.proxyRouteUnreadable
    }
}
