//  One claimed flow, from the moment the provider takes it to the moment both
//  directions are closed.
//
//      claimed ──▶ connect to the bridge ──▶ CONNECT ──▶ 200 ──▶ copy bytes
//                        │                     │
//                        │ refused              └─ 508 ─┐
//                        ▼                              ▼
//                  core unreachable ─────────▶ carry it directly ──▶ copy bytes
//
//  A connection that is only waiting has not failed. The bridge is given up
//  on when nothing is listening; the direct connection is left alone until
//  it is ready or has failed. Ending either one on the wait that follows
//  the provider being enabled drops a flow this provider already claimed.
//
//  The flow is opened towards the application only once there is somewhere
//  for its bytes to go, so an application whose destination was refused sees
//  its connection fail rather than succeed and then go silent.
//
//  "Directly" is a connection this provider opens itself, to where the
//  application was going. The system never hands a provider its own
//  connections, so that one goes straight out; nothing is captured on it.
//
//  Bytes are copied and never looked at. Each direction half-closes on its
//  own, so an application that has finished sending still receives the
//  answer — the same rule the core's tunnels follow.

import Foundation
import Network
import NetworkExtension
import TransparentProxyKit
import os

/// The flows in progress, so that stopping the provider can close them.
final class RelayRegistry: Sendable {
    private let relays = OSAllocatedUnfairLock(initialState: [ObjectIdentifier: FlowRelay]())

    var count: Int { relays.withLock { $0.count } }

    func insert(_ relay: FlowRelay) {
        relays.withLock { $0[ObjectIdentifier(relay)] = relay }
    }

    func remove(_ relay: FlowRelay) {
        _ = relays.withLock { $0.removeValue(forKey: ObjectIdentifier(relay)) }
    }

    func cancelAll() {
        let all = relays.withLock { relays in
            defer { relays.removeAll() }
            return Array(relays.values)
        }
        all.forEach { $0.cancel() }
    }
}

/// Copies one flow to the core's bridge, or straight to its destination.
///
/// Everything it keeps is touched on `queue` alone: the flow's callbacks and
/// the connection's arrive on queues of their own, and each hops here first.
final class FlowRelay: @unchecked Sendable {
    private let flow: NEAppProxyTCPFlow
    private let destination: FlowDestination
    private let source: SourceProcess?
    private let bridgePort: UInt16
    private let policy: FlowPolicy
    private let registry: RelayRegistry
    private let queue = DispatchQueue(label: "com.proxylab.ProxyLab.TransparentProxy.relay")
    private let log = Logger(subsystem: "com.proxylab.ProxyLab.TransparentProxy", category: "relay")

    private var connection: NWConnection?
    private var connected = false
    /// Bytes are being copied, so there is no going back to another route.
    private var relaying = false
    private var finished = false
    private var applicationDoneSending = false
    private var destinationDoneSending = false

    /// The longest single read from either side.
    private static let chunk = 64 * 1_024

    init(
        flow: NEAppProxyTCPFlow,
        destination: FlowDestination,
        source: SourceProcess?,
        bridgePort: UInt16,
        policy: FlowPolicy,
        registry: RelayRegistry
    ) {
        self.flow = flow
        self.destination = destination
        self.source = source
        self.bridgePort = bridgePort
        self.policy = policy
        self.registry = registry
    }

    func start() {
        registry.insert(self)
        queue.async { self.connectToBridge() }
    }

    /// Closes both sides now, from the provider stopping.
    func cancel() {
        queue.async { self.finish(error: NEAppProxyFlowError(.aborted)) }
    }

    // MARK: - The bridge

    private func connectToBridge() {
        guard let request = BridgeHandshake.request(
            host: destination.host,
            port: destination.port,
            sourcePID: source?.pid
        ) else {
            // A destination that cannot be written into a request line is
            // not sent to the core as something else. It goes out as it was.
            connectDirectly()
            return
        }

        let bridge = NWConnection(
            host: NWEndpoint.Host(ProviderConfiguration.bridgeHost),
            port: NWEndpoint.Port(rawValue: bridgePort) ?? 9_091,
            using: .tcp
        )
        connection = bridge
        bridge.stateUpdateHandler = { [weak self] state in
            guard let self else { return }

            switch RelayConnectionPolicy.bridge(Self.condition(state)) {
            case .proceed:
                self.handshake(on: bridge, request: request)
            case .giveUp:
                // Refused, or failed outright: the core is not running.
                // `.waiting` with `ECONNREFUSED` is how a closed loopback
                // port shows itself. A wait for any other reason is the
                // route settling, and cancelling there drops the flow.
                self.bridgeUnreachable(bridge, reason: Self.describe(Self.socketError(state)))
            case .wait:
                break
            }
        }
        bridge.start(queue: queue)
    }

    private func handshake(on bridge: NWConnection, request: Data) {
        // A bridge that was already given up on can still report ready.
        // The direct connection is the one the flow is using by then.
        guard !connected, !finished, connection === bridge else { return }
        connected = true

        bridge.send(content: request, completion: .contentProcessed { [weak self] error in
            guard let self else { return }

            if let error {
                self.bridgeUnreachable(bridge, reason: Self.describe(error))
            } else {
                self.readReply(on: bridge, received: Data())
            }
        })
    }

    private func readReply(on bridge: NWConnection, received: Data) {
        bridge.receive(minimumIncompleteLength: 1, maximumLength: BridgeHandshake.maximumReplyHead) {
            [weak self] data, _, isComplete, error in
            guard let self else { return }

            var received = received
            if let data { received.append(data) }

            switch BridgeHandshake.read(received) {
            case .incomplete where error == nil && !isComplete:
                self.readReply(on: bridge, received: received)
            case .incomplete, .malformed:
                self.bridgeUnreachable(bridge, reason: "something other than the ProxyLab core answered")
            case let .complete(reply, length):
                self.policy.coreReached()
                self.bridgeReplied(bridge, reply: reply, leftover: received.dropFirst(length))
            }
        }
    }

    private func bridgeReplied(_ bridge: NWConnection, reply: BridgeHandshake.Reply, leftover: Data) {
        switch reply {
        case .established:
            relay(through: bridge, leftover: Data(leftover))
        case .passBack:
            // The core's own connection. Carried directly, and its process is
            // never offered again.
            policy.learnProxyLab(source)
            bridge.cancel()
            connection = nil
            connected = false
            connectDirectly()
        case let .refused(status):
            log.info("The core refused a flow with \(status)")
            bridge.cancel()
            finish(error: NEAppProxyFlowError(status == 502 ? .hostUnreachable : .refused))
        }
    }

    private func bridgeUnreachable(_ bridge: NWConnection, reason: String) {
        guard !finished, connection === bridge else { return }
        // A bridge that goes away mid-flow is a core that stopped: the bytes
        // already sent cannot be sent again somewhere else, so the flow ends.
        if relaying {
            finish(error: nil)
            return
        }

        log.notice("The core's bridge is unreachable; carrying flows directly for a while")
        policy.coreUnreachable(reason)
        bridge.cancel()
        connection = nil
        connected = false
        connectDirectly()
    }

    // MARK: - Directly

    private func connectDirectly() {
        guard !finished, let port = NWEndpoint.Port(rawValue: destination.port) else {
            finish(error: NEAppProxyFlowError(.invalidArgument))
            return
        }

        let direct = NWConnection(host: NWEndpoint.Host(destination.host), port: port, using: .tcp)
        connection = direct
        direct.stateUpdateHandler = { [weak self] state in
            guard let self else { return }

            switch RelayConnectionPolicy.direct(Self.condition(state)) {
            case .proceed:
                guard !self.connected, self.connection === direct else { return }
                self.connected = true
                self.relay(through: direct, leftover: Data())
            case .giveUp:
                self.finish(error: NEAppProxyFlowError(.hostUnreachable))
            case .wait:
                // The route is not up yet. The flow stays claimed until it
                // is, or until the connection fails. Ending it here is the
                // Mac losing a connection this provider already took.
                break
            }
        }
        direct.start(queue: queue)
    }

    // MARK: - Copying

    /// Opens the flow towards the application and copies both ways.
    ///
    /// `leftover` is whatever arrived after the bridge's reply head, which
    /// already belongs to the application.
    private func relay(through connection: NWConnection, leftover: Data) {
        relaying = true
        open { [weak self] error in
            guard let self else { return }
            guard error == nil else {
                self.finish(error: NEAppProxyFlowError(.notConnected))
                return
            }

            if leftover.isEmpty {
                self.destinationToApplication(connection)
            } else {
                self.write(leftover, then: connection)
            }
            self.applicationToDestination(connection)
        }
    }

    private func open(_ completion: @escaping @Sendable (Error?) -> Void) {
        let queue = self.queue
        if #available(macOS 15.0, *) {
            flow.open(withLocalFlowEndpoint: nil) { error in queue.async { completion(error) } }
        } else {
            flow.__open(withLocalEndpoint: nil) { error in queue.async { completion(error) } }
        }
    }

    private func applicationToDestination(_ connection: NWConnection) {
        let queue = self.queue
        // Strong: the flow calls this once, and the relay is what it is
        // calling back into. It is let go of when the flow finishes.
        flow.readData { data, error in
            queue.async {
                guard !self.finished else { return }

                if error != nil {
                    self.finish(error: nil)
                } else if let data, !data.isEmpty {
                    connection.send(content: data, completion: .contentProcessed { error in
                        if error != nil {
                            self.finish(error: nil)
                        } else {
                            self.applicationToDestination(connection)
                        }
                    })
                } else {
                    // The application finished sending: say so, and keep
                    // reading the answer.
                    connection.send(
                        content: nil,
                        contentContext: .finalMessage,
                        isComplete: true,
                        completion: .contentProcessed { _ in }
                    )
                    self.applicationDoneSending = true
                    self.finishIfBothDone()
                }
            }
        }
    }

    private func destinationToApplication(_ connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: Self.chunk) {
            [weak self] data, _, isComplete, error in
            guard let self, !self.finished else { return }

            if let data, !data.isEmpty {
                self.write(data, then: connection, closingAfter: isComplete)
            } else if isComplete {
                self.destinationFinished()
            } else if error != nil {
                self.finish(error: nil)
            } else {
                self.destinationToApplication(connection)
            }
        }
    }

    private func write(_ data: Data, then connection: NWConnection, closingAfter: Bool = false) {
        let queue = self.queue
        flow.write(data) { error in
            queue.async {
                guard !self.finished else { return }

                if error != nil {
                    self.finish(error: nil)
                } else if closingAfter {
                    self.destinationFinished()
                } else {
                    self.destinationToApplication(connection)
                }
            }
        }
    }

    private func destinationFinished() {
        flow.closeWriteWithError(nil)
        destinationDoneSending = true
        finishIfBothDone()
    }

    private func finishIfBothDone() {
        if applicationDoneSending && destinationDoneSending {
            finish(error: nil)
        }
    }

    /// Closes both sides, once.
    private func finish(error: Error?) {
        guard !finished else { return }
        finished = true

        flow.closeReadWithError(error)
        flow.closeWriteWithError(error)
        connection?.cancel()
        connection = nil
        registry.remove(self)
    }

    /// A sentence for the status the app is shown. No address, no host.
    private static func describe(_ error: NWError?) -> String {
        switch error {
        case .posix(.ECONNREFUSED):
            "the core is not listening on its bridge port"
        case let .posix(code):
            "the bridge connection failed (\(code.rawValue))"
        default:
            "the bridge connection failed"
        }
    }

    /// The part of a connection state the policy knows about.
    private static func condition(_ state: NWConnection.State) -> RelaySocketCondition {
        switch state {
        case .ready:
            return .ready
        case let .waiting(error):
            return .waiting(failure(error))
        case let .failed(error):
            return .failed(failure(error))
        default:
            return .other
        }
    }

    private static func failure(_ error: NWError) -> RelaySocketFailure {
        if case .posix(.ECONNREFUSED) = error { return .refused }

        return .other
    }

    private static func socketError(_ state: NWConnection.State) -> NWError? {
        switch state {
        case let .waiting(error), let .failed(error):
            return error
        default:
            return nil
        }
    }
}
