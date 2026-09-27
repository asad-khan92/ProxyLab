//  A Modify Request or Modify Response rule, while it is being written.
//
//      rows of text ──▶ check ──▶ ModifyRequestAction / ModifyResponseAction
//
//  The same approach the rest of the editor takes: every field is text, the
//  checks are the core's own made early so a mistake is shown where it
//  happened, and the core checks everything again when the rule is saved.
//
//  A row left completely empty is not an edit — it is a row someone added and
//  did not fill in — and is left out rather than refused. A row with a value
//  and no name is refused, because it is half of an edit.

import Foundation

/// The most edits of one kind a transformation may carry, as
/// `rules::MAX_EDITS`.
public let maxTransformationEdits = 32

/// The longest body text a transformation may carry, as
/// `rules::MAX_BODY_TEXT_BYTES`.
public let maxTransformationBodyBytes = 32 * 1024

/// The headers the core will not let a rule touch, as
/// `rules::PROTECTED_HEADERS`: framing, hop-by-hop, and `Host`.
public let protectedHeaderNames: Set<String> = [
    "content-length", "transfer-encoding", "connection", "keep-alive",
    "proxy-connection", "te", "trailer", "upgrade", "host",
]

/// One header edit being typed.
public struct EditableHeaderEdit: Identifiable, Equatable, Sendable {
    public let id: UUID
    public var operation: HeaderEdit.Operation
    public var name: String
    public var value: String

    public init(
        id: UUID = UUID(),
        operation: HeaderEdit.Operation = .set,
        name: String = "",
        value: String = ""
    ) {
        self.id = id
        self.operation = operation
        self.name = name
        self.value = value
    }

    /// Nothing typed at all.
    var isBlank: Bool {
        name.trimmingCharacters(in: .whitespaces).isEmpty
            && value.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var edit: HeaderEdit {
        HeaderEdit(
            operation,
            name: name.trimmingCharacters(in: .whitespaces),
            value: operation == .remove ? "" : value
        )
    }
}

/// One query edit being typed.
public struct EditableQueryEdit: Identifiable, Equatable, Sendable {
    public let id: UUID
    public var operation: QueryEdit.Operation
    public var name: String
    public var value: String

    public init(
        id: UUID = UUID(),
        operation: QueryEdit.Operation = .set,
        name: String = "",
        value: String = ""
    ) {
        self.id = id
        self.operation = operation
        self.name = name
        self.value = value
    }

    var isBlank: Bool { name.isEmpty && value.isEmpty }

    var edit: QueryEdit {
        QueryEdit(operation, name: name, value: operation == .remove ? "" : value)
    }
}

/// What happens to the path, as a choice.
public enum PathEditMode: String, CaseIterable, Identifiable, Sendable {
    case unchanged
    case set
    case replacePrefix

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .unchanged: "Unchanged"
        case .set: "Set to"
        case .replacePrefix: "Replace prefix"
        }
    }
}

/// What happens to the body, as a choice.
public enum BodyEditMode: String, CaseIterable, Identifiable, Sendable {
    case unchanged
    case replace
    case setJSON

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .unchanged: "Unchanged"
        case .replace: "Replace"
        case .setJSON: "Set JSON field"
        }
    }
}

/// A body edit being typed.
public struct BodyDraft: Equatable, Sendable {
    public var mode: BodyEditMode = .unchanged
    /// The whole new body, for a replacement.
    public var text = ""
    /// Keys joined by dots, for a JSON field.
    public var jsonPath = ""
    /// The value as JSON text, for a JSON field.
    public var jsonValue = ""

    public init() {}

    init(_ edit: BodyEdit?) {
        switch edit {
        case nil:
            break
        case let .replace(text):
            mode = .replace
            self.text = text
        case let .setJSON(path, value):
            mode = .setJSON
            jsonPath = path
            jsonValue = value
        }
    }

    var edit: BodyEdit? {
        switch mode {
        case .unchanged: nil
        case .replace: .replace(text)
        case .setJSON:
            .setJSON(
                path: jsonPath.trimmingCharacters(in: .whitespaces),
                value: jsonValue.trimmingCharacters(in: .whitespacesAndNewlines)
            )
        }
    }

    /// What is wrong with it, or `nil`.
    var problem: String? {
        switch mode {
        case .unchanged:
            return nil
        case .replace:
            return text.utf8.count > maxTransformationBodyBytes
                ? "A body here is at most 32 KiB. For a larger one, use Map Local."
                : nil
        case .setJSON:
            let path = jsonPath.trimmingCharacters(in: .whitespaces)
            if path.isEmpty || path.split(separator: ".", omittingEmptySubsequences: false)
                .contains(where: \.isEmpty)
            {
                return "A JSON field is keys joined by dots, like user.address.city."
            }
            let value = jsonValue.trimmingCharacters(in: .whitespacesAndNewlines)
            guard value.utf8.count <= maxTransformationBodyBytes,
                  (try? JSONSerialization.jsonObject(
                      with: Data(value.utf8), options: .fragmentsAllowed
                  )) != nil
            else {
                return "The value must be JSON: \"text\" in quotes, 42, true, null, or {…}."
            }
            return nil
        }
    }
}

/// A Modify Request rule being typed.
public struct ModifyRequestDraft: Equatable, Sendable {
    /// The method to send instead, or empty to keep the request's.
    public var method = ""
    public var pathMode: PathEditMode = .unchanged
    public var path = ""
    public var pathFrom = ""
    public var pathTo = ""
    public var query: [EditableQueryEdit] = []
    public var headers: [EditableHeaderEdit] = []
    public var body = BodyDraft()

    public init() {}

    init(_ action: ModifyRequestAction) {
        method = action.method ?? ""
        switch action.path {
        case nil:
            break
        case let .set(path):
            pathMode = .set
            self.path = path
        case let .replacePrefix(from, to):
            pathMode = .replacePrefix
            pathFrom = from
            pathTo = to
        }
        query = action.query.map { EditableQueryEdit(operation: $0.operation, name: $0.name, value: $0.value) }
        headers = action.headers.map {
            EditableHeaderEdit(operation: $0.operation, name: $0.name, value: $0.value)
        }
        body = BodyDraft(action.body)
    }

    /// The action this describes. Only meaningful once ``problems()`` is
    /// empty.
    public var action: ModifyRequestAction {
        let method = self.method.trimmingCharacters(in: .whitespaces)
        let path: PathEdit? = switch pathMode {
        case .unchanged: nil
        case .set: .set(self.path.trimmingCharacters(in: .whitespaces))
        case .replacePrefix:
            .replacePrefix(
                from: pathFrom.trimmingCharacters(in: .whitespaces),
                to: pathTo.trimmingCharacters(in: .whitespaces)
            )
        }

        return ModifyRequestAction(
            method: method.isEmpty ? nil : method.uppercased(),
            path: path,
            query: query.filter { !$0.isBlank }.map(\.edit),
            headers: headers.filter { !$0.isBlank }.map(\.edit),
            body: body.edit
        )
    }

    /// What is wrong, field by field.
    public func problems() -> [RuleEditorField: String] {
        var problems: [RuleEditorField: String] = [:]

        let method = self.method.trimmingCharacters(in: .whitespaces)
        if !method.isEmpty, !TransformationChecks.isToken(method) {
            problems[.modifyMethod] = "A method is one word, with no spaces."
        }

        let paths: [String] = switch pathMode {
        case .unchanged: []
        case .set: [path]
        case .replacePrefix: [pathFrom, pathTo]
        }
        if paths.contains(where: { !TransformationChecks.isPath($0) }) {
            problems[.modifyPath] = "A path starts with a slash, and has no spaces, ? or #."
        }

        if let problem = TransformationChecks.query(query) { problems[.modifyQuery] = problem }
        if let problem = TransformationChecks.headers(headers) {
            problems[.requestHeaders] = problem
        }
        if let problem = body.problem { problems[.requestBody] = problem }

        let action = self.action
        if problems.isEmpty, action.method == nil, action.path == nil, action.query.isEmpty,
           action.headers.isEmpty, action.body == nil
        {
            problems[.transformation] = "Change at least one thing: the method, the path, "
                + "a query parameter, a header, or the body."
        }

        return problems
    }
}

/// A Modify Response rule being typed.
public struct ModifyResponseDraft: Equatable, Sendable {
    /// The status to give instead, or empty to keep the origin's.
    public var status = ""
    public var headers: [EditableHeaderEdit] = []
    public var body = BodyDraft()

    public init() {}

    init(_ action: ModifyResponseAction) {
        status = action.status.map(String.init) ?? ""
        headers = action.headers.map {
            EditableHeaderEdit(operation: $0.operation, name: $0.name, value: $0.value)
        }
        body = BodyDraft(action.body)
    }

    public var action: ModifyResponseAction {
        ModifyResponseAction(
            status: UInt16(status.trimmingCharacters(in: .whitespaces)),
            headers: headers.filter { !$0.isBlank }.map(\.edit),
            body: body.edit
        )
    }

    public func problems() -> [RuleEditorField: String] {
        var problems: [RuleEditorField: String] = [:]

        let status = self.status.trimmingCharacters(in: .whitespaces)
        if !status.isEmpty {
            if let value = UInt16(status), (200...599).contains(value) {
                // A final status.
            } else {
                problems[.responseStatus] = "A status here is between 200 and 599."
            }
        }
        if let problem = TransformationChecks.headers(headers) {
            problems[.responseHeaders] = problem
        }
        if let problem = body.problem { problems[.responseBody] = problem }

        let action = self.action
        if problems.isEmpty, action.status == nil, action.headers.isEmpty, action.body == nil {
            problems[.transformation] = "Change at least one thing: the status, a header, "
                + "or the body."
        }

        return problems
    }
}

/// The core's checks on the parts of a transformation, made early.
enum TransformationChecks {
    static func isToken(_ text: String) -> Bool {
        !text.isEmpty
            && text.utf8.allSatisfy { byte in
                (byte >= 0x30 && byte <= 0x39)
                    || (byte >= 0x41 && byte <= 0x5A)
                    || (byte >= 0x61 && byte <= 0x7A)
                    || "!#$%&'*+-.^_`|~".utf8.contains(byte)
            }
    }

    static func isPath(_ text: String) -> Bool {
        let text = text.trimmingCharacters(in: .whitespaces)

        return text.hasPrefix("/")
            && text.utf8.allSatisfy { $0 > 0x20 && $0 != 0x7F && $0 != UInt8(ascii: "?")
                && $0 != UInt8(ascii: "#") }
    }

    static func headers(_ rows: [EditableHeaderEdit]) -> String? {
        let rows = rows.filter { !$0.isBlank }
        if rows.count > maxTransformationEdits {
            return "At most \(maxTransformationEdits) header edits."
        }

        for row in rows {
            let name = row.name.trimmingCharacters(in: .whitespaces)
            if !isToken(name) {
                return "A header name is one word, with no spaces or colons."
            }
            if protectedHeaderNames.contains(name.lowercased()) {
                return "\(name) is the proxy's to set. Where a request goes is Map Remote's."
            }
            if row.operation != .remove,
               row.value.unicodeScalars.contains(where: {
                   ($0.value < 0x20 && $0 != "\t") || $0.value == 0x7F
               })
            {
                return "A header value must not contain line breaks."
            }
        }

        return nil
    }

    static func query(_ rows: [EditableQueryEdit]) -> String? {
        let rows = rows.filter { !$0.isBlank }
        if rows.count > maxTransformationEdits {
            return "At most \(maxTransformationEdits) query edits."
        }

        for row in rows {
            let nameOK = !row.name.isEmpty
                && row.name.utf8.allSatisfy { $0 > 0x20 && $0 != 0x7F && !"&=#".utf8.contains($0) }
            let valueOK = row.operation == .remove
                || row.value.utf8.allSatisfy { $0 > 0x20 && $0 != 0x7F && !"&#".utf8.contains($0) }
            if !nameOK || !valueOK {
                return "A query parameter needs a name, and no spaces, & or #. "
                    + "Write it as it appears in the URL, percent-encoded."
            }
        }

        return nil
    }
}
