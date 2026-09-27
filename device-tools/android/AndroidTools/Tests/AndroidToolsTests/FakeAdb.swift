//  An adb, and an emulator behind it, that the tests decide.
//
//  Not a table of canned replies but a small simulation: it holds a settings
//  value, `settings put` changes it, `settings get` reads it back, and
//  `settings delete` removes it. That is what makes saving a proxy, restoring
//  it, and doing either twice testable as behaviour rather than as a sequence
//  of strings — a fake that always answered the same thing would pass a
//  restore that restored nothing.

import Foundation

@testable import AndroidTools

/// An adb wired to one pretend emulator.
final class FakeAdb: AdbCommandRunner, @unchecked Sendable {
    private let lock = NSLock()
    private var _invocations: [[String]] = []
    private var _proxy: EmulatorProxy
    private var _present: Bool
    private let serial: String
    private let avd: String?
    private let listing: String?
    /// When set, every command fails with this, as adb would.
    private var _failure: (status: Int32, message: String)?

    /// Every argument vector it was given, in order.
    var invocations: [[String]] { lock.withLock { _invocations } }

    /// What the pretend emulator's proxy setting is now.
    var proxy: EmulatorProxy { lock.withLock { _proxy } }

    init(
        serial: String = "emulator-5554",
        proxy: EmulatorProxy = .disabled,
        avd: String? = "Pixel_10_Pro_XL",
        listing: String? = nil,
        present: Bool = true
    ) {
        self.serial = serial
        _proxy = proxy
        self.avd = avd
        self.listing = listing
        _present = present
    }

    /// The emulator is shut down from here on.
    func shutDown() {
        lock.withLock { _present = false }
    }

    /// Every command from here on fails the way adb fails.
    func failEverything(status: Int32 = 1, message: String) {
        lock.withLock { _failure = (status, message) }
    }

    func run(_ arguments: [String]) async throws -> CommandOutcome {
        lock.withLock { _invocations.append(arguments) }

        if let failure = lock.withLock({ _failure }) {
            return CommandOutcome(
                status: failure.status,
                standardOutput: "",
                standardError: failure.message
            )
        }

        if arguments == ["devices", "-l"] {
            return ok(listing ?? defaultListing)
        }

        // Everything else is addressed to a device.
        guard arguments.count >= 2, arguments[0] == "-s" else { return ok("") }
        let addressed = arguments[1]
        let rest = Array(arguments.dropFirst(2))

        guard lock.withLock({ _present }), addressed == serial else {
            return CommandOutcome(
                status: 1,
                standardOutput: "",
                standardError: "adb: device '\(addressed)' not found"
            )
        }

        if rest == ["emu", "avd", "name"] {
            guard let avd else { return CommandOutcome(status: 1, standardOutput: "", standardError: "") }

            return ok("\(avd)\nOK")
        }

        if rest.first == "push" { return ok("1 file pushed, 0 skipped.") }

        if rest == ["shell", "settings", "get", "global", "http_proxy"] {
            return ok(lock.withLock {
                switch _proxy {
                case .unset: "null"
                case .disabled: ":0"
                case let .configured(value): value
                }
            })
        }

        if rest == ["shell", "settings", "delete", "global", "http_proxy"] {
            lock.withLock { _proxy = .unset }

            return ok("Deleted 1 rows")
        }

        if rest.count == 6, Array(rest.prefix(5)) == ["shell", "settings", "put", "global", "http_proxy"] {
            lock.withLock { _proxy = EmulatorProxy.read(rest[5]) }

            return ok("")
        }

        return ok("")
    }

    private func ok(_ output: String) -> CommandOutcome {
        CommandOutcome(status: 0, standardOutput: output, standardError: "")
    }

    private var defaultListing: String {
        """
        List of devices attached
        \(serial)          device product:sdk_gphone16k_arm64 model:sdk_gphone16k_arm64 \
        device:emu64a16k transport_id:1
        """
    }
}
