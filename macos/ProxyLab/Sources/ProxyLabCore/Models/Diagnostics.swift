//  What the core says about itself when asked.
//
//      diagnostics ──▶ version, listeners, storage, IPC, recent problems
//
//  The shape is the core's. LAN exposure in the answer is the same status
//  `lan_status` returns, so this is not a second account of it. Nothing here
//  is a body, a header, or a path: the core removes those before it answers,
//  and the report built from this answer removes them again.

import Foundation

/// What the core says about one listener.
public struct DiagnosticEndpoint: Codable, Equatable, Sendable {
    /// `listening`, `unavailable`, or `unknown`.
    public var state: String
    /// The address it is bound to, when it is listening.
    public var address: String?
    /// Why it is not, when that is known.
    public var message: String?

    public init(state: String, address: String? = nil, message: String? = nil) {
        self.state = state
        self.address = address
        self.message = message
    }
}

/// What the core says about one subsystem that is not a listener.
public struct DiagnosticComponent: Codable, Equatable, Sendable {
    /// `open`, `serving`, `unavailable`, or `unknown`.
    public var state: String
    /// Why, when there is a reason a person can act on.
    public var message: String?

    public init(state: String, message: String? = nil) {
        self.state = state
        self.message = message
    }
}

/// One recent warning or error, already scrubbed by the core.
public struct DiagnosticEvent: Codable, Equatable, Sendable {
    /// `warn` or `error`.
    public var level: String
    /// Which part of ProxyLab it came from.
    public var component: String
    /// What that part was doing.
    public var operation: String
    /// What kind of failure it was.
    public var category: String
    /// The sentence, with secrets removed.
    public var message: String
    /// A host and port, when the failure was about one.
    public var endpoint: String?
    /// The exchange it belonged to, when there was one.
    public var exchangeId: String?
    /// How long the operation had been running, in milliseconds.
    public var durationMs: UInt64?
    /// When it happened, as milliseconds since the Unix epoch.
    public var atMs: UInt64

    public init(
        level: String,
        component: String,
        operation: String,
        category: String,
        message: String,
        endpoint: String? = nil,
        exchangeId: String? = nil,
        durationMs: UInt64? = nil,
        atMs: UInt64
    ) {
        self.level = level
        self.component = component
        self.operation = operation
        self.category = category
        self.message = message
        self.endpoint = endpoint
        self.exchangeId = exchangeId
        self.durationMs = durationMs
        self.atMs = atMs
    }

    private enum CodingKeys: String, CodingKey {
        case level, component, operation, category, message, endpoint
        case exchangeId = "exchange_id"
        case durationMs = "duration_ms"
        case atMs = "at_ms"
    }
}

/// The core's answer to `diagnostics`.
public struct CoreDiagnostics: Codable, Equatable, Sendable {
    /// The core's version.
    public var version: String
    /// `running` when this answer could be produced.
    public var core: String
    /// The loopback proxy listener.
    public var localhost: DiagnosticEndpoint
    /// The transparent-capture bridge.
    public var transparent: DiagnosticEndpoint
    /// LAN exposure, when this core is managing it.
    public var lan: LanStatusResult?
    /// Whether captured traffic is being stored.
    public var storage: DiagnosticComponent
    /// Whether the socket that answered is being served.
    public var ipc: DiagnosticComponent
    /// Recent warnings and errors, newest first.
    public var recent: [DiagnosticEvent]

    public init(
        version: String,
        core: String,
        localhost: DiagnosticEndpoint,
        transparent: DiagnosticEndpoint,
        lan: LanStatusResult? = nil,
        storage: DiagnosticComponent,
        ipc: DiagnosticComponent,
        recent: [DiagnosticEvent]
    ) {
        self.version = version
        self.core = core
        self.localhost = localhost
        self.transparent = transparent
        self.lan = lan
        self.storage = storage
        self.ipc = ipc
        self.recent = recent
    }
}
