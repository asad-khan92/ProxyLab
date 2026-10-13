//  Putting the proxy on the Wi-Fi, so a real iPhone can be pointed at it.
//
//      lan_status ──▶ off, and here are this Mac's addresses
//           │
//      enable_lan ──▶ on, at 192.168.1.42:9090
//           │
//      disable_lan ─▶ off again
//
//  There is no automation here and there cannot be: an iPhone that is not a
//  simulator is a separate device on the same network, and nothing on this Mac
//  can reach into its Settings. So what this model does is make the proxy
//  reachable and say, exactly, what to type — the address, the port, and where
//  the certificate comes from.
//
//  # Off until someone says otherwise, every time
//
//  The chosen address is not written down. Nothing here reads a preference on
//  launch and nothing writes one on quit, so a ProxyLab that has just started
//  is on loopback and nothing else — even if LAN exposure was on when it was
//  last quit. That is deliberate: LAN exposure is an open HTTP proxy for
//  everything on the same Wi-Fi, and a thing with that consequence should be
//  turned on by a person who meant to, in the session they meant it in.
//
//  # The core decides, and this shows what it decided
//
//  Nothing flips on screen before the core says it flipped. The switch goes to
//  the core, the core binds or closes a socket, and the status that comes back
//  is what is drawn — so a switch that says "on" is a socket that is listening,
//  and never an intention.

import Foundation
import Observation

/// What the iPhone section is doing.
public enum LanExposureState: Equatable, Sendable {
    /// The status has not arrived yet.
    case loading
    /// The proxy is on loopback and nothing else.
    case off
    /// Local network access was turned on, and this Mac has no usable address.
    /// The loopback listener is unaffected. A later address turns the LAN
    /// listener back on without being asked again.
    case waitingForNetwork
    /// The switch is in flight; the core has not answered yet.
    case working
    /// The proxy is also on this address and port.
    case on(endpoint: String)
    /// The core could not be asked, or said no.
    case failed(String)
}

/// Everything the iPhone section shows.
@MainActor
@Observable
public final class IPhoneSetupModel {
    /// What the section is doing.
    public private(set) var state: LanExposureState = .loading
    /// The status as the core last reported it.
    public private(set) var status: LanStatusResult?
    /// Which address the picker has. One of ``addresses``, or `nil` when the
    /// core has offered none.
    public var selected: String?
    /// The last thing that went wrong, for the sentence under the buttons.
    public private(set) var lastError: String?
    /// Whether the setup instructions are on screen.
    public var showingInstructions = false

    private let session: any LanSession

    /// The user turned local network access on and has not turned it off.
    ///
    /// Kept when the LAN listener is closed because there is no address, so
    /// the listener comes back when an address does. Not written anywhere:
    /// a new launch starts with it off, the same as the core does.
    private var wantsLan = false

    /// The interface the default route is using, when a network change said.
    private var primaryInterface: String?

    /// Which request is the current one, so a slow answer to a switch that
    /// has been superseded does not replace what a newer one produced.
    private var generation = 0

    public init(session: any LanSession) {
        self.session = session
    }

    /// The addresses the core is offering right now.
    public var addresses: [LanAddress] { status?.available ?? [] }

    /// The port the proxy is listening on, once the core has said.
    public var port: UInt16? { status?.port }

    /// Whether the proxy is on the local network.
    public var isOn: Bool { status?.enabled == true }

    /// Whether anything is in flight.
    public var isWorking: Bool { state == .working }

    /// Whether there is an address to turn exposure on at.
    public var canEnable: Bool {
        !isWorking && !isOn && selected != nil
    }

    /// Whether there is anything to turn off, including a listener that is
    /// waiting for an address to come back.
    public var canDisable: Bool { !isWorking && (isOn || state == .waitingForNetwork) }

    /// What the status line says.
    public var statusText: String {
        switch state {
        case .loading: "Checking…"
        case .working: "Working…"
        case .off: "Off — only this Mac can reach the proxy"
        case .waitingForNetwork: "Waiting for a network — physical devices cannot reach the proxy"
        case let .on(endpoint): "On — \(endpoint)"
        case let .failed(reason): reason
        }
    }

    /// The sentence shown while the proxy is on the local network.
    ///
    /// Present whenever it is on and absent whenever it is not, because what
    /// it says is only true while it is on: anything on this Wi-Fi can use
    /// the proxy, and there is no password on it.
    public var exposureWarning: String? {
        guard case .on = state else { return nil }

        return "Any device on this Wi-Fi can use this proxy while it is on. "
            + "There is no password. Turn it off when you are done."
    }

    /// Asks the core where it is listening, and offers an address to pick.
    ///
    /// The selection is kept if the core still offers it, and replaced with
    /// the first address if it does not: a Mac that has changed networks has
    /// different addresses, and holding a stale one would offer a switch that
    /// could only fail. This does not move the LAN listener. A network change
    /// does that, through ``networkDidChange(primaryInterface:)``.
    public func refresh() async {
        await run { try await self.session.lanStatus() }
    }

    /// The network path changed. Refreshes the addresses the core offers and,
    /// when local network access is on, moves or suspends the LAN listener.
    ///
    /// Loopback is not part of this. `127.0.0.1:9090` and `127.0.0.1:9091`
    /// are not rebound because an interface changed.
    public func networkDidChange(primaryInterface: String?) async {
        self.primaryInterface = primaryInterface
        await run(quiet: true) { try await self.session.lanStatus() }
        guard wantsLan, lastError == nil, let status else { return }

        switch LanMigration.action(
            wantsLan: true,
            enabled: status.enabled,
            boundAddress: status.address,
            available: status.available,
            primaryInterface: primaryInterface
        ) {
        case .none:
            break
        case let .bind(address):
            await run { try await self.session.enableLan(address: address) }
        case .suspend:
            await suspendListener()
        case .wait:
            if !status.enabled {
                state = .waitingForNetwork
            }
        }
    }

    /// Puts the proxy on the selected address.
    public func enable() async {
        guard let selected else { return }

        await run { try await self.session.enableLan(address: selected) }
        if isOn { wantsLan = true }
    }

    /// Takes the proxy off the local network, including when it is only
    /// waiting for an address.
    public func disable() async {
        wantsLan = false
        if state == .waitingForNetwork && !isOn {
            state = .off
            return
        }
        await run { try await self.session.disableLan() }
        // A failed request did not close the listener. Keep following it.
        if lastError != nil {
            wantsLan = status?.enabled == true
        }
    }

    /// The steps a developer follows on the phone.
    ///
    /// Built from the status rather than written as fixed prose, because
    /// three of the five name the address and the port the core actually
    /// bound — which is the whole of what a person has to type.
    public var instructions: [String] {
        let endpoint = status?.endpoint
        let server = status?.address ?? "this Mac's Wi-Fi address"
        let port = status.map { String($0.port) } ?? "the proxy port"

        return [
            endpoint == nil
                ? "Turn on local network access above, and pick this Mac's Wi-Fi address."
                : "Join the iPhone to the same Wi-Fi network as this Mac.",
            "On the iPhone, open Settings › Wi-Fi, tap the ⓘ beside the network, "
                + "scroll to HTTP Proxy and choose Manual.",
            "Enter Server \(server) and Port \(port), then tap Save.",
            "In Safari on the iPhone, open http://\(Self.setupHost)/ and follow the page "
                + "to install the ProxyLab certificate.",
            "Open Settings › General › About › Certificate Trust Settings and turn ProxyLab on. "
                + "HTTPS is not readable until this step is done.",
        ]
    }

    /// The host the core answers as itself, where the certificate comes from.
    ///
    /// The Rust side's `setup::SETUP_HOST`, mirrored here for the same reason
    /// ``CoreLocations`` mirrors the listen address: one place in this app
    /// spells it, and it follows the core when the core moves.
    public static let setupHost = "proxylab.test"

    /// Runs one core operation and draws whatever it produced.
    private func run(
        quiet: Bool = false,
        _ operation: @escaping () async throws -> LanStatusResult
    ) async {
        generation += 1
        let mine = generation
        if !quiet { state = .working }
        lastError = nil

        do {
            let status = try await operation()
            guard mine == generation else { return }

            apply(status)
        } catch {
            guard mine == generation else { return }

            let message = (error as? IPCError)?.message ?? "The ProxyLab core could not be asked."
            lastError = message
            state = .failed(message)
        }
    }

    /// Closes the LAN listener and keeps the request for it, so an address
    /// that comes back is bound without being asked for again.
    private func suspendListener() async {
        let wanted = wantsLan
        await run { try await self.session.disableLan() }
        wantsLan = wanted
        if wantsLan, !isOn, lastError == nil {
            state = .waitingForNetwork
        }
    }

    /// Takes the core's answer as the truth about what is listening.
    private func apply(_ status: LanStatusResult) {
        self.status = status
        let boundUsable = status.enabled && status.address.map { bound in
            status.available.contains { $0.address == bound }
        } == true

        if boundUsable, let bound = status.address {
            // The proxy is on an address the Mac still has, so that is the
            // address shown. A picker reading one address while the proxy is
            // listening on another is how a developer types the wrong one
            // into a phone.
            selected = bound
        } else if selected == nil
            || !status.available.contains(where: { $0.address == selected })
        {
            // Nothing usable is bound, so keep what was picked if this Mac
            // still offers it, and otherwise fall to the first address there
            // is. An address that is no longer offered is not kept: that is
            // the stale LAN address.
            selected = status.available.first?.address
        }

        if status.enabled { wantsLan = true }

        if boundUsable, let endpoint = status.endpoint {
            state = .on(endpoint: endpoint)
        } else if wantsLan, status.available.isEmpty {
            state = .waitingForNetwork
        } else {
            state = .off
        }
    }
}
