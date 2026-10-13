//  A deadline for one lifecycle wait.
//
//  The work is what decides. The deadline is only the bound: when it passes,
//  or when Stop fires the wake, the caller continues. The work may still be
//  inside a system call. It must not touch the lifecycle after that unless
//  it still holds the generation it started with.
//
//  A dropped continuation would trap, so a wake that is discarded resumes
//  anyone still waiting on it.

import Foundation

/// Resumed by Stop, by a core exit, or when the signal is discarded.
final class WakeSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var fired = false

    deinit {
        fire()
    }

    func fire() {
        lock.lock()
        if fired {
            lock.unlock()
            return
        }
        fired = true
        let pending = waiters
        waiters.removeAll()
        lock.unlock()
        for waiter in pending {
            waiter.resume()
        }
    }

    func wait() async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.lock()
            if fired {
                lock.unlock()
                continuation.resume()
                return
            }
            waiters.append(continuation)
            lock.unlock()
        }
    }
}

/// The first result wins. A late result is discarded.
@MainActor
enum LifecycleBound {
    /// `nil` when `limit` passes or `wake` fires before `operation` returns.
    static func value<T: Sendable>(
        limit: Duration,
        wake: WakeSignal? = nil,
        _ operation: @escaping @MainActor () async -> T
    ) async -> T? {
        let gate = OnceGate()
        return await withCheckedContinuation { continuation in
            let finish: @MainActor (sending T?) -> Void = { value in
                guard gate.claim() else { return }
                continuation.resume(returning: value)
            }

            Task { @MainActor in
                finish(await operation())
            }
            Task { @MainActor in
                try? await Task.sleep(for: limit)
                finish(nil)
            }
            if let wake {
                Task { @MainActor in
                    await wake.wait()
                    finish(nil)
                }
            }
        }
    }

    /// The operation's own error, or `nil` when the bound wins.
    ///
    /// The result stays on the main actor. It is not sent across an isolation
    /// boundary, so `T` does not have to be sendable.
    static func required<T>(
        limit: Duration,
        _ operation: @escaping @MainActor () async throws -> T
    ) async throws -> T? {
        let box = OutcomeBox<T>()
        let finished: Void? = await value(limit: limit) {
            do {
                box.value = try await operation()
            } catch {
                box.error = error
            }
            return ()
        }
        if finished == nil { return nil }
        if let error = box.error { throw error }
        return box.value
    }
}

/// Holds a result on the main actor until the bound's caller reads it back.
private final class OutcomeBox<T> {
    var value: T?
    var error: Error?
}

/// One claim, on the main actor.
@MainActor
private final class OnceGate {
    private var claimed = false

    func claim() -> Bool {
        if claimed { return false }
        claimed = true
        return true
    }
}

/// One resume across threads. Provider-message callbacks are not on the main actor.
final class CallbackGate<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var delivered = false

    func finish(_ continuation: CheckedContinuation<T, Never>, _ value: sending T) {
        lock.lock()
        if delivered {
            lock.unlock()
            return
        }
        delivered = true
        lock.unlock()
        continuation.resume(returning: value)
    }
}
