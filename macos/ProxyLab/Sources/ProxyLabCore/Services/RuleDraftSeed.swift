//  A new rule, filled in from the request it was started from.
//
//      selected exchange ──▶ seed for one action ──▶ the rule editor
//
//  The editor's fields stay the editor's. This is only the first draft: host,
//  path and method where a rule matches, and the action fields that kind of
//  rule actually has. A Map Local rule is not handed the response body, and a
//  Modify rule is not handed every header — `Authorization` and `Cookie` stay
//  out, and so do the headers the proxy owns. The person edits the draft
//  before it is saved. Nothing here is stored.

import Foundation

/// What the rule editor is opened with when a rule is started from traffic.
public struct RuleDraftSeed: Sendable {
    /// A name the person can change.
    public var name: String
    /// The host to match, or empty for every host.
    public var host: String
    /// The path to match, or empty for every path.
    public var path: String
    /// The method to match, or empty for every method.
    public var method: String
    /// The status a Map Local rule answers with, when the response had one.
    public var localStatus: String?
    /// The content type a Map Local rule answers with, when the response named one.
    public var localContentType: String?
    /// Where a Map Remote rule starts, as a URL the editor can split.
    public var destination: String?
    /// Where a Redirect rule starts, as a URL the editor can split.
    public var redirectURL: String?
    /// What a Modify Request rule starts from. Empty unless that kind was asked for.
    public var modifyRequest: ModifyRequestDraft
    /// What a Modify Response rule starts from. Empty unless that kind was asked for.
    public var modifyResponse: ModifyResponseDraft

    /// Method, host and path on one line, for the chooser that opened the editor.
    public var matchLine: String {
        [method, host, path].filter { !$0.isEmpty }.joined(separator: " ")
    }

    /// A draft from a traffic row. Headers and bodies are not on a summary,
    /// so a Modify rule started from a row alone matches the request and does
    /// not yet change it. The chooser fetches the exchange before the editor
    /// opens, and uses ``from(exchange:kind:)`` when that succeeds.
    public static func from(summary: ExchangeSummary, kind: RuleActionKind) -> RuleDraftSeed {
        make(
            kind: kind,
            scheme: summary.scheme,
            url: summary.url,
            method: summary.method,
            destination: summary.destination,
            status: summary.status,
            responseContentType: summary.responseContentType,
            requestHeaders: Headers(),
            responseHeaders: Headers(),
            requestBody: nil,
            responseBody: nil
        )
    }

    /// A draft from the whole exchange. Only the fields that kind of rule
    /// uses are filled; the rest stay empty.
    public static func from(exchange: TrafficExchange, kind: RuleActionKind) -> RuleDraftSeed {
        let inspection = BodyInspection.of(exchange)

        return make(
            kind: kind,
            scheme: exchange.connection.scheme,
            url: exchange.request.url,
            method: exchange.request.method,
            destination: exchange.connection.destination,
            status: exchange.response?.status,
            responseContentType: exchange.response.flatMap { $0.headers.text("Content-Type") },
            requestHeaders: exchange.request.headers,
            responseHeaders: exchange.response?.headers ?? Headers(),
            requestBody: bodyText(inspection.request),
            responseBody: inspection.response.flatMap(bodyText)
        )
    }

    // MARK: - What each kind is given

    private static func make(
        kind: RuleActionKind,
        scheme: Scheme,
        url: String,
        method: String,
        destination: Destination?,
        status: UInt16?,
        responseContentType: String?,
        requestHeaders: Headers,
        responseHeaders: Headers,
        requestBody: String?,
        responseBody: String?
    ) -> RuleDraftSeed {
        let host = matchHost(url: url, destination: destination)
        let path = matchPath(url)
        let method = matchMethod(method)
        var seed = RuleDraftSeed(
            name: suggestedName(kind: kind, host: host, path: path),
            host: host,
            path: path,
            method: method,
            localStatus: nil,
            localContentType: nil,
            destination: nil,
            redirectURL: nil,
            modifyRequest: ModifyRequestDraft(),
            modifyResponse: ModifyResponseDraft()
        )

        // Where the request actually went, so Map Remote and Redirect start
        // from a destination the person can edit rather than from a blank field.
        let remote = remoteURL(scheme: scheme, url: url, destination: destination)

        switch kind {
        case .mapLocal:
            if let status, (100...599).contains(Int(status)) {
                seed.localStatus = String(status)
            }
            seed.localContentType = mediaType(responseContentType)
        case .mapRemote:
            seed.destination = remote
        case .redirect:
            seed.redirectURL = remote
        case .modifyRequest:
            var draft = ModifyRequestDraft()
            draft.query = queryEdits(url)
            draft.headers = headerEdits(requestHeaders)
            if let requestBody {
                draft.body.mode = .replace
                draft.body.text = requestBody
            }
            seed.modifyRequest = draft
        case .modifyResponse:
            var draft = ModifyResponseDraft()
            if let status, (200...599).contains(Int(status)) {
                draft.status = String(status)
            }
            draft.headers = headerEdits(responseHeaders)
            if let responseBody {
                draft.body.mode = .replace
                draft.body.text = responseBody
            }
            seed.modifyResponse = draft
        case .block, .allow:
            break
        }

        return seed
    }

    /// The host a rule matches: the one in the URL, which is the request's,
    /// and the destination only when the URL has none.
    private static func matchHost(url: String, destination: Destination?) -> String {
        if let host = URLComponents(string: url)?.host, !host.isEmpty {
            return host
        }

        return destination?.host ?? ""
    }

    /// The path a rule matches, without the query. Empty when it is not a path
    /// the editor would accept.
    private static func matchPath(_ url: String) -> String {
        guard let components = URLComponents(string: url), components.host != nil else { return "" }

        let path = components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath

        return TransformationChecks.isPath(path) ? path : ""
    }

    /// The method a rule matches, or empty when it is not a method token.
    private static func matchMethod(_ method: String) -> String {
        let method = method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()

        return TransformationChecks.isToken(method) ? method : ""
    }

    private static func suggestedName(kind: RuleActionKind, host: String, path: String) -> String {
        let readablePath = path == "/" ? "" : path
        let name = [kind.title, host, readablePath]
            .filter { !$0.isEmpty }
            .joined(separator: " ")

        guard name.count > maxRuleNameLength else { return name }

        return String(name.prefix(maxRuleNameLength))
    }

    /// `scheme://host[:port][/path][?query]` for the destination the proxy
    /// connected to, keeping the request's path and query.
    ///
    /// A piece the destination parser refuses is dropped and the rest is tried
    /// again, so a query the editor cannot keep does not throw away the host.
    private static func remoteURL(
        scheme: Scheme,
        url: String,
        destination: Destination?
    ) -> String? {
        let components = URLComponents(string: url)
        let host = destination?.host ?? components?.host
        guard let host, !host.isEmpty else { return nil }

        let path = components?.percentEncodedPath
        let query = components?.percentEncodedQuery
        let port = destination?.port

        return validated(scheme: scheme, host: host, port: port, path: path, query: query)
            ?? validated(scheme: scheme, host: host, port: port, path: path, query: nil)
            ?? validated(scheme: scheme, host: host, port: port, path: nil, query: nil)
    }

    private static func validated(
        scheme: Scheme,
        host: String,
        port: UInt16?,
        path: String?,
        query: String?
    ) -> String? {
        let schemeName = scheme == .http ? "http" : "https"
        let defaultPort: UInt16 = scheme == .http ? 80 : 443
        let authority = host.contains(":") ? "[\(host)]" : host
        var url = "\(schemeName)://\(authority)"
        if let port, port != defaultPort, port != 0 {
            url += ":\(port)"
        }
        if let path, !path.isEmpty {
            url += path.hasPrefix("/") ? path : "/\(path)"
        }
        if let query, !query.isEmpty {
            url += "?\(query)"
        }

        return (try? RemoteTarget.parse(url))?.url
    }

    /// The media type, without parameters. `application/json; charset=utf-8`
    /// is `application/json`, which is what a Map Local content type is for.
    private static func mediaType(_ header: String?) -> String? {
        guard let header else { return nil }

        let essence = header.split(separator: ";", maxSplits: 1).first
            .map { $0.trimmingCharacters(in: .whitespaces) } ?? ""

        return essence.isEmpty ? nil : essence
    }

    /// Text the rule can carry. A truncated, binary, or oversized body is left
    /// for the person: a partial body saved as a replacement would not be the
    /// body that was captured, and a rule holds at most 32 KiB of text.
    private static func bodyText(_ inspection: BodyInspection) -> String? {
        guard !inspection.capture.truncated else { return nil }
        guard case let .text(body) = inspection.content, !body.shortened else { return nil }
        guard !body.text.isEmpty, body.text.utf8.count <= maxTransformationBodyBytes else {
            return nil
        }

        return body.text
    }

    /// Headers a Modify rule may start from.
    ///
    /// Not every header. The proxy's own are refused when the rule is written,
    /// and a credential header is not copied into a rule from a right-click.
    /// `Content-Type` is kept first when it is present, because it is the one
    /// a body edit usually wants beside it. At most a few, so the form stays
    /// a form.
    private static func headerEdits(_ headers: Headers) -> [EditableHeaderEdit] {
        let ordered = headers.entries.sorted { left, right in
            let leftType = left.name.caseInsensitiveCompare("Content-Type") == .orderedSame
            let rightType = right.name.caseInsensitiveCompare("Content-Type") == .orderedSame
            if leftType == rightType { return false }

            return leftType
        }

        var edits: [EditableHeaderEdit] = []
        for header in ordered {
            guard edits.count < headerCap else { break }
            guard let edit = headerEdit(header) else { continue }

            edits.append(edit)
        }

        return edits
    }

    private static func headerEdit(_ header: Header) -> EditableHeaderEdit? {
        let name = header.name.trimmingCharacters(in: .whitespaces)
        guard TransformationChecks.isToken(name) else { return nil }
        guard !omittedHeaders.contains(name.asciiLowercased()) else { return nil }
        guard let value = header.value.text, value.utf8.count <= 1024 else { return nil }
        guard
            !value.unicodeScalars.contains(where: {
                ($0.value < 0x20 && $0.value != 0x09) || $0.value == 0x7F
            })
        else { return nil }

        return EditableHeaderEdit(operation: .set, name: name, value: value)
    }

    /// Query parameters as they appear in the URL, skipping any the editor
    /// would refuse. At most a few.
    private static func queryEdits(_ url: String) -> [EditableQueryEdit] {
        var edits: [EditableQueryEdit] = []
        for parameter in QueryParameters.of(url: url) {
            guard edits.count < queryCap else { break }
            guard let value = parameter.rawValue else { continue }

            let row = EditableQueryEdit(operation: .set, name: parameter.rawName, value: value)
            guard TransformationChecks.query([row]) == nil else { continue }

            edits.append(row)
        }

        return edits
    }

    /// How many header edits a draft starts with.
    private static let headerCap = 4
    /// How many query edits a draft starts with.
    private static let queryCap = 4

    /// Headers that are not copied out of a captured message into a rule.
    private static let omittedHeaders: Set<String> = protectedHeaderNames.union([
        "authorization", "proxy-authorization", "cookie", "set-cookie",
    ])
}

/// A rule action started from one traffic row, before a rule is chosen or written.
public struct TrafficRuleRequest: Sendable {
    /// Which kind of rule the menu asked for.
    public var kind: RuleActionKind
    /// The exchange the draft is taken from.
    public var exchangeID: String
    /// What the editor is filled with if the exchange cannot be read again.
    public var seed: RuleDraftSeed

    public init(kind: RuleActionKind, exchangeID: String, seed: RuleDraftSeed) {
        self.kind = kind
        self.exchangeID = exchangeID
        self.seed = seed
    }
}
