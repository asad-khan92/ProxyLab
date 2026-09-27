//  Running `simctl`, and nothing else.
//
//      xcrun simctl <arguments…>  ──▶  status, stdout, stderr
//
//  One protocol so every command in this module is testable without a
//  simulator, one implementation that actually spawns `xcrun`, and no
//  knowledge anywhere in here of what the arguments mean.
//
//  `xcrun` rather than a path into Xcode: which Xcode is selected is
//  `xcode-select`'s business, and hard-coding `/Applications/Xcode.app` would
//  be wrong on any Mac with two of them.
//
//  # Both pipes are read at once
//
//  A process whose standard error fills its pipe buffer blocks forever if
//  nobody is draining it, and a `simctl` failure prints to standard error
//  while `simctl list -j` prints to standard output. So both are read
//  concurrently and the process is waited on after, which is the only ordering
//  that cannot deadlock.

import Foundation

/// What a command left behind.
public struct CommandOutcome: Equatable, Sendable {
    /// The exit status. Zero is success; `simctl` uses several non-zero ones.
    public let status: Int32
    /// What it wrote to standard output.
    public let standardOutput: Data
    /// What it wrote to standard error, trimmed.
    public let standardError: String

    public init(status: Int32, standardOutput: Data, standardError: String) {
        self.status = status
        self.standardOutput = standardOutput
        self.standardError = standardError
    }

    /// Whether it worked.
    public var succeeded: Bool { status == 0 }
}

/// Runs one `simctl` command.
///
/// A protocol because everything above it — discovery, trusting a certificate
/// — is logic worth testing, and none of that logic should need a booted
/// simulator to be tested.
public protocol SimulatorCommandRunner: Sendable {
    /// Runs `simctl` with `arguments` and answers what it left behind.
    ///
    /// Throws only when the command could not be started at all. A command
    /// that ran and failed is a `CommandOutcome` with a non-zero status,
    /// because that is a result and not an accident.
    func run(_ arguments: [String]) async throws -> CommandOutcome
}

/// The real one: `xcrun simctl …`.
public struct SimctlRunner: SimulatorCommandRunner {
    /// Where `xcrun` is. Fixed, because it is part of macOS rather than of
    /// whichever Xcode is selected.
    private let xcrun: String

    public init(xcrun: String = "/usr/bin/xcrun") {
        self.xcrun = xcrun
    }

    public func run(_ arguments: [String]) async throws -> CommandOutcome {
        let xcrun = xcrun

        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(with: Result { try spawn(xcrun, arguments) })
            }
        }
    }
}

/// Runs the process and drains both of its pipes.
///
/// Free function rather than a method so the concurrency story is plain: it
/// runs on whatever thread called it, blocks that thread, and touches nothing
/// shared.
private func spawn(_ xcrun: String, _ arguments: [String]) throws -> CommandOutcome {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: xcrun)
    process.arguments = ["simctl"] + arguments

    let output = Pipe()
    let errors = Pipe()
    process.standardOutput = output
    process.standardError = errors
    // A command this module runs never reads anything, and a process that
    // inherits a terminal can stop waiting for one.
    process.standardInput = FileHandle.nullDevice

    do {
        try process.run()
    } catch {
        throw SimulatorToolsError.commandLineToolsUnavailable(
            (error as NSError).localizedDescription
        )
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

    return CommandOutcome(
        status: process.terminationStatus,
        standardOutput: collected.output,
        standardError: String(decoding: collected.errors, as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    )
}

/// What the two reading threads put their halves into.
///
/// A lock rather than a queue hop: each half is written once, by one thread,
/// and read once after `DispatchGroup.wait` has already ordered everything.
/// The lock is what makes that ordering something the compiler will accept
/// rather than something a comment claims.
private final class Collected: @unchecked Sendable {
    private let lock = NSLock()
    private var _output = Data()
    private var _errors = Data()

    var output: Data {
        lock.lock()
        defer { lock.unlock() }

        return _output
    }

    var errors: Data {
        lock.lock()
        defer { lock.unlock() }

        return _errors
    }

    func setOutput(_ data: Data) {
        lock.lock()
        _output = data
        lock.unlock()
    }

    func setErrors(_ data: Data) {
        lock.lock()
        _errors = data
        lock.unlock()
    }
}
