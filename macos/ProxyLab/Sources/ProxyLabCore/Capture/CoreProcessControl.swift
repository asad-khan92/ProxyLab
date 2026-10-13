//  The proxy-engine process this app launched, and no other.
//
//      launch     Contents/MacOS/proxy-engine, kept as a Process
//      stop       SIGINT, which is the core's existing shutdown
//                 then SIGTERM, then SIGKILL, only if that process is still
//                 the one this object started
//
//  A process this app did not start is never signaled. Two launches are not
//  started on top of each other: a second launch while the first is running
//  is the first launch.

import Darwin
import Foundation

/// Why the bundled engine could not be started.
public struct CoreProcessError: Error, Equatable, Sendable, LocalizedError {
    public let message: String

    public var errorDescription: String? { message }

    static func missingExecutable(_ path: String) -> Self {
        Self(message: "ProxyLab could not find its proxy engine at \(path). "
            + "Open the ProxyLab.app built by Scripts/make-app-bundle.sh.")
    }

    static func launchFailed(_ detail: String) -> Self {
        Self(message: "ProxyLab could not start its proxy engine. \(detail)")
    }
}

/// Launching and stopping the one core process an app instance owns.
@MainActor
public protocol CoreProcessControlling: AnyObject {
    /// Whether the process this object launched is still running.
    var isRunning: Bool { get }

    /// Called when that process ends without ``stop()`` having asked it to.
    var onExit: (@MainActor () -> Void)? { get set }

    /// Starts the executable, unless it is already running.
    func launch() throws

    /// Asks the owned process to exit, then forces it if it does not.
    func stop() async
}

/// `Process` around the bundled proxy engine.
@MainActor
public final class BundledCoreProcess: CoreProcessControlling {
    public private(set) var isRunning = false
    public var onExit: (@MainActor () -> Void)?

    /// The pid of the process this object launched, while it is alive.
    public private(set) var processIdentifier: Int32?

    private let executable: URL
    private let arguments: [String]
    private var process: Process?
    /// Set for a stop this object asked for, so that exit is not reported as
    /// a crash. Cleared when the exit arrives.
    private var suppressExitNotification = false
    private var exitWaiters: [ExitTicket] = []
    private var stdout: Pipe?
    private var stderr: Pipe?

    /// How long SIGINT is given before SIGTERM, and SIGTERM before SIGKILL.
    ///
    /// Bounds on an exit that is observed from the process itself. They are
    /// not a delay the caller sleeps instead of watching the process.
    public var gracefulTimeout: Duration = .seconds(4)
    public var terminateTimeout: Duration = .seconds(2)
    public var killTimeout: Duration = .seconds(2)

    public init(executable: URL, arguments: [String] = []) {
        self.executable = executable
        self.arguments = arguments
    }

    public func launch() throws {
        if isRunning { return }

        guard FileManager.default.isExecutableFile(atPath: executable.path) else {
            throw CoreProcessError.missingExecutable(executable.path)
        }

        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        process.terminationHandler = { [weak self] _ in
            Task { @MainActor in
                self?.didExit()
            }
        }

        do {
            try process.run()
        } catch {
            throw CoreProcessError.launchFailed(error.localizedDescription)
        }

        self.process = process
        processIdentifier = process.processIdentifier
        isRunning = true
        self.stdout = stdout
        self.stderr = stderr
        drain(stdout)
        drain(stderr)
    }

    public func stop() async {
        guard let process, let pid = processIdentifier, isRunning else {
            isRunning = false
            return
        }

        suppressExitNotification = true
        if process.isRunning {
            process.interrupt()
        }
        if await waitUntilStopped(gracefulTimeout) { return }

        guard owns(pid, process) else { return }
        process.terminate()
        if await waitUntilStopped(terminateTimeout) { return }

        guard owns(pid, process) else { return }
        Darwin.kill(pid, SIGKILL)
        _ = await waitUntilStopped(killTimeout)
    }

    /// Whether `pid` is still the process this object started.
    ///
    /// `Process.isRunning` is that process, not "whatever now has this pid",
    /// so a pid that has already exited is not signaled again.
    private func owns(_ pid: Int32, _ process: Process) -> Bool {
        isRunning && processIdentifier == pid && process.isRunning && process.processIdentifier == pid
    }

    private func didExit() {
        let notify = !suppressExitNotification
        suppressExitNotification = false
        isRunning = false
        process = nil
        processIdentifier = nil
        closePipes()

        let waiters = exitWaiters
        exitWaiters.removeAll()
        for waiter in waiters {
            waiter.resume()
        }

        if notify {
            onExit?()
        }
    }

    /// Resolves when the owned process exits, or when `timeout` passes,
    /// whichever is first. The timeout is the bound; the exit is the signal.
    private func waitUntilStopped(_ timeout: Duration) async -> Bool {
        if !isRunning { return true }

        let ticket = ExitTicket()
        let bound = timeout
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            if !self.isRunning {
                continuation.resume()
                return
            }
            ticket.continuation = continuation
            self.exitWaiters.append(ticket)
            Task { @MainActor in
                try? await Task.sleep(for: bound)
                ticket.resume()
            }
        }
        exitWaiters.removeAll { $0 === ticket }
        return !isRunning
    }

    /// Reads the child's output and drops it.
    ///
    /// A full pipe blocks the child. The text is not logged: startup lines
    /// name paths, and a captured exchange must not end up in a log either.
    private func drain(_ pipe: Pipe) {
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            if chunk.isEmpty {
                handle.readabilityHandler = nil
            }
        }
    }

    private func closePipes() {
        stdout?.fileHandleForReading.readabilityHandler = nil
        stderr?.fileHandleForReading.readabilityHandler = nil
        try? stdout?.fileHandleForReading.close()
        try? stderr?.fileHandleForReading.close()
        stdout = nil
        stderr = nil
    }
}

/// One waiter for the owned process to exit. Resumed by that exit or by the
/// timeout, and only once.
@MainActor
private final class ExitTicket {
    var continuation: CheckedContinuation<Void, Never>?

    func resume() {
        let pending = continuation
        continuation = nil
        pending?.resume()
    }
}
