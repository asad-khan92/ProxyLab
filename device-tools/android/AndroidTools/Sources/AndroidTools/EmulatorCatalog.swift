//  Which Android Emulators are running.
//
//      adb devices -l
//          │
//          ▼
//      List of devices attached
//      emulator-5554   device product:sdk_gphone16k_arm64 model:… transport_id:1
//
//  # Only emulators, and that is the platform filter
//
//  A serial beginning `emulator-` is the standard Android Emulator, reached
//  over its local adb port. A physical device's serial is its hardware id and
//  a Genymotion or other virtualised device appears as `host:port` — neither
//  is listed here, which is what keeps this step to the emulator it was
//  written for and keeps `10.0.2.2` from being offered to something it does
//  not mean anything to.
//
//  # The header line is not a device
//
//  `adb devices` prints `List of devices attached` first and may print
//  daemon-startup chatter before that. Anything that is not two or more
//  whitespace-separated fields whose first field looks like a serial is not a
//  device row.

import Foundation

/// The running emulators, asked for from `adb`.
public struct EmulatorCatalog: Sendable {
    /// The serial prefix the standard Android Emulator uses.
    public static let emulatorPrefix = "emulator-"

    private let runner: any AdbCommandRunner

    public init(runner: any AdbCommandRunner) {
        self.runner = runner
    }

    /// Every running emulator, named as well as adb can name them.
    ///
    /// An empty list is the answer, not an error: the caller is the one that
    /// knows whether having none is a problem.
    public func runningEmulators() async throws -> [AndroidEmulator] {
        let outcome = try await runner.run(["devices", "-l"])

        guard outcome.succeeded else {
            throw AndroidToolsError.commandFailed(
                command: "devices -l",
                status: outcome.status,
                reason: outcome.failureReason
            )
        }

        let listed = Self.parse(outcome.standardOutput)

        // One extra command per emulator, for the one thing `devices -l` does
        // not carry: what the developer called the AVD. A failure here is not
        // a failure to list — the emulator is still there, and its serial and
        // model still name it.
        var named: [AndroidEmulator] = []
        for emulator in listed {
            named.append(
                AndroidEmulator(
                    serial: emulator.serial,
                    state: emulator.state,
                    model: emulator.model,
                    avdName: emulator.isUsable ? await avdName(of: emulator.serial) : nil
                )
            )
        }

        return named
    }

    /// The AVD's name, or `nil` if adb would not say.
    private func avdName(of serial: String) async -> String? {
        guard let outcome = try? await runner.run(["-s", serial, "emu", "avd", "name"]),
              outcome.succeeded
        else { return nil }

        return Self.readAvdName(outcome.standardOutput)
    }

    /// `emu` replies with the answer and then `OK` on its own line.
    static func readAvdName(_ output: String) -> String? {
        let name = output
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty && $0 != "OK" }

        return (name?.isEmpty ?? true) ? nil : name
    }

    /// Reads `adb devices -l` output into emulators.
    ///
    /// Static and pure, so the shape of adb's output is tested against
    /// captured output rather than against whatever happens to be running.
    public static func parse(_ output: String) -> [AndroidEmulator] {
        output
            .split(separator: "\n")
            .compactMap { row($0) }
            .filter { $0.serial.hasPrefix(emulatorPrefix) }
            .sorted { $0.serial < $1.serial }
    }

    /// One line, if it is a device row.
    private static func row(_ line: Substring) -> AndroidEmulator? {
        let fields = line.split(whereSeparator: \.isWhitespace).map(String.init)
        guard fields.count >= 2 else { return nil }

        let serial = fields[0]
        // `List of devices attached`, and daemon chatter, are not device rows.
        guard !serial.hasSuffix(":") , serial != "List" else { return nil }

        let properties = fields.dropFirst(2).reduce(into: [String: String]()) { found, field in
            let parts = field.split(separator: ":", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { return }

            found[parts[0]] = parts[1]
        }

        return AndroidEmulator(serial: serial, state: fields[1], model: properties["model"])
    }
}
