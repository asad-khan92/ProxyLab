//  An adb, and an emulator behind it, that the tests decide.
//
//  The same simulation the `AndroidTools` tests use: it holds a settings
//  value, `settings put` changes it, `settings get` reads it back. Saving a
//  proxy and putting it back is only testable against something that actually
//  remembers what it was told.

import Foundation

import AndroidTools

/// An adb wired to one pretend emulator.
final class FakeAdb: AdbCommandRunner, @unchecked Sendable {
    private let lock = NSLock()
    private var _invocations: [[String]] = []
    private var _proxy: EmulatorProxy
    private var _present: Bool
    private let serial: String

    var invocations: [[String]] { lock.withLock { _invocations } }
    var proxy: EmulatorProxy { lock.withLock { _proxy } }

    init(serial: String = "emulator-5554", proxy: EmulatorProxy = .disabled, present: Bool = true) {
        self.serial = serial
        _proxy = proxy
        _present = present
    }

    /// The emulator is shut down from here on.
    func shutDown() { lock.withLock { _present = false } }

    func run(_ arguments: [String]) async throws -> CommandOutcome {
        lock.withLock { _invocations.append(arguments) }

        if arguments == ["devices", "-l"] {
            let rows = lock.withLock { _present }
                ? "\(serial)          device product:sdk_gphone16k_arm64 model:sdk_gphone16k_arm64"
                : ""

            return ok("List of devices attached\n\(rows)")
        }

        guard arguments.count >= 2, arguments[0] == "-s" else { return ok("") }
        let rest = Array(arguments.dropFirst(2))

        guard lock.withLock({ _present }), arguments[1] == serial else {
            return CommandOutcome(
                status: 1,
                standardOutput: "",
                standardError: "adb: device '\(arguments[1])' not found"
            )
        }

        if rest == ["emu", "avd", "name"] { return ok("Pixel_10_Pro_XL\nOK") }
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
        if rest.count == 6,
           Array(rest.prefix(5)) == ["shell", "settings", "put", "global", "http_proxy"] {
            lock.withLock { _proxy = EmulatorProxy.read(rest[5]) }

            return ok("")
        }

        return ok("")
    }

    private func ok(_ output: String) -> CommandOutcome {
        CommandOutcome(status: 0, standardOutput: output, standardError: "")
    }
}
