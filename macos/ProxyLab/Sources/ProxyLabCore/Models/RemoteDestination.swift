//  A map-remote destination, read out of the URL a person typed.
//
//      https://staging.example.com:8443/v2?debug=1
//        │            │           │    │     │
//      scheme        host       port  path  query
//
//  The core carries a destination in parts and never as a string, because it
//  parses one once — when the rule is written — and builds every outgoing URL
//  from the parts rather than by pasting two strings together. So the editor
//  has to do the splitting before it can send the rule, and this is that split.
//
//  It makes the same refusals `RemoteTarget::parse` makes, and for the same
//  reasons: a scheme this proxy cannot speak, credentials in the authority, or
//  a fragment that would never be sent to a server are all worth refusing while
//  someone is still typing. **It is not what decides.** The core validates the
//  parts again when it receives them and refuses the rule if they are not a
//  destination; this only means the refusal can be shown under the field.

import Foundation

/// Why a typed destination is not one a request could be mapped to.
///
/// Each case carries the core's own phrase, so the editor and the core say the
/// same thing about the same mistake.
public enum RemoteDestinationError: Error, Sendable, Equatable {
    /// Nothing was typed.
    case empty
    /// No `scheme://` at all.
    case noScheme
    /// A scheme this proxy cannot speak.
    case unsupportedScheme
    /// Characters a URL cannot carry.
    case invalidURL
    /// A `#fragment`, which is never sent to a server.
    case fragment
    /// `user:password@`, which is credentials sitting in a rule.
    case userinfo
    /// No host.
    case hostEmpty
    /// A host a connection could not be opened to.
    case hostInvalid
    /// A port that is not a port.
    case portInvalid
    /// A path that does not start with a slash.
    case pathInvalid
    /// A query with whitespace in it.
    case queryInvalid

    /// What a person is told, in the core's own words.
    public var message: String {
        switch self {
        case .empty: "A Map Remote rule must name a destination."
        case .noScheme: "A destination must name http or https."
        case .unsupportedScheme: "A destination must be http or https."
        case .invalidURL: "That is not a URL."
        case .fragment: "A destination must not carry a #fragment."
        case .userinfo: "A destination must not carry credentials."
        case .hostEmpty: "A destination must name a host."
        case .hostInvalid: "That host is not a host."
        case .portInvalid: "That port is not a port."
        case .pathInvalid: "A path must start with a slash."
        case .queryInvalid: "A query must not contain spaces."
        }
    }
}

extension RemoteTarget {
    /// Reads a destination out of `url`, or says why it is not one.
    ///
    /// What is accepted is `scheme://host[:port][/path][?query]`, with no
    /// userinfo and no fragment — deliberately strict, because a mapping is a
    /// destination a developer typed and the things a lenient parser would
    /// quietly accept are the ones worth refusing now rather than at the
    /// moment a request would have been sent.
    public static func parse(_ url: String) throws -> RemoteTarget {
        let url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !url.isEmpty else { throw RemoteDestinationError.empty }
        guard !url.utf8.contains(where: { $0 <= 0x20 || $0 == 0x7F }) else {
            throw RemoteDestinationError.invalidURL
        }

        guard let separator = url.range(of: "://") else { throw RemoteDestinationError.noScheme }

        let scheme: RemoteScheme
        switch url[url.startIndex..<separator.lowerBound].lowercased() {
        case "http": scheme = .http
        case "https": scheme = .https
        default: throw RemoteDestinationError.unsupportedScheme
        }

        let rest = String(url[separator.upperBound...])
        // A fragment is never sent to a server, so a mapping carrying one is a
        // mapping whose author expected something that cannot happen.
        guard !rest.contains("#") else { throw RemoteDestinationError.fragment }

        let split = rest.firstIndex { $0 == "/" || $0 == "?" }
        let authority = String(split.map { rest[rest.startIndex..<$0] } ?? Substring(rest))
        let pathAndQuery = String(split.map { rest[$0...] } ?? "")

        // Userinfo would be credentials sitting in a rule and then sent to a
        // server, and it is never what a mapping means.
        guard !authority.contains("@") else { throw RemoteDestinationError.userinfo }

        let (host, port) = try splitAuthority(authority)
        let (path, query) = splitPathAndQuery(pathAndQuery)

        let target = RemoteTarget(scheme: scheme, host: host, port: port, path: path, query: query)
        try target.checked()

        return target
    }

    /// Whether this is a destination a connection could be opened to.
    ///
    /// The same checks the core makes on the parts it is sent, so a
    /// destination assembled here rather than typed is held to the same rule.
    public func checked() throws {
        guard !host.isEmpty else { throw RemoteDestinationError.hostEmpty }
        guard
            !host.utf8.contains(where: { byte in
                byte <= 0x20 || byte == 0x7F || "/?#@\\".utf8.contains(byte)
            })
        else {
            throw RemoteDestinationError.hostInvalid
        }
        guard port != 0 else { throw RemoteDestinationError.portInvalid }
        if let path {
            guard path.hasPrefix("/"), !path.contains(where: \.isWhitespace) else {
                throw RemoteDestinationError.pathInvalid
            }
        }
        if let query, query.contains(where: \.isWhitespace) {
            throw RemoteDestinationError.queryInvalid
        }
    }

    /// Splits `host:port`, keeping an IPv6 literal in one piece.
    private static func splitAuthority(_ authority: String) throws -> (String, UInt16?) {
        guard !authority.isEmpty else { throw RemoteDestinationError.hostEmpty }

        // `[::1]:8443` — the brackets are the URL's way of saying the colons in
        // the middle belong to the address and not to a port.
        if authority.hasPrefix("[") {
            let rest = authority.dropFirst()
            guard let close = rest.firstIndex(of: "]") else {
                throw RemoteDestinationError.hostInvalid
            }

            return (String(rest[rest.startIndex..<close]), try port(after: String(rest[rest.index(after: close)...])))
        }

        guard let colon = authority.lastIndex(of: ":") else { return (authority, nil) }

        return (
            String(authority[authority.startIndex..<colon]),
            try port(after: String(authority[colon...]))
        )
    }

    /// Reads the `:port` that may follow an authority.
    private static func port(after rest: String) throws -> UInt16? {
        guard !rest.isEmpty else { return nil }
        guard rest.hasPrefix(":") else { throw RemoteDestinationError.invalidURL }
        guard let port = UInt16(rest.dropFirst()) else {
            throw RemoteDestinationError.portInvalid
        }

        return port
    }

    /// Splits the `/path?query` a mapping may carry.
    ///
    /// A mapping of `https://host` and one of `https://host/` mean the same
    /// thing — keep the request's path — because a bare slash is what a URL
    /// writes when it names no path at all.
    private static func splitPathAndQuery(_ rest: String) -> (String?, String?) {
        guard !rest.isEmpty else { return (nil, nil) }

        let path: String
        let query: String?
        if let mark = rest.firstIndex(of: "?") {
            path = String(rest[rest.startIndex..<mark])
            query = String(rest[rest.index(after: mark)...])
        } else {
            path = rest
            query = nil
        }

        return (path.isEmpty || path == "/" ? nil : path, query)
    }
}
