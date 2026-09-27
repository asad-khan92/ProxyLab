//  The rule model, as `core/rules` spells it on the wire.
//
//      {"id":"…","enabled":true,"name":"block analytics","priority":10,
//       "matcher":{"host":{"exact":"analytics.example.com"}},
//       "action":{"type":"block","status":403},
//       "created_at_ms":1700000000000,"updated_at_ms":1700000000000}
//
//  Mirrored field for field, because the core sends the rule as its own type
//  and refuses a field it does not know. Nothing here decides anything: a rule
//  is matched, validated, stored, and carried out in Rust, and this is the
//  shape that travels between the two.
//
//  # Why the destination arrives in parts
//
//  A map-remote rule carries a `RemoteTarget` — scheme, host, port, path,
//  query — and never a URL string, because the core parses a destination once,
//  when the rule is written, and keeps the parts. So a URL typed into the
//  editor is split here, by ``RemoteTarget/parse(_:)``, which makes exactly the
//  refusals `RemoteTarget::parse` makes. The core validates the parts again
//  when it receives them: this is what lets the editor say "that is not a
//  destination" while the user is typing, and it is not what decides.

import Foundation

/// The longest name a rule may carry, as the core enforces it.
public let maxRuleNameLength = 200

// MARK: - Matching

/// The host a rule is for.
public enum HostMatch: Codable, Sendable, Equatable {
    /// This host exactly, compared without regard to case.
    case exact(String)

    /// The host as written.
    public var text: String {
        switch self {
        case let .exact(host): host
        }
    }

    private enum CodingKeys: String, CodingKey {
        case exact
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        guard let host = try container.decodeIfPresent(String.self, forKey: .exact) else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a host matcher is an exact host"
                )
            )
        }

        self = .exact(host)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .exact(host): try container.encode(host, forKey: .exact)
        }
    }
}

/// The path a rule is for.
public enum PathMatch: Codable, Sendable, Equatable {
    /// This path exactly, compared with regard to case.
    case exact(String)
    /// Every path that starts with this one, as a string and not as a path
    /// segment: `/assets` matches `/assets/logo.png` and `/assetsX` alike.
    case prefix(String)

    /// The path as written.
    public var text: String {
        switch self {
        case let .exact(path), let .prefix(path): path
        }
    }

    /// Whether this is a prefix rather than an exact path.
    public var isPrefix: Bool {
        if case .prefix = self { return true }

        return false
    }

    private enum CodingKeys: String, CodingKey {
        case exact
        case prefix
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        if let path = try container.decodeIfPresent(String.self, forKey: .exact) {
            self = .exact(path)
        } else if let path = try container.decodeIfPresent(String.self, forKey: .prefix) {
            self = .prefix(path)
        } else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a path matcher is an exact path or a prefix"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .exact(path): try container.encode(path, forKey: .exact)
        case let .prefix(path): try container.encode(path, forKey: .prefix)
        }
    }
}

/// The method a rule is for.
public enum MethodMatch: Codable, Sendable, Equatable {
    /// This method exactly, compared without regard to case.
    case exact(String)

    /// The method as written.
    public var text: String {
        switch self {
        case let .exact(method): method
        }
    }

    private enum CodingKeys: String, CodingKey {
        case exact
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        guard let method = try container.decodeIfPresent(String.self, forKey: .exact) else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a method matcher is an exact method"
                )
            )
        }

        self = .exact(method)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .exact(method): try container.encode(method, forKey: .exact)
        }
    }
}

/// Which requests a rule applies to.
///
/// Three optional criteria combined with `and`. **One that is not set matches
/// anything**, so a matcher naming nothing matches every request — which is
/// how "block all of it except these" is written.
public struct RuleMatcher: Codable, Sendable, Equatable {
    /// The host the request is for, or every host.
    public var host: HostMatch?
    /// The path the request asks for, or every path.
    public var path: PathMatch?
    /// The method the request used, or every method.
    public var method: MethodMatch?

    public init(host: HostMatch? = nil, path: PathMatch? = nil, method: MethodMatch? = nil) {
        self.host = host
        self.path = path
        self.method = method
    }

    /// A matcher that matches every request.
    public static let any = RuleMatcher()

    /// Whether this matcher narrows nothing.
    public var isAny: Bool { host == nil && path == nil && method == nil }

    private enum CodingKeys: String, CodingKey {
        case host
        case path
        case method
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        host = try container.decodeIfPresent(HostMatch.self, forKey: .host)
        path = try container.decodeIfPresent(PathMatch.self, forKey: .path)
        method = try container.decodeIfPresent(MethodMatch.self, forKey: .method)
    }

    /// A criterion that is not set is not sent: the core refuses a key it does
    /// not know, and a `null` is still a key it would have to read.
    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encodeIfPresent(host, forKey: .host)
        try container.encodeIfPresent(path, forKey: .path)
        try container.encodeIfPresent(method, forKey: .method)
    }
}

// MARK: - Actions

/// Answering a request here rather than carrying it.
public struct BlockAction: Codable, Sendable, Equatable {
    /// The status the client is answered with.
    ///
    /// The body is one fixed sentence the core writes, and is not the rule's
    /// to choose.
    public var status: UInt16

    public init(status: UInt16 = BlockAction.defaultStatus) {
        self.status = status
    }

    /// What a block answers with unless the rule names another status.
    public static let defaultStatus: UInt16 = 403

    private enum CodingKeys: String, CodingKey {
        case status
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        status = try container.decodeIfPresent(UInt16.self, forKey: .status) ?? Self.defaultStatus
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(status, forKey: .status)
    }
}

/// Answering a request with a file on this machine.
///
/// The path is the rule's, and only the rule's: nothing from the request is
/// joined to it. The app never opens the file — the core reads it when the
/// rule runs.
public struct MapLocalAction: Codable, Sendable, Equatable {
    /// The file to answer with. Absolute; the core refuses anything else.
    public var path: String
    /// The content type to answer with, or `nil` to let the core work it out
    /// from the file's extension.
    public var contentType: String?
    /// The status to answer with.
    public var status: UInt16

    public init(path: String, contentType: String? = nil, status: UInt16 = 200) {
        self.path = path
        self.contentType = contentType
        self.status = status
    }

    private enum CodingKeys: String, CodingKey {
        case path
        case contentType = "content_type"
        case status
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        path = try container.decode(String.self, forKey: .path)
        contentType = try container.decodeIfPresent(String.self, forKey: .contentType)
        status = try container.decodeIfPresent(UInt16.self, forKey: .status) ?? 200
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(path, forKey: .path)
        try container.encodeIfPresent(contentType, forKey: .contentType)
        try container.encode(status, forKey: .status)
    }
}

/// The schemes a request may be mapped to.
public enum RemoteScheme: String, Codable, Sendable, Equatable, CaseIterable {
    /// Plaintext HTTP.
    case http
    /// HTTP inside a TLS session the proxy opens and verifies.
    case https

    /// The port this scheme implies.
    public var defaultPort: UInt16 {
        switch self {
        case .http: 80
        case .https: 443
        }
    }
}

/// A destination a request can be mapped to, in the parts the core keeps.
public struct RemoteTarget: Codable, Sendable, Equatable {
    /// `http` or `https`.
    public var scheme: RemoteScheme
    /// The host, without brackets around an IPv6 literal.
    public var host: String
    /// The port, or `nil` for the scheme's own.
    public var port: UInt16?
    /// The path the rule names, or `nil` to keep the request's.
    public var path: String?
    /// The query the rule names, or `nil` to keep the request's.
    public var query: String?

    public init(
        scheme: RemoteScheme,
        host: String,
        port: UInt16? = nil,
        path: String? = nil,
        query: String? = nil
    ) {
        self.scheme = scheme
        self.host = host
        self.port = port
        self.path = path
        self.query = query
    }

    private enum CodingKeys: String, CodingKey {
        case scheme
        case host
        case port
        case path
        case query
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        scheme = try container.decode(RemoteScheme.self, forKey: .scheme)
        host = try container.decode(String.self, forKey: .host)
        port = try container.decodeIfPresent(UInt16.self, forKey: .port)
        path = try container.decodeIfPresent(String.self, forKey: .path)
        query = try container.decodeIfPresent(String.self, forKey: .query)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(scheme, forKey: .scheme)
        try container.encode(host, forKey: .host)
        try container.encodeIfPresent(port, forKey: .port)
        try container.encodeIfPresent(path, forKey: .path)
        try container.encodeIfPresent(query, forKey: .query)
    }

    /// The authority a `Host` header would carry: the port only when it is
    /// not the scheme's own.
    public var authority: String {
        let host = self.host.contains(":") ? "[\(self.host)]" : self.host

        if let port, port != scheme.defaultPort { return "\(host):\(port)" }

        return host
    }

    /// The destination written back out as a URL.
    ///
    /// What the editor shows for a rule that already exists, and what the
    /// rule list shows as its destination.
    public var url: String {
        var url = "\(scheme.rawValue)://\(authority)"
        if let path { url += path }
        if let query { url += "?\(query)" }

        return url
    }
}

// MARK: - Transformations

/// What happens to one header, as `rules::HeaderEdit` spells it.
public struct HeaderEdit: Codable, Sendable, Equatable {
    /// Set replaces every value the header had; add appends one; remove takes
    /// every value away.
    public enum Operation: String, Codable, Sendable, Equatable, CaseIterable, Identifiable {
        case set
        case add
        case remove

        public var id: String { rawValue }

        public var title: String {
            switch self {
            case .set: "Set"
            case .add: "Add"
            case .remove: "Remove"
            }
        }
    }

    public var operation: Operation
    public var name: String
    /// The value, for set and add. Not sent for remove.
    public var value: String

    public init(_ operation: Operation, name: String, value: String = "") {
        self.operation = operation
        self.name = name
        self.value = value
    }

    private enum CodingKeys: String, CodingKey {
        case operation = "op"
        case name
        case value
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        operation = try container.decode(Operation.self, forKey: .operation)
        name = try container.decode(String.self, forKey: .name)
        value = try container.decodeIfPresent(String.self, forKey: .value) ?? ""
    }

    /// A remove carries no value: the core refuses a field it does not know.
    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(operation, forKey: .operation)
        try container.encode(name, forKey: .name)
        if operation != .remove { try container.encode(value, forKey: .value) }
    }
}

/// What happens to one query parameter, as `rules::QueryEdit` spells it.
///
/// Names and values are written exactly as they appear in a URL: nothing is
/// percent-encoded for you.
public struct QueryEdit: Codable, Sendable, Equatable {
    public enum Operation: String, Codable, Sendable, Equatable, CaseIterable, Identifiable {
        case set
        case remove

        public var id: String { rawValue }

        public var title: String {
            switch self {
            case .set: "Set"
            case .remove: "Remove"
            }
        }
    }

    public var operation: Operation
    public var name: String
    public var value: String

    public init(_ operation: Operation, name: String, value: String = "") {
        self.operation = operation
        self.name = name
        self.value = value
    }

    private enum CodingKeys: String, CodingKey {
        case operation = "op"
        case name
        case value
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        operation = try container.decode(Operation.self, forKey: .operation)
        name = try container.decode(String.self, forKey: .name)
        value = try container.decodeIfPresent(String.self, forKey: .value) ?? ""
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(operation, forKey: .operation)
        try container.encode(name, forKey: .name)
        if operation != .remove { try container.encode(value, forKey: .value) }
    }
}

/// What happens to a request path, as `rules::PathEdit` spells it.
public enum PathEdit: Codable, Sendable, Equatable {
    /// The path becomes exactly this.
    case set(String)
    /// A path starting with `from` has that part replaced by `to` — a string
    /// prefix, like the matcher's.
    case replacePrefix(from: String, to: String)

    private enum CodingKeys: String, CodingKey {
        case operation = "op"
        case path
        case from
        case to
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        switch try container.decode(String.self, forKey: .operation) {
        case "set":
            self = .set(try container.decode(String.self, forKey: .path))
        case "replace_prefix":
            self = .replacePrefix(
                from: try container.decode(String.self, forKey: .from),
                to: try container.decode(String.self, forKey: .to)
            )
        default:
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a path edit this build does not know"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .set(path):
            try container.encode("set", forKey: .operation)
            try container.encode(path, forKey: .path)
        case let .replacePrefix(from, to):
            try container.encode("replace_prefix", forKey: .operation)
            try container.encode(from, forKey: .from)
            try container.encode(to, forKey: .to)
        }
    }
}

/// What happens to a body, as `rules::BodyEdit` spells it.
public enum BodyEdit: Codable, Sendable, Equatable {
    /// The body becomes exactly this text.
    case replace(String)
    /// One field of a JSON object body is set. `path` is keys joined by dots;
    /// `value` is JSON text — `"staging"`, `42`, `true`, `{"a":1}`.
    case setJSON(path: String, value: String)

    private enum CodingKeys: String, CodingKey {
        case operation = "op"
        case text
        case path
        case value
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        switch try container.decode(String.self, forKey: .operation) {
        case "replace":
            self = .replace(try container.decode(String.self, forKey: .text))
        case "set_json":
            self = .setJSON(
                path: try container.decode(String.self, forKey: .path),
                value: try container.decode(String.self, forKey: .value)
            )
        default:
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "a body edit this build does not know"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case let .replace(text):
            try container.encode("replace", forKey: .operation)
            try container.encode(text, forKey: .text)
        case let .setJSON(path, value):
            try container.encode("set_json", forKey: .operation)
            try container.encode(path, forKey: .path)
            try container.encode(value, forKey: .value)
        }
    }
}

/// What a Modify Request rule changes. Every part is optional; the core
/// refuses one that changes nothing.
public struct ModifyRequestAction: Codable, Sendable, Equatable {
    public var method: String?
    public var path: PathEdit?
    public var query: [QueryEdit]
    public var headers: [HeaderEdit]
    public var body: BodyEdit?

    public init(
        method: String? = nil,
        path: PathEdit? = nil,
        query: [QueryEdit] = [],
        headers: [HeaderEdit] = [],
        body: BodyEdit? = nil
    ) {
        self.method = method
        self.path = path
        self.query = query
        self.headers = headers
        self.body = body
    }

    private enum CodingKeys: String, CodingKey {
        case method
        case path
        case query
        case headers
        case body
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        method = try container.decodeIfPresent(String.self, forKey: .method)
        path = try container.decodeIfPresent(PathEdit.self, forKey: .path)
        query = try container.decodeIfPresent([QueryEdit].self, forKey: .query) ?? []
        headers = try container.decodeIfPresent([HeaderEdit].self, forKey: .headers) ?? []
        body = try container.decodeIfPresent(BodyEdit.self, forKey: .body)
    }

    /// What is not changed is not sent, which is how the core writes it too.
    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encodeIfPresent(method, forKey: .method)
        try container.encodeIfPresent(path, forKey: .path)
        if !query.isEmpty { try container.encode(query, forKey: .query) }
        if !headers.isEmpty { try container.encode(headers, forKey: .headers) }
        try container.encodeIfPresent(body, forKey: .body)
    }
}

/// What a Modify Response rule changes.
public struct ModifyResponseAction: Codable, Sendable, Equatable {
    public var status: UInt16?
    public var headers: [HeaderEdit]
    public var body: BodyEdit?

    public init(status: UInt16? = nil, headers: [HeaderEdit] = [], body: BodyEdit? = nil) {
        self.status = status
        self.headers = headers
        self.body = body
    }

    private enum CodingKeys: String, CodingKey {
        case status
        case headers
        case body
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        status = try container.decodeIfPresent(UInt16.self, forKey: .status)
        headers = try container.decodeIfPresent([HeaderEdit].self, forKey: .headers) ?? []
        body = try container.decodeIfPresent(BodyEdit.self, forKey: .body)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encodeIfPresent(status, forKey: .status)
        if !headers.isEmpty { try container.encode(headers, forKey: .headers) }
        try container.encodeIfPresent(body, forKey: .body)
    }
}

/// Answering a request with a redirect.
public struct RedirectAction: Codable, Sendable, Equatable {
    /// Where the client is sent, exactly as written.
    public var target: RemoteTarget
    /// One of ``statuses``.
    public var status: UInt16

    public init(target: RemoteTarget, status: UInt16 = RedirectAction.defaultStatus) {
        self.target = target
        self.status = status
    }

    /// The four statuses the core accepts, as `rules::REDIRECT_STATUSES`.
    public static let statuses: [UInt16] = [301, 302, 307, 308]

    /// What a redirect answers with unless the rule names another.
    public static let defaultStatus: UInt16 = 302

    private enum CodingKeys: String, CodingKey {
        case target
        case status
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        target = try container.decode(RemoteTarget.self, forKey: .target)
        status = try container.decodeIfPresent(UInt16.self, forKey: .status) ?? Self.defaultStatus
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        try container.encode(target, forKey: .target)
        try container.encode(status, forKey: .status)
    }
}

/// What a rule does with a request it matches.
public enum RuleAction: Codable, Sendable, Equatable {
    /// Carry the request as the proxy would have anyway.
    ///
    /// Not a no-op: an allow rule at a lower priority number is how one
    /// request is excepted from a block that would otherwise cover it.
    case allow
    /// Answer the request here, without contacting the origin.
    case block(BlockAction)
    /// Answer the request with the contents of a file on this machine.
    case mapLocal(MapLocalAction)
    /// Send the request somewhere else.
    case mapRemote(RemoteTarget)
    /// Answer the request with a redirect.
    case redirect(RedirectAction)
    /// Change the request before it is sent. Decides nothing: the rules
    /// below it are still consulted.
    case modifyRequest(ModifyRequestAction)
    /// Change the origin's response before the client is given it.
    case modifyResponse(ModifyResponseAction)

    /// What this action is called on the wire.
    public var name: String {
        switch self {
        case .allow: "allow"
        case .block: "block"
        case .mapLocal: "map_local"
        case .mapRemote: "map_remote"
        case .redirect: "redirect"
        case .modifyRequest: "modify_request"
        case .modifyResponse: "modify_response"
        }
    }

    /// Which kind of action this is, without its configuration.
    public var kind: RuleActionKind {
        switch self {
        case .allow: .allow
        case .block: .block
        case .mapLocal: .mapLocal
        case .mapRemote: .mapRemote
        case .redirect: .redirect
        case .modifyRequest: .modifyRequest
        case .modifyResponse: .modifyResponse
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case target
    }

    /// Internally tagged, as the core writes it: the action's own fields sit
    /// beside `type` rather than under it.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        switch try container.decode(String.self, forKey: .type) {
        case "allow":
            self = .allow
        case "block":
            self = .block(try BlockAction(from: decoder))
        case "map_local":
            self = .mapLocal(try MapLocalAction(from: decoder))
        case "map_remote":
            self = .mapRemote(try container.decode(RemoteTarget.self, forKey: .target))
        case "redirect":
            self = .redirect(try RedirectAction(from: decoder))
        case "modify_request":
            self = .modifyRequest(try ModifyRequestAction(from: decoder))
        case "modify_response":
            self = .modifyResponse(try ModifyResponseAction(from: decoder))
        default:
            throw DecodingError.dataCorrupted(
                DecodingError.Context(
                    codingPath: container.codingPath,
                    debugDescription: "an action this build cannot carry out"
                )
            )
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(name, forKey: .type)

        switch self {
        case .allow:
            break
        case let .block(block):
            try block.encode(to: encoder)
        case let .mapLocal(map):
            try map.encode(to: encoder)
        case let .mapRemote(target):
            try container.encode(target, forKey: .target)
        case let .redirect(redirect):
            try redirect.encode(to: encoder)
        case let .modifyRequest(modify):
            try modify.encode(to: encoder)
        case let .modifyResponse(modify):
            try modify.encode(to: encoder)
        }
    }
}

/// Which kind of action a rule has, as a choice a person makes.
public enum RuleActionKind: String, Codable, Sendable, Equatable, CaseIterable, Identifiable {
    case block
    case mapLocal
    case mapRemote
    case redirect
    case modifyRequest
    case modifyResponse
    case allow

    public var id: String { rawValue }

    /// What the action is called on screen.
    public var title: String {
        switch self {
        case .allow: "Allow"
        case .block: "Block"
        case .mapLocal: "Map Local"
        case .mapRemote: "Map Remote"
        case .redirect: "Redirect"
        case .modifyRequest: "Modify Request"
        case .modifyResponse: "Modify Response"
        }
    }

    /// Whether this kind changes a message rather than deciding what happens
    /// to it, so the rules below it are still consulted.
    public var isTransformation: Bool {
        self == .modifyRequest || self == .modifyResponse
    }

    /// One sentence saying what the core does with a matching request.
    public var explanation: String {
        switch self {
        case .allow:
            "Carries the request as the proxy would anyway. An allow rule above a "
                + "block is how one request is excepted from it."
        case .block:
            "Answers the request here, without contacting the origin."
        case .mapLocal:
            "Answers the request with the contents of a file on this Mac."
        case .mapRemote:
            "Sends the request to another destination."
        case .redirect:
            "Answers the request with a redirect to another URL, without contacting the origin."
        case .modifyRequest:
            "Changes the request before it is sent. The rules below this one are still "
                + "consulted; the first that decides what happens to the request still does."
        case .modifyResponse:
            "Changes the origin's response before the client is given it. The rules below "
                + "this one are still consulted."
        }
    }

    /// The symbol the list and the editor mark this action with.
    public var symbol: String {
        switch self {
        case .allow: "checkmark.shield"
        case .block: "hand.raised.fill"
        case .mapLocal: "doc.text.fill"
        case .mapRemote: "arrow.triangle.branch"
        case .redirect: "arrow.uturn.right"
        case .modifyRequest: "square.and.pencil"
        case .modifyResponse: "pencil.and.list.clipboard"
        }
    }
}

// MARK: - Rules

/// One rule, as the core holds it.
public struct Rule: Codable, Sendable, Equatable, Identifiable {
    /// Which rule this is: a UUID the core minted.
    public let id: String
    /// Whether it is consulted at all.
    public var enabled: Bool
    /// What a person calls it.
    public var name: String
    /// Which requests it is for.
    public var matcher: RuleMatcher
    /// What it does with them.
    public var action: RuleAction
    /// Where it sits in the order rules are consulted in: lower first.
    public var priority: Int32
    /// When it was created, in milliseconds since the Unix epoch.
    public let createdAtMs: UInt64
    /// When it was last changed, in milliseconds since the Unix epoch.
    public let updatedAtMs: UInt64

    public init(
        id: String,
        enabled: Bool,
        name: String,
        matcher: RuleMatcher,
        action: RuleAction,
        priority: Int32,
        createdAtMs: UInt64,
        updatedAtMs: UInt64
    ) {
        self.id = id
        self.enabled = enabled
        self.name = name
        self.matcher = matcher
        self.action = action
        self.priority = priority
        self.createdAtMs = createdAtMs
        self.updatedAtMs = updatedAtMs
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case enabled
        case name
        case matcher
        case action
        case priority
        case createdAtMs = "created_at_ms"
        case updatedAtMs = "updated_at_ms"
    }
}

/// A rule that has not been created yet.
///
/// Everything but the id and the timestamps, which are the core's to mint.
public struct NewRule: Encodable, Sendable, Equatable {
    /// Whether the rule is consulted once it exists.
    public var enabled: Bool
    /// What a person calls it.
    public var name: String
    /// Which requests it is for.
    public var matcher: RuleMatcher
    /// What it does with them.
    public var action: RuleAction
    /// Where it sits in the order. Lower numbers are consulted first.
    public var priority: Int32

    public init(
        enabled: Bool = true,
        name: String,
        matcher: RuleMatcher = .any,
        action: RuleAction,
        priority: Int32 = 0
    ) {
        self.enabled = enabled
        self.name = name
        self.matcher = matcher
        self.action = action
        self.priority = priority
    }
}

/// A change to a rule that already exists.
///
/// Every field is optional, and one that is not named is not changed. That is
/// what makes "turn this rule off" one field rather than the whole rule read
/// back, changed, and sent again — which would race with anything else
/// editing it.
public struct RuleUpdate: Encodable, Sendable, Equatable {
    /// Whether the rule is consulted.
    public var enabled: Bool?
    /// What a person calls it.
    public var name: String?
    /// Which requests it is for.
    public var matcher: RuleMatcher?
    /// What it does with them.
    public var action: RuleAction?
    /// Where it sits in the order.
    public var priority: Int32?

    public init(
        enabled: Bool? = nil,
        name: String? = nil,
        matcher: RuleMatcher? = nil,
        action: RuleAction? = nil,
        priority: Int32? = nil
    ) {
        self.enabled = enabled
        self.name = name
        self.matcher = matcher
        self.action = action
        self.priority = priority
    }

    /// Whether this update changes anything at all.
    ///
    /// The core refuses one that does not, so nothing sends one.
    public var isEmpty: Bool {
        enabled == nil && name == nil && matcher == nil && action == nil && priority == nil
    }
}

/// The result of removing one rule.
public struct RuleDeletedResult: Codable, Sendable, Equatable {
    /// Which rule.
    public let ruleID: String
    /// Whether there was one to remove.
    public let deleted: Bool

    public init(ruleID: String, deleted: Bool) {
        self.ruleID = ruleID
        self.deleted = deleted
    }

    private enum CodingKeys: String, CodingKey {
        case ruleID = "rule_id"
        case deleted
    }
}

/// Every rule there is, in the order they are consulted in.
public struct RuleListResult: Codable, Sendable, Equatable {
    /// The rules: priority ascending, then age.
    public let rules: [Rule]

    public init(rules: [Rule]) {
        self.rules = rules
    }
}
