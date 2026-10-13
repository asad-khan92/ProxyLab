//  How a rule reads in a list.
//
//      ● Block       Block analytics
//        analytics.example.com · /collect · POST          priority 10
//
//  One place, so the list row, the detail pane, and the editor's summary line
//  all describe a rule the same way — and so that "any host" is worded once.
//  Nothing here decides anything: it turns a rule into the words for a rule.

import Foundation

/// The words a rule is shown with.
public enum RuleDisplay {
    /// What an unset criterion is called.
    ///
    /// A matcher field that is not set matches everything, and that is the one
    /// thing about the matching a person has to understand before writing a
    /// rule. It is never left blank on screen.
    public static let anything = "Any"

    /// The matcher as one line: the criteria that are set, in order.
    ///
    /// A matcher that sets nothing says so, because a rule that matches every
    /// request is a rule worth noticing before it is enabled.
    public static func matcherSummary(_ matcher: RuleMatcher) -> String {
        var parts: [String] = []

        if let host = matcher.host { parts.append(host.text) }
        if let path = matcher.path {
            parts.append(path.isPrefix ? "\(path.text)*" : path.text)
        }
        if let method = matcher.method { parts.append(method.text.uppercased()) }

        return parts.isEmpty ? "Every request" : parts.joined(separator: " · ")
    }

    /// The matcher as the rules list and the traffic-rule chooser both read it.
    ///
    /// Method, then host and path, the way a request is read:
    /// `GET api.example.com/products`. A prefix path keeps the star the list
    /// already uses. A matcher that sets nothing says so.
    static func chooserMatch(_ matcher: RuleMatcher) -> String {
        var location = matcher.host?.text ?? ""
        if let path = matcher.path {
            location += path.isPrefix ? "\(path.text)*" : path.text
        }
        let method = matcher.method?.text.uppercased() ?? ""
        let line = [method, location].filter { !$0.isEmpty }.joined(separator: " ")

        return line.isEmpty ? "Every request" : line
    }

    /// Where the action sends a match, or what it changes, on one line.
    ///
    /// Shared by the rules list and the traffic-rule chooser.
    ///
    /// Header values are left out, the same way the rule list leaves them
    /// out. An action with nothing to point at — Allow, or a Modify rule
    /// that changes nothing — returns nil, and the chooser shows the match
    /// alone.
    static func chooserTarget(_ action: RuleAction) -> String? {
        switch action {
        case .allow:
            return nil
        case let .block(block):
            return "Answered \(block.status)"
        case let .mapLocal(map):
            let name = fileName(of: map.path)
            return name.isEmpty ? nil : "→ \(name)"
        case let .mapRemote(target):
            return "→ \(destinationSummary(target))"
        case let .redirect(redirect):
            return "\(redirect.status) → \(destinationSummary(redirect.target))"
        case let .modifyRequest(modify):
            let parts = changes(of: modify)
            return parts.isEmpty ? nil : parts.joined(separator: " · ")
        case let .modifyResponse(modify):
            let parts = changes(of: modify)
            return parts.isEmpty ? nil : parts.joined(separator: " · ")
        }
    }

    /// Host, port when it is not the scheme's own, and path when the rule names one.
    private static func destinationSummary(_ target: RemoteTarget) -> String {
        var text = target.authority
        if let path = target.path, !path.isEmpty, path != "/" {
            text += path
        }

        return text
    }

    /// What the action does with a matching request, in a few words.
    public static func actionSummary(_ action: RuleAction) -> String {
        switch action {
        case .allow:
            "Carried as usual"
        case let .block(block):
            "Answered \(block.status)"
        case let .mapLocal(map):
            fileName(of: map.path)
        case let .mapRemote(target):
            target.url
        case let .redirect(redirect):
            "\(redirect.status) → \(redirect.target.url)"
        case let .modifyRequest(modify):
            changes(of: modify).joined(separator: " · ")
        case let .modifyResponse(modify):
            changes(of: modify).joined(separator: " · ")
        }
    }

    /// Each thing a Modify Request rule changes, as a short phrase.
    ///
    /// Header values are not shown: the header a rule most often sets is
    /// `Authorization`, and a rule list is on screen.
    public static func changes(of modify: ModifyRequestAction) -> [String] {
        var parts: [String] = []

        if let method = modify.method { parts.append("Method \(method)") }
        switch modify.path {
        case let .set(path): parts.append("Path \(path)")
        case let .replacePrefix(from, to): parts.append("Path \(from)… → \(to)…")
        case nil: break
        }
        parts += modify.query.map { edit in
            edit.operation == .remove ? "Remove ?\(edit.name)" : "Set ?\(edit.name)"
        }
        parts += modify.headers.map(headerPhrase)
        if let body = modify.body { parts.append(bodyPhrase(body)) }

        return parts
    }

    /// Each thing a Modify Response rule changes, as a short phrase.
    public static func changes(of modify: ModifyResponseAction) -> [String] {
        var parts: [String] = []

        if let status = modify.status { parts.append("Status \(status)") }
        parts += modify.headers.map(headerPhrase)
        if let body = modify.body { parts.append(bodyPhrase(body)) }

        return parts
    }

    private static func headerPhrase(_ edit: HeaderEdit) -> String {
        "\(edit.operation.title) \(edit.name)"
    }

    private static func bodyPhrase(_ edit: BodyEdit) -> String {
        switch edit {
        case .replace: "Replace body"
        case let .setJSON(path, _): "Set JSON \(path)"
        }
    }

    /// The last path component of a map-local file, for a row that has no room
    /// for the whole path.
    public static func fileName(of path: String) -> String {
        let name = (path as NSString).lastPathComponent

        return name.isEmpty ? path : name
    }

    /// A path with the home directory written as `~`.
    ///
    /// What the editor shows: the whole path, without the part of it that is
    /// the same for everything on this Mac.
    public static func abbreviated(_ path: String) -> String {
        (path as NSString).abbreviatingWithTildeInPath
    }

    /// The host criterion as the editor's placeholder reads it.
    public static func criterionText(_ text: String?) -> String {
        guard let text, !text.isEmpty else { return anything }

        return text
    }
}
