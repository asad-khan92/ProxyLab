//  Which iOS Simulators are booted.
//
//      xcrun simctl list devices booted -j
//          │
//          ▼
//      { "devices": { "<runtime id>": [ { udid, name, state }, … ] } }
//
//  The JSON form rather than the printed one: the printed list is for people
//  and changes shape between Xcode releases, and `-j` is the documented way to
//  ask the same question and get something to parse.
//
//  Only iOS runtimes are listed. A booted watchOS or tvOS simulator is a real
//  thing to have, and it is not what this section of the interface is for;
//  showing one in a picker labelled "iOS Simulator" would be an invitation to
//  trust a certificate somewhere nobody meant to.

import Foundation

/// The booted simulators, asked for from `simctl`.
public struct SimulatorCatalog: Sendable {
    /// The prefix every runtime identifier carries.
    private static let runtimePrefix = "com.apple.CoreSimulator.SimRuntime."

    /// The platform this catalog lists.
    private static let platform = "iOS"

    private let runner: SimulatorCommandRunner

    public init(runner: SimulatorCommandRunner = SimctlRunner()) {
        self.runner = runner
    }

    /// Every booted iOS Simulator, ordered so a picker does not reshuffle
    /// itself between refreshes.
    ///
    /// An empty list is not an error here — it is the answer, and the caller
    /// is the one that knows whether having none is a problem.
    public func bootedSimulators() async throws -> [Simulator] {
        let outcome = try await runner.run(["list", "devices", "booted", "-j"])

        guard outcome.succeeded else {
            throw SimulatorToolsError.commandFailed(
                command: "list devices booted",
                status: outcome.status,
                reason: outcome.standardError
            )
        }

        return try Self.parse(outcome.standardOutput)
    }

    /// Reads `simctl list -j` output into simulators.
    ///
    /// Static and pure, so the shape of Apple's JSON is tested against
    /// captured output rather than against whatever happens to be booted on
    /// the machine running the tests.
    public static func parse(_ json: Data) throws -> [Simulator] {
        let listing: DeviceListing
        do {
            listing = try JSONDecoder().decode(DeviceListing.self, from: json)
        } catch {
            throw SimulatorToolsError.deviceListUnreadable
        }

        let simulators = listing.devices.flatMap { runtimeIdentifier, devices in
            devices
                .filter { $0.state == "Booted" }
                .compactMap { device -> Simulator? in
                    guard let runtime = runtimeName(from: runtimeIdentifier) else { return nil }

                    return Simulator(udid: device.udid, name: device.name, runtime: runtime)
                }
        }

        // A dictionary has no order, so one is imposed: the same two
        // simulators come back in the same order every time, and a picker's
        // rows do not jump while someone is reading them.
        return simulators.sorted {
            ($0.name, $0.runtime, $0.udid) < ($1.name, $1.runtime, $1.udid)
        }
    }

    /// `com.apple.CoreSimulator.SimRuntime.iOS-26-1` → `iOS 26.1`.
    ///
    /// Answers `nil` for a runtime that is not iOS, which is how the platform
    /// filter is applied: there is one place that reads a runtime identifier,
    /// so there is one place that decides what counts.
    static func runtimeName(from identifier: String) -> String? {
        let tail = identifier.hasPrefix(runtimePrefix)
            ? String(identifier.dropFirst(runtimePrefix.count))
            : identifier

        let parts = tail.split(separator: "-")
        guard let platform = parts.first, platform == Self.platform else { return nil }

        let version = parts.dropFirst().joined(separator: ".")

        return version.isEmpty ? String(platform) : "\(platform) \(version)"
    }
}

/// The part of `simctl list -j` this module reads.
///
/// Deliberately a few fields of many: `dataPath`, `logPath`, and the sizes
/// beside them are none of ProxyLab's business, and a decoder that ignores
/// them is one that keeps working when Apple adds the next field.
private struct DeviceListing: Decodable {
    let devices: [String: [Device]]

    struct Device: Decodable {
        let udid: String
        let name: String
        let state: String
    }
}
