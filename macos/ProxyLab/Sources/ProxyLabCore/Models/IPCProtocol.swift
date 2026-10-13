//  The wire protocol, as the client speaks it.
//
//      client ──▶ {"version":1,"request_id":"1","type":"list_traffic","limit":200}
//      server ──▶ {"version":1,"request_id":"1","ok":true,"type":"list_traffic","result":{…}}
//      server ──▶ {"version":1,"type":"traffic_added","exchange":{…}}
//
//  One JSON object per line, in both directions. A line is a complete message:
//  there is no length prefix and no handshake.
//
//  # Telling a response from an event
//
//  A response carries `ok` and the `request_id` it answers. An event carries
//  neither, because nothing asked for it. So `ok` is the discriminator, and
//  that is what ``IPCMessage`` decodes on — not the `type` field, which both
//  kinds of message have and which means different things in each.
//
//  # Versioning
//
//  Every request names ``IPCProtocolVersion/current`` and every message from
//  the server carries it. A message naming another version is refused rather
//  than guessed at: a client that disagrees with the server about the schema
//  will misread whatever it is sent.
//
//  # Nothing optional is sent as null
//
//  The server refuses parameters it does not know, and an operation that takes
//  none is sent none. Swift's synthesized encoding omits `nil` rather than
//  writing `null`, and ``IPCRequest`` encodes explicitly for the same reason:
//  a `"cursor": null` on a `clear_traffic` request would be refused.

import Foundation

/// The protocol version this client speaks.
public enum IPCProtocolVersion {
    /// The one version there is. A message naming another is refused.
    public static let current = 1
}

/// What the core will read.
public enum IPCLimits {
    /// The longest request line the core reads, in bytes.
    ///
    /// A longer one is answered with `message_too_large` and **no request
    /// id** — the core does not buffer an oversized line, so it never reads
    /// far enough to know which request it was. That answer therefore
    /// correlates with nothing, and a client that sent such a line would wait
    /// for an answer that cannot come.
    ///
    /// So this is a limit the app checks *before* writing, which it can do
    /// exactly: see ``IPCRequest/isWithinCoreLimit``. Every request the older
    /// operations send is metadata and nowhere near it; a replayed request
    /// carries a body, and is the one that can reach it.
    public static let maxRequestBytes = 65_536
}

// MARK: - Requests

/// What the client is asking the server to do.
public enum IPCOperation: Sendable, Equatable {
    /// A page of the traffic list.
    case listTraffic(ListTrafficParameters)
    /// One exchange, whole.
    case getTraffic(exchangeID: String)
    /// A page of stream messages on one exchange.
    case listStreamMessages(exchangeID: String, after: UInt64? = nil, limit: Int? = nil)
    /// One stream message, payload included.
    case getStreamMessage(streamMessageID: String)
    /// Remove one exchange.
    case deleteTraffic(exchangeID: String)
    /// Remove all of them.
    case clearTraffic
    /// Send this connection every exchange from now on.
    case subscribe
    /// Every rule, in the order they are consulted in.
    case listRules
    /// One rule.
    case getRule(ruleID: String)
    /// Store a new rule.
    case createRule(NewRule)
    /// Change a rule that exists.
    case updateRule(ruleID: String, RuleUpdate)
    /// Remove a rule.
    case deleteRule(ruleID: String)
    /// Read a cURL command as the request it describes.
    case parseCurl(String)
    /// Write a stored request out as the cURL command that would send it
    /// again.
    case curlFromTraffic(exchangeID: String)
    /// Send a request through the proxy, as a new exchange.
    case replayRequest(ReplayRequest)
    /// Whether the proxy is on the local network, and which addresses it
    /// could use.
    case lanStatus
    /// Start listening on this address of this Mac, as well as on loopback.
    case enableLan(address: String)
    /// Stop listening on the local network, leaving loopback alone.
    case disableLan
    /// Every configured breakpoint.
    case listBreakpoints
    /// Configure a new breakpoint.
    case createBreakpoint(NewBreakpoint)
    /// Change one that exists.
    case updateBreakpoint(breakpointID: String, BreakpointUpdate)
    /// Remove one.
    case deleteBreakpoint(breakpointID: String)
    /// Send this connection every paused exchange from now on.
    ///
    /// Also what makes the core pause at all: a core nobody is watching holds
    /// nothing still, which is what keeps a request from being stranded if
    /// this app goes away.
    case watchBreakpoints
    /// Let a paused exchange carry on, with whatever it carries changed.
    case continueBreakpoint(
        pauseID: String,
        request: RequestEdit?,
        response: ResponseEdit?,
        websocket: WebSocketEdit? = nil
    )
    /// Stop a paused exchange here.
    case cancelBreakpoint(pauseID: String)
    /// How the core is shaping the traffic it carries.
    case throttleStatus
    /// Shape it this way from the next exchange on.
    case setThrottle(ThrottleSettings)
    /// Every saved request, in the order they were saved.
    case listSavedRequests
    /// One saved request.
    case getSavedRequest(savedRequestID: String)
    /// Keep a request this app has described.
    case createSavedRequest(NewSavedRequest)
    /// Keep the request of one captured exchange, under this name.
    case saveRequestFromTraffic(exchangeID: String, name: String)
    /// Change one that exists.
    case updateSavedRequest(savedRequestID: String, SavedRequestUpdate)
    /// Remove one.
    case deleteSavedRequest(savedRequestID: String)
    /// Write captured traffic out as a HAR file at this path.
    case exportHar(HarExportRequest)
    /// Read the HAR file at this path, and keep what it describes.
    case importHar(path: String)
    /// Where the core stands, and the recent problems it has kept.
    case diagnostics

    /// What this operation is called on the wire.
    public var name: String {
        switch self {
        case .listTraffic: "list_traffic"
        case .getTraffic: "get_traffic"
        case .listStreamMessages: "list_stream_messages"
        case .getStreamMessage: "get_stream_message"
        case .deleteTraffic: "delete_traffic"
        case .clearTraffic: "clear_traffic"
        case .subscribe: "subscribe"
        case .listRules: "list_rules"
        case .getRule: "get_rule"
        case .createRule: "create_rule"
        case .updateRule: "update_rule"
        case .deleteRule: "delete_rule"
        case .parseCurl: "parse_curl"
        case .curlFromTraffic: "curl_from_traffic"
        case .replayRequest: "replay_request"
        case .lanStatus: "lan_status"
        case .enableLan: "enable_lan"
        case .disableLan: "disable_lan"
        case .listBreakpoints: "list_breakpoints"
        case .createBreakpoint: "create_breakpoint"
        case .updateBreakpoint: "update_breakpoint"
        case .deleteBreakpoint: "delete_breakpoint"
        case .watchBreakpoints: "watch_breakpoints"
        case .continueBreakpoint: "continue_breakpoint"
        case .cancelBreakpoint: "cancel_breakpoint"
        case .throttleStatus: "throttle_status"
        case .setThrottle: "set_throttle"
        case .listSavedRequests: "list_saved_requests"
        case .getSavedRequest: "get_saved_request"
        case .createSavedRequest: "create_saved_request"
        case .saveRequestFromTraffic: "save_request_from_traffic"
        case .updateSavedRequest: "update_saved_request"
        case .deleteSavedRequest: "delete_saved_request"
        case .exportHar: "export_har"
        case .importHar: "import_har"
        case .diagnostics: "diagnostics"
        }
    }
}

/// Which responses a list request is asking for.
public enum StatusFilter: Codable, Sendable, Equatable {
    /// One status code exactly.
    case exact(UInt16)
    /// A class of status code: `4` for every `4xx`.
    case `class`(UInt8)

    private enum CodingKeys: String, CodingKey {
        case exact
        case `class`
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        if let exact = try container.decodeIfPresent(UInt16.self, forKey: .exact) {
            self = .exact(exact)
        } else if let group = try container.decodeIfPresent(UInt8.self, forKey: .class) {
            self = .class(group)
        } else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a status filter is an exact code or a class"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .exact(status): try container.encode(status, forKey: .exact)
        case let .class(group): try container.encode(group, forKey: .class)
        }
    }
}

/// Where a page of traffic stopped.
///
/// Opaque to the client: it is handed back to ask for the next page and is
/// never built here. The fields are the server's, and are modelled rather than
/// carried as loose JSON so that a page request round-trips exactly.
public struct TrafficCursor: Codable, Hashable, Sendable {
    /// The millisecond the exchange started in.
    public let startedAtMs: Int64
    /// Which exchange, among those that started in that millisecond.
    public let id: String

    public init(startedAtMs: Int64, id: String) {
        self.startedAtMs = startedAtMs
        self.id = id
    }

    private enum CodingKeys: String, CodingKey {
        case startedAtMs = "started_at_ms"
        case id
    }
}

/// Which end of the stored order a page is read from.
///
/// Not a sort of its own: the core keeps one order — the millisecond an
/// exchange started in, then its id — and this says which end to read it
/// from. A cursor names a place in that order, so a page request that
/// changes this starts again without one.
public enum TrafficOrder: String, Codable, Sendable, Equatable {
    /// The most recent exchange first.
    case newest
    /// The earliest exchange first.
    case oldest
}

/// The parameters of a request for a page of the traffic list.
///
/// Every one of them is optional, and an omitted one is not sent at all. The
/// server refuses a parameter it does not know, so this is the whole of what
/// a list request may carry.
public struct ListTrafficParameters: Sendable, Equatable {
    /// How many exchanges to return. The server's own default when `nil`,
    /// and at most `1000`.
    public var limit: Int?
    /// The page to carry on from, from a previous page's `next_cursor`.
    public var cursor: TrafficCursor?
    /// Only exchanges for this host.
    public var host: String?
    /// Only exchanges with this method.
    public var method: String?
    /// Only exchanges whose response matches.
    public var status: StatusFilter?
    /// Only exchanges with this text in captured traffic.
    ///
    /// A substring by default, matched by the core against URL, host, path,
    /// method, status, headers, bodies, and stream-message text.
    public var search: String?
    /// Whether `search` is a regular expression.
    public var searchRegex: Bool
    /// Whether `search` distinguishes letter case.
    public var searchCaseSensitive: Bool
    /// Only exchanges whose `Content-Type` contains this.
    public var contentType: String?
    /// Only exchanges framed with this HTTP version on either half.
    public var httpVersion: HTTPVersion?
    /// Only WebSocket handshakes.
    public var websocket: Bool
    /// Only Server-Sent Event responses.
    public var sse: Bool
    /// Only exchanges that ended in a proxy error.
    public var failed: Bool
    /// Which end of the order to read from. The newest first when omitted.
    public var order: TrafficOrder?
    /// Only exchanges that started at or after this moment.
    public var startedAtOrAfter: Timestamp?
    /// Only exchanges that started before it.
    public var startedBefore: Timestamp?

    public init(
        limit: Int? = nil,
        cursor: TrafficCursor? = nil,
        host: String? = nil,
        method: String? = nil,
        status: StatusFilter? = nil,
        search: String? = nil,
        searchRegex: Bool = false,
        searchCaseSensitive: Bool = false,
        contentType: String? = nil,
        httpVersion: HTTPVersion? = nil,
        websocket: Bool = false,
        sse: Bool = false,
        failed: Bool = false,
        order: TrafficOrder? = nil,
        startedAtOrAfter: Timestamp? = nil,
        startedBefore: Timestamp? = nil
    ) {
        self.limit = limit
        self.cursor = cursor
        self.host = host
        self.method = method
        self.status = status
        self.search = search
        self.searchRegex = searchRegex
        self.searchCaseSensitive = searchCaseSensitive
        self.contentType = contentType
        self.httpVersion = httpVersion
        self.websocket = websocket
        self.sse = sse
        self.failed = failed
        self.order = order
        self.startedAtOrAfter = startedAtOrAfter
        self.startedBefore = startedBefore
    }
}

/// One request, ready to be written as a line.
public struct IPCRequest: Encodable, Sendable, Equatable {
    /// The client's label for this request, which its response echoes.
    public let requestID: String
    /// What is being asked for.
    public let operation: IPCOperation

    public init(requestID: String, operation: IPCOperation) {
        self.requestID = requestID
        self.operation = operation
    }

    /// Whether this request is one the core will read.
    ///
    /// Measured rather than estimated: the request is encoded exactly as it
    /// would be sent, and the newline that ends the line is counted. See
    /// ``IPCLimits/maxRequestBytes`` for why the app has to ask.
    public var isWithinCoreLimit: Bool {
        guard let line = try? JSONEncoder().encode(self) else { return false }

        return line.count + 1 <= IPCLimits.maxRequestBytes
    }

    /// The keys every request carries, plus every parameter any of them may.
    ///
    /// One flat object: the operation's parameters sit beside `type` rather
    /// than under it, which is the shape the server parses.
    private enum CodingKeys: String, CodingKey {
        case version
        case requestID = "request_id"
        case type
        case exchangeID = "exchange_id"
        case limit
        case cursor
        case host
        case method
        case status
        case search
        case searchRegex = "search_regex"
        case searchCaseSensitive = "search_case_sensitive"
        case contentType = "content_type"
        case httpVersion = "http_version"
        case websocket
        case sse
        case failed
        case order
        case startedAtOrAfter = "started_at_or_after"
        case startedBefore = "started_before"
        case ruleID = "rule_id"
        case enabled
        case name
        case matcher
        case action
        case priority
        case curl
        case url
        case headers
        case body
        case address
        case breakpointID = "breakpoint_id"
        case direction
        case pauseID = "pause_id"
        case request
        case response
        case latencyMs = "latency_ms"
        case downloadBytesPerSecond = "download_bytes_per_second"
        case uploadBytesPerSecond = "upload_bytes_per_second"
        case savedRequestID = "saved_request_id"
        case streamMessageID = "stream_message_id"
        case after
        case path
        case exchangeIDs = "exchange_ids"
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(IPCProtocolVersion.current, forKey: .version)
        try container.encode(requestID, forKey: .requestID)
        try container.encode(operation.name, forKey: .type)

        switch operation {
        case let .listTraffic(parameters):
            try container.encodeIfPresent(parameters.limit, forKey: .limit)
            try container.encodeIfPresent(parameters.cursor, forKey: .cursor)
            try container.encodeIfPresent(parameters.host, forKey: .host)
            try container.encodeIfPresent(parameters.method, forKey: .method)
            try container.encodeIfPresent(parameters.status, forKey: .status)
            try container.encodeIfPresent(parameters.search, forKey: .search)
            if parameters.searchRegex {
                try container.encode(true, forKey: .searchRegex)
            }
            if parameters.searchCaseSensitive {
                try container.encode(true, forKey: .searchCaseSensitive)
            }
            try container.encodeIfPresent(parameters.contentType, forKey: .contentType)
            try container.encodeIfPresent(parameters.httpVersion, forKey: .httpVersion)
            if parameters.websocket {
                try container.encode(true, forKey: .websocket)
            }
            if parameters.sse {
                try container.encode(true, forKey: .sse)
            }
            if parameters.failed {
                try container.encode(true, forKey: .failed)
            }
            try container.encodeIfPresent(parameters.order, forKey: .order)
            try container.encodeIfPresent(parameters.startedAtOrAfter, forKey: .startedAtOrAfter)
            try container.encodeIfPresent(parameters.startedBefore, forKey: .startedBefore)
        case let .getTraffic(exchangeID), let .deleteTraffic(exchangeID):
            try container.encode(exchangeID, forKey: .exchangeID)
        case let .listStreamMessages(exchangeID, after, limit):
            try container.encode(exchangeID, forKey: .exchangeID)
            try container.encodeIfPresent(after, forKey: .after)
            try container.encodeIfPresent(limit, forKey: .limit)
        case let .getStreamMessage(streamMessageID):
            try container.encode(streamMessageID, forKey: .streamMessageID)
        case .clearTraffic, .subscribe, .listRules, .lanStatus, .disableLan, .listBreakpoints,
            .watchBreakpoints, .throttleStatus, .listSavedRequests, .diagnostics:
            // None of them takes a parameter, and the server refuses one that
            // is sent any — including a key whose value is null.
            break
        case let .enableLan(address):
            try container.encode(address, forKey: .address)
        case let .getRule(ruleID), let .deleteRule(ruleID):
            try container.encode(ruleID, forKey: .ruleID)
        case let .createRule(rule):
            // Flattened, as the server parses it: the rule's own fields sit
            // beside `type` rather than under a `rule` key.
            try container.encode(rule.enabled, forKey: .enabled)
            try container.encode(rule.name, forKey: .name)
            try container.encode(rule.matcher, forKey: .matcher)
            try container.encode(rule.action, forKey: .action)
            try container.encode(rule.priority, forKey: .priority)
        case let .parseCurl(command):
            try container.encode(command, forKey: .curl)
        case let .curlFromTraffic(exchangeID):
            try container.encode(exchangeID, forKey: .exchangeID)
        case let .replayRequest(request):
            // Flattened, as the core parses it: the request's own fields sit
            // beside `type` rather than under a `request` key, which is how
            // `create_rule` carries a rule too. The body is `Data`, which
            // `JSONEncoder` writes as the base64 the core reads.
            try container.encode(request.method, forKey: .method)
            try container.encode(request.url, forKey: .url)
            try container.encode(request.headers, forKey: .headers)
            try container.encode(request.body, forKey: .body)
        case let .createBreakpoint(breakpoint):
            // Flattened, as the core parses it: the breakpoint's own fields
            // sit beside `type` rather than under a `breakpoint` key, which
            // is how `create_rule` carries a rule too.
            try container.encode(breakpoint.enabled, forKey: .enabled)
            try container.encode(breakpoint.direction, forKey: .direction)
            try container.encode(breakpoint.matcher, forKey: .matcher)
        case let .updateBreakpoint(breakpointID, update):
            try container.encode(breakpointID, forKey: .breakpointID)
            try container.encodeIfPresent(update.enabled, forKey: .enabled)
            try container.encodeIfPresent(update.direction, forKey: .direction)
            try container.encodeIfPresent(update.matcher, forKey: .matcher)
        case let .deleteBreakpoint(breakpointID):
            try container.encode(breakpointID, forKey: .breakpointID)
        case let .continueBreakpoint(pauseID, request, response, websocket):
            // Not flattened, unlike a rule or a breakpoint: a request edit and
            // a response edit both carry `headers` and `body`, so the two have
            // to be named apart.
            try container.encode(pauseID, forKey: .pauseID)
            try container.encodeIfPresent(request, forKey: .request)
            try container.encodeIfPresent(response, forKey: .response)
            try container.encodeIfPresent(websocket, forKey: .websocket)
        case let .cancelBreakpoint(pauseID):
            try container.encode(pauseID, forKey: .pauseID)
        case let .setThrottle(settings):
            try container.encode(settings.enabled, forKey: .enabled)
            try container.encode(settings.latencyMs, forKey: .latencyMs)
            try container.encode(
                settings.downloadBytesPerSecond,
                forKey: .downloadBytesPerSecond
            )
            try container.encode(settings.uploadBytesPerSecond, forKey: .uploadBytesPerSecond)
        case let .updateRule(ruleID, update):
            try container.encode(ruleID, forKey: .ruleID)
            // Only the fields the change names. One that is not sent is one
            // the server leaves as it was, which is what keeps an edit from
            // overwriting something changed elsewhere in the meantime.
            try container.encodeIfPresent(update.enabled, forKey: .enabled)
            try container.encodeIfPresent(update.name, forKey: .name)
            try container.encodeIfPresent(update.matcher, forKey: .matcher)
            try container.encodeIfPresent(update.action, forKey: .action)
            try container.encodeIfPresent(update.priority, forKey: .priority)
        case let .getSavedRequest(savedRequestID), let .deleteSavedRequest(savedRequestID):
            try container.encode(savedRequestID, forKey: .savedRequestID)
        case let .createSavedRequest(request):
            // Flattened, as the core parses it: the request's own fields sit
            // beside `type` rather than under a `saved_request` key, which is
            // how `create_rule` carries a rule too. The body is `Data`, which
            // `JSONEncoder` writes as the base64 the core reads.
            try container.encode(request.name, forKey: .name)
            try container.encode(request.method, forKey: .method)
            try container.encode(request.url, forKey: .url)
            try container.encode(request.headers, forKey: .headers)
            try container.encode(request.body, forKey: .body)
        case let .saveRequestFromTraffic(exchangeID, name):
            // The exchange and what to call it, and nothing else: what the
            // request *was* is the core's to read out of its own store, which
            // is why `curl_from_traffic` carries an id and not a request.
            try container.encode(exchangeID, forKey: .exchangeID)
            try container.encode(name, forKey: .name)
        case let .updateSavedRequest(savedRequestID, update):
            try container.encode(savedRequestID, forKey: .savedRequestID)
            try container.encodeIfPresent(update.name, forKey: .name)
            try container.encodeIfPresent(update.method, forKey: .method)
            try container.encodeIfPresent(update.url, forKey: .url)
            try container.encodeIfPresent(update.headers, forKey: .headers)
            try container.encodeIfPresent(update.body, forKey: .body)
        case let .exportHar(export):
            try container.encode(export.path, forKey: .path)

            switch export.selection {
            case let .exchanges(ids):
                try container.encode(ids, forKey: .exchangeIDs)
            case let .filtered(filters):
                // The same filter fields a list request carries, and never
                // beside `exchange_ids`: the core refuses a request that
                // names both. `limit`, `cursor` and `order` are not among
                // them — an export is every match, oldest first.
                try container.encodeIfPresent(filters.host, forKey: .host)
                try container.encodeIfPresent(filters.method, forKey: .method)
                try container.encodeIfPresent(filters.status, forKey: .status)
                try container.encodeIfPresent(filters.search, forKey: .search)
                if filters.searchRegex {
                    try container.encode(true, forKey: .searchRegex)
                }
                if filters.searchCaseSensitive {
                    try container.encode(true, forKey: .searchCaseSensitive)
                }
                try container.encodeIfPresent(filters.contentType, forKey: .contentType)
                try container.encodeIfPresent(filters.httpVersion, forKey: .httpVersion)
                if filters.websocket {
                    try container.encode(true, forKey: .websocket)
                }
                if filters.sse {
                    try container.encode(true, forKey: .sse)
                }
                if filters.failed {
                    try container.encode(true, forKey: .failed)
                }
                try container.encodeIfPresent(
                    filters.startedAtOrAfter,
                    forKey: .startedAtOrAfter
                )
                try container.encodeIfPresent(filters.startedBefore, forKey: .startedBefore)
            }
        case let .importHar(path):
            try container.encode(path, forKey: .path)
        }
    }
}

// MARK: - Responses

/// What a successful request produced.
public enum IPCResult: Sendable, Equatable {
    /// A page of the traffic list.
    case listTraffic(TrafficListResult)
    /// One exchange, whole.
    case getTraffic(TrafficExchange)
    /// A page of stream messages on one exchange.
    case listStreamMessages(StreamMessageListResult)
    /// One stream message, payload included.
    case getStreamMessage(StreamMessage)
    /// One exchange removed, or not there to remove.
    case deleteTraffic(DeletedResult)
    /// The history emptied.
    case clearTraffic(ClearedResult)
    /// This connection is now receiving traffic events.
    case subscribe
    /// Every rule, in the order they are consulted in.
    case listRules(RuleListResult)
    /// One rule, as the core now holds it.
    case getRule(Rule)
    /// A rule that has just been created.
    case createRule(Rule)
    /// A rule as it is after being changed.
    case updateRule(Rule)
    /// One rule removed, or not there to remove.
    case deleteRule(RuleDeletedResult)
    /// A cURL command read as the request it describes, or as the reason it
    /// describes none.
    case parseCurl(CurlParseResult)
    /// A stored request written out as a cURL command.
    case curlFromTraffic(CurlGenerated)
    /// A request sent again, and what came back.
    case replayRequest(ReplayResult)
    /// Where the proxy is listening, and where it could.
    case lanStatus(LanStatusResult)
    /// The same, after being put on the local network.
    case enableLan(LanStatusResult)
    /// The same, after being taken off it.
    case disableLan(LanStatusResult)
    /// Every configured breakpoint.
    case listBreakpoints(BreakpointListResult)
    /// A breakpoint that has just been configured.
    case createBreakpoint(Breakpoint)
    /// A breakpoint as it is after being changed.
    case updateBreakpoint(Breakpoint)
    /// One breakpoint removed, or not there to remove.
    case deleteBreakpoint(BreakpointDeletedResult)
    /// This connection is now receiving paused exchanges.
    case watchBreakpoints
    /// A paused exchange told to carry on, or not there to tell.
    case continueBreakpoint(PauseResolvedResult)
    /// A paused exchange told to stop, or not there to tell.
    case cancelBreakpoint(PauseResolvedResult)
    /// How the core is shaping the traffic it carries.
    case throttleStatus(ThrottleSettings)
    /// The same, after being changed.
    case setThrottle(ThrottleSettings)
    /// Every saved request, in the order they were saved.
    case listSavedRequests(SavedRequestListResult)
    /// One saved request, as the core now holds it.
    case getSavedRequest(SavedRequest)
    /// A request that has just been saved.
    case createSavedRequest(SavedRequest)
    /// The same, saved out of one captured exchange.
    case saveRequestFromTraffic(SavedRequest)
    /// A saved request as it is after being changed.
    case updateSavedRequest(SavedRequest)
    /// One saved request removed, or not there to remove.
    case deleteSavedRequest(SavedRequestDeletedResult)
    /// Captured traffic written out as a HAR file.
    case exportHar(HarExportResult)
    /// A HAR file read in as captured traffic.
    case importHar(HarImportResult)
    /// Where the core stands, and the recent problems it has kept.
    case diagnostics(CoreDiagnostics)
}

/// A page of the traffic list.
public struct TrafficListResult: Codable, Sendable, Equatable {
    /// The exchanges, newest first.
    public let exchanges: [ExchangeSummary]
    /// Where to carry on from, or `nil` at the end of the list.
    public let nextCursor: TrafficCursor?
    /// How many exchanges the filter matches in total, across every page.
    public let total: UInt64

    public init(exchanges: [ExchangeSummary], nextCursor: TrafficCursor?, total: UInt64) {
        self.exchanges = exchanges
        self.nextCursor = nextCursor
        self.total = total
    }

    private enum CodingKeys: String, CodingKey {
        case exchanges
        case nextCursor = "next_cursor"
        case total
    }
}

/// The result of removing one exchange.
public struct DeletedResult: Codable, Sendable, Equatable {
    /// Which exchange.
    public let exchangeID: String
    /// Whether there was one to remove.
    public let deleted: Bool

    public init(exchangeID: String, deleted: Bool) {
        self.exchangeID = exchangeID
        self.deleted = deleted
    }

    private enum CodingKeys: String, CodingKey {
        case exchangeID = "exchange_id"
        case deleted
    }
}

/// The result of emptying the history.
public struct ClearedResult: Codable, Sendable, Equatable {
    /// How many exchanges were removed.
    public let removed: UInt64

    public init(removed: UInt64) {
        self.removed = removed
    }
}

/// What kind of failure a response is reporting.
///
/// A wrapper around the name rather than a closed enum, so a code this build
/// has not heard of is still a code the client can show rather than a message
/// it cannot decode at all.
public struct IPCErrorCode: RawRepresentable, Codable, Hashable, Sendable {
    public let rawValue: String

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    public static let invalidJSON = IPCErrorCode(rawValue: "invalid_json")
    public static let unsupportedVersion = IPCErrorCode(rawValue: "unsupported_version")
    public static let missingRequestID = IPCErrorCode(rawValue: "missing_request_id")
    public static let invalidRequestID = IPCErrorCode(rawValue: "invalid_request_id")
    public static let unknownOperation = IPCErrorCode(rawValue: "unknown_operation")
    public static let invalidParameters = IPCErrorCode(rawValue: "invalid_parameters")
    public static let invalidExchangeID = IPCErrorCode(rawValue: "invalid_exchange_id")
    public static let invalidStreamMessageID = IPCErrorCode(rawValue: "invalid_stream_message_id")
    public static let invalidRuleID = IPCErrorCode(rawValue: "invalid_rule_id")
    public static let invalidRule = IPCErrorCode(rawValue: "invalid_rule")
    public static let invalidRequest = IPCErrorCode(rawValue: "invalid_request")
    public static let notFound = IPCErrorCode(rawValue: "not_found")
    public static let messageTooLarge = IPCErrorCode(rawValue: "message_too_large")
    public static let alreadySubscribed = IPCErrorCode(rawValue: "already_subscribed")
    public static let storageUnavailable = IPCErrorCode(rawValue: "storage_unavailable")
    public static let lanUnavailable = IPCErrorCode(rawValue: "lan_unavailable")
    public static let invalidSavedRequestID = IPCErrorCode(rawValue: "invalid_saved_request_id")
    public static let invalidSavedRequest = IPCErrorCode(rawValue: "invalid_saved_request")
    public static let harUnavailable = IPCErrorCode(rawValue: "har_unavailable")
    public static let invalidBreakpointID = IPCErrorCode(rawValue: "invalid_breakpoint_id")
    public static let invalidBreakpoint = IPCErrorCode(rawValue: "invalid_breakpoint")
    public static let invalidPauseID = IPCErrorCode(rawValue: "invalid_pause_id")
    public static let `internal` = IPCErrorCode(rawValue: "internal")
}

/// Why a request was not carried out.
public struct IPCErrorBody: Codable, Sendable, Equatable {
    /// What kind of failure it was, for the client to match on.
    public let code: IPCErrorCode
    /// What it was, for a person to read. Written by the server, and never
    /// anything a client sent or an exchange carried.
    public let message: String

    public init(code: IPCErrorCode, message: String) {
        self.code = code
        self.message = message
    }
}

/// One answer to one request.
public struct IPCResponse: Sendable, Equatable {
    /// The request being answered, or `nil` when the server could not read
    /// one far enough to name it.
    public let requestID: String?
    /// What it produced, when it worked.
    public let result: IPCResult?
    /// Why it did not, when it did not.
    public let error: IPCErrorBody?

    public init(requestID: String?, result: IPCResult?, error: IPCErrorBody?) {
        self.requestID = requestID
        self.result = result
        self.error = error
    }
}

// MARK: - Events

/// Something that happened, sent without being asked.
public enum IPCEvent: Sendable, Equatable {
    /// The proxy finished an exchange.
    case trafficAdded(ExchangeSummary)
    /// This subscriber fell behind and exchanges were dropped for it.
    ///
    /// The list it holds is now missing rows, so the only correct answer is
    /// to ask for the list again.
    case trafficLost(missed: UInt64)
    /// A breakpoint is holding an exchange still.
    case breakpointPaused(PausedExchange)
    /// An exchange that was being held is not any more.
    ///
    /// Sent for every ending, including one this client asked for: the pause
    /// on screen has to stop being shown.
    case breakpointReleased(pauseID: String, reason: ReleaseReason)
    /// A WebSocket message or SSE event was captured.
    case streamMessageAdded(StreamMessageSummary)
    /// This subscriber fell behind and stream-message summaries were dropped.
    case streamMessagesLost(missed: UInt64)
}

// MARK: - Either one

/// One line from the server.
public enum IPCMessage: Sendable, Equatable {
    /// An answer to a request.
    case response(IPCResponse)
    /// Something that happened.
    case event(IPCEvent)
}

extension IPCMessage: Decodable {
    private enum CodingKeys: String, CodingKey {
        case version
        case requestID = "request_id"
        case ok
        case type
        case result
        case error
        case exchange
        case missed
        case paused
        case pauseID = "pause_id"
        case reason
        case message
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        let version = try container.decode(Int.self, forKey: .version)
        guard version == IPCProtocolVersion.current else {
            throw IPCError.unsupportedProtocolVersion(version)
        }

        // `ok` is the whole of the discrimination. Every response has it and
        // no event does, which is what the protocol guarantees; `type` means
        // an operation in one and an event name in the other, so reading it
        // first would be reading two different fields that share a name.
        guard let ok = try container.decodeIfPresent(Bool.self, forKey: .ok) else {
            self = .event(try IPCMessage.event(from: container))
            return
        }

        let requestID = try container.decodeIfPresent(String.self, forKey: .requestID)

        guard ok else {
            let error = try container.decode(IPCErrorBody.self, forKey: .error)
            self = .response(IPCResponse(requestID: requestID, result: nil, error: error))
            return
        }

        self = .response(
            IPCResponse(
                requestID: requestID,
                result: try IPCMessage.result(from: container),
                error: nil
            )
        )
    }

    /// Reads the result of a successful response, named by its `type`.
    private static func result(
        from container: KeyedDecodingContainer<CodingKeys>
    ) throws -> IPCResult {
        switch try container.decode(String.self, forKey: .type) {
        case "list_traffic":
            return .listTraffic(try container.decode(TrafficListResult.self, forKey: .result))
        case "get_traffic":
            return .getTraffic(try container.decode(TrafficDetail.self, forKey: .result).exchange)
        case "list_stream_messages":
            return .listStreamMessages(
                try container.decode(StreamMessageListResult.self, forKey: .result)
            )
        case "get_stream_message":
            return .getStreamMessage(
                try container.decode(StreamMessageDetail.self, forKey: .result).message
            )
        case "delete_traffic":
            return .deleteTraffic(try container.decode(DeletedResult.self, forKey: .result))
        case "clear_traffic":
            return .clearTraffic(try container.decode(ClearedResult.self, forKey: .result))
        case "subscribe":
            _ = try container.decode(Subscribed.self, forKey: .result)
            return .subscribe
        case "list_rules":
            return .listRules(try container.decode(RuleListResult.self, forKey: .result))
        case "get_rule":
            return .getRule(try container.decode(RuleDetail.self, forKey: .result).rule)
        case "create_rule":
            return .createRule(try container.decode(RuleDetail.self, forKey: .result).rule)
        case "update_rule":
            return .updateRule(try container.decode(RuleDetail.self, forKey: .result).rule)
        case "delete_rule":
            return .deleteRule(try container.decode(RuleDeletedResult.self, forKey: .result))
        case "parse_curl":
            return .parseCurl(try container.decode(CurlParseResult.self, forKey: .result))
        case "curl_from_traffic":
            return .curlFromTraffic(try container.decode(CurlGenerated.self, forKey: .result))
        case "replay_request":
            return .replayRequest(try container.decode(ReplayResult.self, forKey: .result))
        case "lan_status":
            return .lanStatus(try container.decode(LanStatusResult.self, forKey: .result))
        case "enable_lan":
            return .enableLan(try container.decode(LanStatusResult.self, forKey: .result))
        case "disable_lan":
            return .disableLan(try container.decode(LanStatusResult.self, forKey: .result))
        case "list_breakpoints":
            return .listBreakpoints(
                try container.decode(BreakpointListResult.self, forKey: .result)
            )
        case "create_breakpoint":
            return .createBreakpoint(
                try container.decode(BreakpointDetail.self, forKey: .result).breakpoint
            )
        case "update_breakpoint":
            return .updateBreakpoint(
                try container.decode(BreakpointDetail.self, forKey: .result).breakpoint
            )
        case "delete_breakpoint":
            return .deleteBreakpoint(
                try container.decode(BreakpointDeletedResult.self, forKey: .result)
            )
        case "watch_breakpoints":
            _ = try container.decode(Watching.self, forKey: .result)
            return .watchBreakpoints
        case "continue_breakpoint":
            return .continueBreakpoint(
                try container.decode(PauseResolvedResult.self, forKey: .result)
            )
        case "cancel_breakpoint":
            return .cancelBreakpoint(
                try container.decode(PauseResolvedResult.self, forKey: .result)
            )
        case "throttle_status":
            return .throttleStatus(try container.decode(ThrottleSettings.self, forKey: .result))
        case "set_throttle":
            return .setThrottle(try container.decode(ThrottleSettings.self, forKey: .result))
        case "list_saved_requests":
            return .listSavedRequests(
                try container.decode(SavedRequestListResult.self, forKey: .result)
            )
        case "get_saved_request":
            return .getSavedRequest(try savedRequest(from: container))
        case "create_saved_request":
            return .createSavedRequest(try savedRequest(from: container))
        case "save_request_from_traffic":
            return .saveRequestFromTraffic(try savedRequest(from: container))
        case "update_saved_request":
            return .updateSavedRequest(try savedRequest(from: container))
        case "delete_saved_request":
            return .deleteSavedRequest(
                try container.decode(SavedRequestDeletedResult.self, forKey: .result)
            )
        case "export_har":
            return .exportHar(try container.decode(HarExportResult.self, forKey: .result))
        case "import_har":
            return .importHar(try container.decode(HarImportResult.self, forKey: .result))
        case "diagnostics":
            return .diagnostics(try container.decode(CoreDiagnostics.self, forKey: .result))
        default:
            // A result this build cannot read is a disagreement about the
            // schema, and the version said there was none.
            throw IPCError.malformedMessage
        }
    }

    /// Reads an event, named by its `type`.
    private static func event(from container: KeyedDecodingContainer<CodingKeys>) throws -> IPCEvent {
        switch try container.decode(String.self, forKey: .type) {
        case "traffic_added":
            return .trafficAdded(try container.decode(ExchangeSummary.self, forKey: .exchange))
        case "traffic_lost":
            return .trafficLost(missed: try container.decode(UInt64.self, forKey: .missed))
        case "breakpoint_paused":
            return .breakpointPaused(try container.decode(PausedExchange.self, forKey: .paused))
        case "breakpoint_released":
            return .breakpointReleased(
                pauseID: try container.decode(String.self, forKey: .pauseID),
                reason: try container.decode(ReleaseReason.self, forKey: .reason)
            )
        case "stream_message_added":
            return .streamMessageAdded(
                try container.decode(StreamMessageSummary.self, forKey: .message)
            )
        case "stream_messages_lost":
            return .streamMessagesLost(missed: try container.decode(UInt64.self, forKey: .missed))
        default:
            throw IPCError.malformedMessage
        }
    }

    /// The wrapper a whole exchange arrives in.
    private struct TrafficDetail: Decodable {
        let exchange: TrafficExchange
    }

    /// The wrapper one rule arrives in.
    private struct RuleDetail: Decodable {
        let rule: Rule
    }

    /// The wrapper a subscription confirmation arrives in.
    private struct Subscribed: Decodable {
        let subscribed: Bool
    }

    /// The wrapper one saved request arrives in.
    private struct SavedRequestDetail: Decodable {
        let savedRequest: SavedRequest

        private enum CodingKeys: String, CodingKey {
            case savedRequest = "saved_request"
        }
    }

    /// The saved request in a response that carries one.
    private static func savedRequest(
        from container: KeyedDecodingContainer<CodingKeys>
    ) throws -> SavedRequest {
        try container.decode(SavedRequestDetail.self, forKey: .result).savedRequest
    }

    /// The wrapper one breakpoint arrives in.
    private struct BreakpointDetail: Decodable {
        let breakpoint: Breakpoint
    }

    /// The wrapper a breakpoint-watch confirmation arrives in.
    private struct Watching: Decodable {
        let watching: Bool
    }
}
