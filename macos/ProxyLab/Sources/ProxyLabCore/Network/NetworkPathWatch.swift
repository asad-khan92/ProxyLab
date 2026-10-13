//  Network changes, as the system reports them.
//
//  `NWPathMonitor` calls once when it starts and again when the path changes.
//  A burst of those — an interface going down and another coming up — is
//  folded into one callback. Nothing here polls, and nothing here decides
//  what a path means for the proxy, the LAN listener, or the system proxy.
//  The callback does.

import Foundation
import Network

@MainActor
public final class NetworkPathWatch {
    public var onChange: (@MainActor () async -> Void)?

    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "com.proxylab.ProxyLab.network-path")
    private var coalesce: Task<Void, Never>?
    private var started = false

    public init() {}

    public func start() {
        guard !started else { return }
        started = true
        monitor.pathUpdateHandler = { [weak self] _ in
            Task { @MainActor in
                self?.schedule()
            }
        }
        monitor.start(queue: queue)
    }

    private func schedule() {
        coalesce?.cancel()
        coalesce = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(200))
            guard !Task.isCancelled else { return }
            await onChange?()
        }
    }
}
