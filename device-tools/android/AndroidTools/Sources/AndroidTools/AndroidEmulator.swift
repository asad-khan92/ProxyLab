//  One Android Emulator that adb can see.
//
//  A serial, what state adb says it is in, and enough of a name for a person
//  to pick the right one from a menu.

import Foundation

/// An emulator `adb devices` listed.
public struct AndroidEmulator: Identifiable, Hashable, Sendable {
    /// What every adb command is addressed to: `emulator-5554`.
    public let serial: String
    /// What adb says it is: `device`, `offline`, `unauthorized`, `bootloader`.
    public let state: String
    /// The `model:` adb printed, when it printed one.
    public let model: String?
    /// The AVD's own name, asked for separately with `emu avd name`.
    public let avdName: String?

    public var id: String { serial }

    public init(serial: String, state: String, model: String? = nil, avdName: String? = nil) {
        self.serial = serial
        self.state = state
        self.model = model
        self.avdName = avdName
    }

    /// The state adb reports for an emulator that is booted and answering.
    public static let ready = "device"

    /// Whether it can be talked to at all.
    ///
    /// An emulator part-way through booting is `offline`, and every command
    /// sent to it fails. Listing it and refusing to connect it says more than
    /// leaving it out, which would look like it was not running.
    public var isUsable: Bool { state == Self.ready }

    /// What a picker shows.
    ///
    /// The AVD name if adb gave one, the model if not, and the serial always —
    /// two emulators from the same AVD image are told apart by nothing else.
    public var label: String {
        guard let name = avdName ?? model, !name.isEmpty else { return serial }

        return "\(name) — \(serial)"
    }
}
