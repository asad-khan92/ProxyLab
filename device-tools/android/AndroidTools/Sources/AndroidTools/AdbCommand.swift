//  Running `adb`, and nothing else.
//
//      adb -s emulator-5554 shell settings get global http_proxy
//          │
//          ▼
//      status, stdout, stderr
//
//  # No shell, ever
//
//  `Process` is given an executable and an **argument vector**. There is no
//  `/bin/sh -c`, no interpolated command string, and nothing anywhere in this
//  package builds one. A serial, a path, and a proxy value are elements of
//  that vector, so a value containing a space or a semicolon is one argument
//  rather than the start of a second command.
//
//  `adb shell` is the one place that deserves a second look, because adb joins
//  the arguments after `shell` and hands them to a shell **on the device**.
//  That is adb's behaviour and not something this package can turn off, so the
//  values that reach it are checked instead — see `EmulatorProxy.isRestorable`
//  — and the rest are fixed words chosen here.
//
//  # Both pipes are read at once
//
//  A process whose standard error fills its pipe buffer blocks forever if
//  nobody is draining it. Both are read concurrently and the process is waited
//  on after, which is the only ordering that cannot deadlock.

import Foundation

/// What a command left behind.
public struct CommandOutcome: Equatable, Sendable {
    /// The exit status. Zero is success.
    public let status: Int32
    /// What it wrote to standard output, trimmed.
    public let standardOutput: String
    /// What it wrote to standard error, trimmed.
    public let standardError: String

    public init(status: Int32, standardOutput: String, standardError: String) {
        self.status = status
        self.standardOutput = standardOutput
        self.standardError = standardError
    }

    /// Whether it worked.
    public var succeeded: Bool { status == 0 }

    /// The most useful thing there is to say about a failure: adb's own words,
    /// or its output when it said nothing on standard error.
    public var failureReason: String {
        standardError.isEmpty ? standardOutput : standardError
    }
}

/// Runs one `adb` command.
///
/// A protocol because everything above it — device discovery, proxy state,
/// saving and restoring it — is logic worth testing, and none of it should
/// need a running emulator to be tested.
public protocol AdbCommandRunner: Sendable {
    /// Runs `adb` with `arguments` and answers what it left behind.
    ///
    /// Throws only when the command could not be started at all. A command
    /// that ran and failed is a `CommandOutcome` with a non-zero status,
    /// because that is a result and not an accident.
    func run(_ arguments: [String]) async throws -> CommandOutcome
}

/// The real one: the `adb` this Mac has.
public struct AdbRunner: AdbCommandRunner {
    /// Where `adb` is.
    private let adb: String

    /// - Parameter adb: the executable, which `AdbLocation` resolves.
    public init(adb: String) {
        self.adb = adb
    }

    public func run(_ arguments: [String]) async throws -> CommandOutcome {
        let adb = adb

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(with: Result { try spawn(adb, arguments) })
            }
        }
    }
}

/// Runs the process and drains both of its pipes.
private func spawn(_ adb: String, _ arguments: [String]) throws -> CommandOutcome {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: adb)
    process.arguments = arguments
    // Nothing here reads anything, and a process that inherits a terminal can
    // stop waiting for one.
    process.standardInput = FileHandle.nullDevice

    let output = Pipe()
    let errors = Pipe()
    process.standardOutput = output
    process.standardError = errors

    do {
        try process.run()
    } catch {
        throw AndroidToolsError.adbNotRunnable((error as NSError).localizedDescription)
    }

    let collected = Collected()
    let group = DispatchGroup()
    group.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        collected.setOutput(output.fileHandleForReading.readDataToEndOfFile())
        group.leave()
    }
    group.enter()
    DispatchQueue.global(qos: .userInitiated).async {
        collected.setErrors(errors.fileHandleForReading.readDataToEndOfFile())
        group.leave()
    }

    process.waitUntilExit()
    group.wait()

    func text(_ data: Data) -> String {
        String(decoding: data, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    return CommandOutcome(
        status: process.terminationStatus,
        standardOutput: text(collected.output),
        standardError: text(collected.errors)
    )
}

/// What the two reading threads put their halves into.
private final class Collected: @unchecked Sendable {
    private let lock = NSLock()
    private var _output = Data()
    private var _errors = Data()

    var output: Data { lock.withLock { _output } }
    var errors: Data { lock.withLock { _errors } }

    func setOutput(_ data: Data) { lock.withLock { _output = data } }
    func setErrors(_ data: Data) { lock.withLock { _errors = data } }
}
