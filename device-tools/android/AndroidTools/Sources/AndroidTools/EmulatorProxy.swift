//  The emulator's HTTP proxy: what it is, how it is changed, and how it is put
//  back exactly as it was.
//
//      adb -s <serial> shell settings get    global http_proxy
//      adb -s <serial> shell settings put    global http_proxy 10.0.2.2:9090
//      adb -s <serial> shell settings delete global http_proxy
//
//  # There are three states, not two
//
//      null            the key is absent — the emulator has never had one
//      :0              the key is there and means "no proxy"
//      host:port       a proxy
//
//  `null` and `:0` both mean traffic goes direct, and they are **not the same
//  stored value**. Putting `:0` back onto an emulator whose key was absent
//  leaves a row behind that was not there before, which is a settings database
//  that does not match the one ProxyLab found. So `unset` is restored with
//  `settings delete` and `disabled` with `settings put :0`, and the difference
//  is kept all the way through.
//
//  # Nothing else is touched
//
//  `global http_proxy` and that key alone. The exclusion list beside it
//  (`global_http_proxy_exclusion_list`) is never read and never written, so
//  whatever the developer had there survives a connect and a disconnect
//  untouched — it is not ProxyLab's to change.

import Foundation

/// Where ProxyLab asks an emulator to send its traffic.
public struct EmulatorProxyAddress: Equatable, Sendable, CustomStringConvertible {
    public let host: String
    public let port: Int

    public init(host: String, port: Int) {
        self.host = host
        self.port = port
    }

    /// The standard Android Emulator's alias for the host's loopback
    /// interface.
    ///
    /// This is why ProxyLab needs no second listener and no LAN exposure for
    /// an emulator: `10.0.2.2` from inside the emulator arrives at `127.0.0.1`
    /// on the Mac, which is where the proxy already is.
    ///
    /// It is the *standard* emulator's alias and nobody else's. Genymotion
    /// uses another address, a physical device has no such alias at all, and
    /// neither is offered this — see `EmulatorCatalog`, which lists only
    /// `emulator-` serials.
    public static let hostLoopbackAlias = "10.0.2.2"

    /// ProxyLab, as an emulator can reach it.
    public static func onHostLoopback(port: Int) -> Self {
        Self(host: hostLoopbackAlias, port: port)
    }

    public var description: String { "\(host):\(port)" }
}

/// What an emulator's `global http_proxy` says.
public enum EmulatorProxy: Equatable, Sendable {
    /// The key is absent: `settings get` answered `null`.
    case unset
    /// The key is there and means no proxy: `:0`.
    case disabled
    /// A proxy, held exactly as the device spells it.
    case configured(String)

    /// How adb printed it, which is what is shown to a person.
    public var description: String {
        switch self {
        case .unset: "none"
        case .disabled: "none"
        case let .configured(value): value
        }
    }

    /// Whether traffic is being proxied anywhere.
    public var isProxied: Bool {
        if case .configured = self { return true }

        return false
    }

    /// Reads what `settings get global http_proxy` printed.
    public static func read(_ output: String) -> EmulatorProxy {
        let value = output.trimmingCharacters(in: .whitespacesAndNewlines)

        if value.isEmpty || value == "null" { return .unset }
        if value == ":0" { return .disabled }

        return .configured(value)
    }

    /// Whether this value can be sent back to the device.
    ///
    /// `adb shell` hands its arguments to a shell **on the device**, so a
    /// value with a space or a shell metacharacter in it would not go back as
    /// one word. A proxy setting never looks like that, and a value that does
    /// is refused and reported rather than sent — see
    /// `AndroidToolsError.proxyNotRestorable`.
    public var isRestorable: Bool {
        guard case let .configured(value) = self else { return true }

        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz")
            .union(CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZ"))
            .union(CharacterSet(charactersIn: "0123456789.:-_[]"))

        return !value.isEmpty && value.unicodeScalars.allSatisfy(allowed.contains)
    }

    /// The adb arguments that put this state back, after `-s <serial>`.
    public var restoreArguments: [String] {
        switch self {
        case .unset:
            ["shell", "settings", "delete", "global", "http_proxy"]
        case .disabled:
            ["shell", "settings", "put", "global", "http_proxy", ":0"]
        case let .configured(value):
            ["shell", "settings", "put", "global", "http_proxy", value]
        }
    }
}

/// Reads and changes one emulator's proxy setting.
public struct EmulatorProxyControl: Sendable {
    private let runner: any AdbCommandRunner

    public init(runner: any AdbCommandRunner) {
        self.runner = runner
    }

    /// What the emulator's proxy is right now.
    public func currentProxy(on serial: String) async throws -> EmulatorProxy {
        let outcome = try await runner.run(
            ["-s", serial, "shell", "settings", "get", "global", "http_proxy"]
        )

        guard outcome.succeeded else {
            throw Self.failure(outcome, command: "shell settings get global http_proxy")
        }

        return EmulatorProxy.read(outcome.standardOutput)
    }

    /// Points the emulator at `address`, and reads it back to be sure.
    ///
    /// Verified rather than assumed: `settings put` exits zero for a write
    /// that a later read does not agree with, and a proxy that was reported as
    /// set and is not is the one failure that would waste an afternoon.
    public func connect(_ serial: String, to address: EmulatorProxyAddress) async throws {
        try await apply(.configured(address.description), on: serial)

        let readBack = try await currentProxy(on: serial)
        guard readBack == .configured(address.description) else {
            throw AndroidToolsError.proxyNotApplied(
                expected: address.description,
                actual: readBack.description
            )
        }
    }

    /// Puts `proxy` back, and reads it back to be sure.
    public func restore(_ proxy: EmulatorProxy, on serial: String) async throws {
        guard proxy.isRestorable else {
            throw AndroidToolsError.proxyNotRestorable(proxy.description)
        }

        try await apply(proxy, on: serial)

        let readBack = try await currentProxy(on: serial)
        guard readBack == proxy else {
            throw AndroidToolsError.proxyNotApplied(
                expected: proxy.description,
                actual: readBack.description
            )
        }
    }

    /// Writes the setting, without reading it back.
    private func apply(_ proxy: EmulatorProxy, on serial: String) async throws {
        let outcome = try await runner.run(["-s", serial] + proxy.restoreArguments)

        guard outcome.succeeded else {
            throw Self.failure(outcome, command: "shell settings put global http_proxy")
        }
    }

    private static func failure(_ outcome: CommandOutcome, command: String) -> AndroidToolsError {
        .commandFailed(command: command, status: outcome.status, reason: outcome.failureReason)
    }
}
