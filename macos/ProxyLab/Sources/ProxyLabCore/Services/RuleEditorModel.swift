//  Editing one rule, before the core has been told about it.
//
//      draft ──▶ check ──▶ create_rule / update_rule ──▶ the rule the core kept
//        ▲                          │
//        └──────── refused ─────────┘
//
//  Every field here is text, because a half-typed port is text and not a
//  number, and an editor that refuses to hold what someone is in the middle of
//  typing is an editor that fights them. The draft becomes a rule only when it
//  is sent.
//
//  # What this checks, and what it does not decide
//
//  The checks below are the core's own, made early so a mistake is shown under
//  the field that caused it rather than as a refusal after a round trip. They
//  are not the authority and are not treated as one: every save goes to the
//  core, the core validates the whole rule again, and a refusal from it is
//  shown the same way — under the field where that is possible, and as a
//  message on the form where it is not.
//
//  # An edit sends what changed
//
//  `update_rule` carries only the fields that differ from the rule as it was
//  loaded. A field nobody touched is a field the core leaves alone, so an edit
//  to a rule's matcher does not overwrite a priority something else moved in
//  the meantime.

import Foundation
import Observation

/// Which part of the editor a message belongs under.
public enum RuleEditorField: Hashable, Sendable {
    case name
    case priority
    case host
    case path
    case method
    case blockStatus
    case localPath
    case localContentType
    case localStatus
    case destination
    case redirectURL
    case modifyMethod
    case modifyPath
    case modifyQuery
    case requestHeaders
    case requestBody
    case responseStatus
    case responseHeaders
    case responseBody
    /// A transformation as a whole: one that changes nothing.
    case transformation
}

/// What the editor is for.
public enum RuleEditorMode: Sendable, Equatable {
    /// A rule that does not exist yet.
    case create
    /// A rule that does, named by its id.
    case edit(ruleID: String)

    /// Whether this is a new rule.
    public var isCreate: Bool { self == .create }
}

/// What the editor is doing about saving.
public enum RuleSaveState: Sendable, Equatable {
    /// Nothing has been sent.
    case editing
    /// A save is in flight.
    case saving
    /// The core refused, and said why.
    case failed(String)
    /// The rule is not there any more: something else removed it while it was
    /// being edited.
    case gone
}

/// The methods the editor offers, before a rule's own is added to them.
///
/// The matcher compares a method exactly and without regard to case, so this
/// is a convenience and not a constraint: a rule that already names something
/// else keeps it, and it appears in the menu for as long as that rule is open.
public let commonHTTPMethods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]

/// One rule being written or changed.
@MainActor
@Observable
public final class RuleEditorModel {
    /// What is being edited.
    ///
    /// A duplicate starts from an existing rule and then becomes a create, so
    /// this is set once the fields have been copied and not only in `init`.
    public private(set) var mode: RuleEditorMode

    // MARK: - The draft

    /// What a person calls the rule.
    public var name: String
    /// Whether the rule is consulted once it is saved.
    public var enabled: Bool
    /// Where it sits in the order, as typed.
    public var priority: String

    /// The host to match, or empty for every host.
    public var host: String
    /// The path to match, or empty for every path.
    public var path: String
    /// Whether the path is a prefix rather than an exact path.
    public var pathIsPrefix: Bool
    /// The method to match, or empty for every method.
    public var method: String

    /// Which action the rule has.
    public var actionKind: RuleActionKind

    /// The status a block answers with, as typed.
    public var blockStatus: String
    /// The file a Map Local rule answers with.
    public private(set) var localPath: String
    /// The content type it answers with, or empty to let the core work it out.
    public var localContentType: String
    /// The status it answers with, as typed.
    public var localStatus: String
    /// Where a Map Remote rule sends the request, as typed.
    public var destination: String
    /// Where a Redirect rule sends the client, as typed.
    public var redirectURL: String
    /// Which of the four redirect statuses it answers with.
    public var redirectStatus: UInt16
    /// What a Modify Request rule changes.
    public var modifyRequest: ModifyRequestDraft
    /// What a Modify Response rule changes.
    public var modifyResponse: ModifyResponseDraft

    // MARK: - What is wrong with it

    /// The message to show under each field, if any.
    public private(set) var fieldErrors: [RuleEditorField: String] = [:]
    /// A refusal that belongs to no one field.
    public private(set) var formError: String?
    /// Whether a save is in flight, refused, or has not been tried.
    public private(set) var saveState: RuleSaveState = .editing

    /// The rule as it was loaded, for an edit: what a change is measured
    /// against, so only what differs is sent. A duplicate clears this, because
    /// the draft is a new rule and not a change to the one it was copied from.
    private var original: Rule?

    /// A title that is not "New Rule" or "Edit Rule". Set for a duplicate.
    private var titleOverride: String? = nil

    private let session: any RulesSession
    private let chooser: any LocalFileChooser
    private let fileManager: FileManager

    /// A new rule, with `kind` chosen and, when `seed` is set, the fields a
    /// captured exchange can fill in for that kind.
    public init(
        creating kind: RuleActionKind,
        seed: RuleDraftSeed? = nil,
        session: any RulesSession,
        chooser: any LocalFileChooser,
        fileManager: FileManager = .default
    ) {
        mode = .create
        original = nil
        self.session = session
        self.chooser = chooser
        self.fileManager = fileManager

        name = ""
        enabled = true
        priority = "0"
        host = ""
        path = ""
        pathIsPrefix = false
        method = ""
        actionKind = kind
        blockStatus = String(BlockAction.defaultStatus)
        localPath = ""
        localContentType = ""
        localStatus = "200"
        destination = ""
        redirectURL = ""
        redirectStatus = RedirectAction.defaultStatus
        modifyRequest = ModifyRequestDraft()
        modifyResponse = ModifyResponseDraft()

        if let seed { apply(seed) }
    }

    /// A new rule whose fields are copied from `rule`.
    ///
    /// The id is not copied. Saving sends `create_rule`, and the core mints a
    /// new one. The name gains " Copy". The priority is the source's, so the
    /// newer id sorts the copy just after it when the priorities are equal.
    /// Nothing transient — a refusal, a save in flight — is copied, because
    /// those belong to the editor session and not to the rule.
    public convenience init(
        duplicating rule: Rule,
        session: any RulesSession,
        chooser: any LocalFileChooser,
        fileManager: FileManager = .default
    ) {
        self.init(editing: rule, session: session, chooser: chooser, fileManager: fileManager)
        mode = .create
        original = nil
        name = Self.duplicateName(for: rule.name)
        titleOverride = "Duplicate Rule"
    }

    /// "`name` Copy", shortened so it still fits in the name the core allows.
    static func duplicateName(for name: String) -> String {
        let suffix = " Copy"
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let combined = trimmed + suffix
        guard combined.count > maxRuleNameLength else { return combined }

        let keep = max(0, maxRuleNameLength - suffix.count)
        let head = String(trimmed.prefix(keep)).trimmingCharacters(in: .whitespacesAndNewlines)

        return head + suffix
    }

    /// An existing rule, with every field as the core holds it.
    public init(
        editing rule: Rule,
        session: any RulesSession,
        chooser: any LocalFileChooser,
        fileManager: FileManager = .default
    ) {
        mode = .edit(ruleID: rule.id)
        original = rule
        self.session = session
        self.chooser = chooser
        self.fileManager = fileManager

        name = rule.name
        enabled = rule.enabled
        priority = String(rule.priority)
        host = rule.matcher.host?.text ?? ""
        path = rule.matcher.path?.text ?? ""
        pathIsPrefix = rule.matcher.path?.isPrefix ?? false
        method = rule.matcher.method?.text ?? ""
        actionKind = rule.action.kind

        // Every action's fields are filled in, not just the one in use: a
        // person who switches a rule from Block to Map Remote and back again
        // should find what they had typed still there.
        blockStatus = String(BlockAction.defaultStatus)
        localPath = ""
        localContentType = ""
        localStatus = "200"
        destination = ""
        redirectURL = ""
        redirectStatus = RedirectAction.defaultStatus
        modifyRequest = ModifyRequestDraft()
        modifyResponse = ModifyResponseDraft()

        switch rule.action {
        case .allow:
            break
        case let .block(block):
            blockStatus = String(block.status)
        case let .mapLocal(map):
            localPath = map.path
            localContentType = map.contentType ?? ""
            localStatus = String(map.status)
        case let .mapRemote(target):
            destination = target.url
        case let .redirect(redirect):
            redirectURL = redirect.target.url
            redirectStatus = redirect.status
        case let .modifyRequest(modify):
            modifyRequest = ModifyRequestDraft(modify)
        case let .modifyResponse(modify):
            modifyResponse = ModifyResponseDraft(modify)
        }
    }

    /// Copies a draft started from a captured exchange into the fields.
    ///
    /// The file a Map Local rule answers with is not among them: the exchange
    /// does not name a file, and the person still chooses one.
    private func apply(_ seed: RuleDraftSeed) {
        if !seed.name.isEmpty { name = seed.name }
        host = seed.host
        path = seed.path
        pathIsPrefix = false
        method = seed.method
        if let status = seed.localStatus { localStatus = status }
        if let type = seed.localContentType { localContentType = type }
        if let destination = seed.destination { self.destination = destination }
        if let redirectURL = seed.redirectURL { self.redirectURL = redirectURL }
        modifyRequest = seed.modifyRequest
        modifyResponse = seed.modifyResponse
    }

    // MARK: - What the editor shows

    /// The title of the window the editor is in.
    public var title: String {
        if let titleOverride { return titleOverride }

        return mode.isCreate ? "New Rule" : "Edit Rule"
    }

    /// What the save button says.
    public var saveTitle: String {
        mode.isCreate ? "Create Rule" : "Save Changes"
    }

    /// Whether a save is in flight.
    public var isSaving: Bool { saveState == .saving }

    /// Whether the rule was removed from under the editor.
    public var isGone: Bool { saveState == .gone }

    /// The methods the method menu offers.
    ///
    /// The common ones, plus whatever this rule already names if it is not one
    /// of them — so a rule written against an unusual method survives being
    /// opened in the editor.
    public var methodChoices: [String] {
        let method = self.method.trimmingCharacters(in: .whitespaces).uppercased()

        guard !method.isEmpty, !commonHTTPMethods.contains(method) else {
            return commonHTTPMethods
        }

        return commonHTTPMethods + [method]
    }

    /// The file a Map Local rule names, written with `~` for the home folder.
    public var localPathForDisplay: String {
        localPath.isEmpty ? "" : RuleDisplay.abbreviated(localPath)
    }

    /// Whether anything has been changed from the rule as it was loaded.
    ///
    /// Always true for a new rule: there is nothing to compare it to. A draft
    /// that cannot be turned into a rule at all counts as changed, so the save
    /// button stays live and pressing it says what is wrong.
    public var hasChanges: Bool {
        guard original != nil else { return true }
        guard let change = try? change() else { return true }

        return !change.isEmpty
    }

    // MARK: - Choosing a file

    /// Asks for the file a Map Local rule answers with.
    ///
    /// A cancelled pick leaves the rule as it was and is not a failure: it is
    /// what "never mind" looks like.
    public func chooseLocalFile() {
        guard let path = chooser.chooseFile(startingAt: localPath.isEmpty ? nil : localPath)
        else {
            return
        }

        localPath = path
        fieldErrors[.localPath] = LocalFilePath.check(path, in: fileManager)?.message
        if fieldErrors[.localPath] == nil { fieldErrors.removeValue(forKey: .localPath) }
    }

    // MARK: - Saving

    /// Sends the draft, and gives back the rule the core kept.
    ///
    /// `nil` means it was not saved and the editor stays open with everything
    /// still in it. Whatever was wrong is in ``fieldErrors`` or ``formError``.
    public func save() async -> Rule? {
        fieldErrors = [:]
        formError = nil

        let errors = validate()
        guard errors.isEmpty else {
            fieldErrors = errors
            saveState = .editing

            return nil
        }

        saveState = .saving

        do {
            let rule: Rule
            switch mode {
            case .create:
                rule = try await session.createRule(try draft())
            case let .edit(ruleID):
                let update = try change()
                // Nothing was changed. The core refuses an update that names
                // nothing, and there is no reason to ask it to.
                guard !update.isEmpty else {
                    saveState = .editing

                    return original
                }

                rule = try await session.updateRule(id: ruleID, update)
            }

            saveState = .editing

            return rule
        } catch let error as IPCError where error.serverCode == .notFound {
            saveState = .gone

            return nil
        } catch {
            apply(error)

            return nil
        }
    }

    /// Puts a refusal from the core where it belongs.
    private func apply(_ error: any Error) {
        guard let error = error as? IPCError else {
            saveState = .failed("Something went wrong talking to the ProxyLab core.")
            return
        }

        // A refusal of the rule itself carries one of the core's fixed
        // phrases, and most of them name one field.
        if error.serverCode == .invalidRule, case let .server(body) = error {
            if let field = field(for: body.message) {
                fieldErrors[field] = sentence(body.message)
                saveState = .editing

                return
            }
        }

        saveState = .failed(sentence(error.message))
    }

    /// The core's phrases are lowercase fragments; this is one of them as a
    /// sentence, so the editor reads the way the rest of the app does.
    private func sentence(_ message: String) -> String {
        guard let first = message.first else { return message }

        let capitalised = first.uppercased() + message.dropFirst()

        return capitalised.hasSuffix(".") ? capitalised : capitalised + "."
    }

    /// Which field a refusal from the core belongs under, if one does.
    private func field(for message: String) -> RuleEditorField? {
        if message.hasPrefix("a rule name") || message == "a rule must have a name" {
            return .name
        }
        if message.hasPrefix("a block status") { return .blockStatus }
        if message.hasPrefix("a map-local content type") { return .localContentType }
        if message.hasPrefix("a map-local status") { return .localStatus }
        if message.hasPrefix("a map-local") { return .localPath }
        if message.hasPrefix("a map-remote") { return .destination }
        if message.hasPrefix("a redirect") { return .redirectURL }
        if message.hasPrefix("a modify method") { return .modifyMethod }
        if message.hasPrefix("a modify path") { return .modifyPath }
        if message.hasPrefix("a modify query") { return .modifyQuery }
        if message.hasPrefix("a modify status") { return .responseStatus }

        let response = actionKind == .modifyResponse
        if message.hasPrefix("a modify header") {
            return response ? .responseHeaders : .requestHeaders
        }
        if message.hasPrefix("a modify body") || message.hasPrefix("a modify JSON") {
            return response ? .responseBody : .requestBody
        }
        if message.hasPrefix("a modify rule") { return .transformation }

        return nil
    }

    // MARK: - Building the rule

    /// The draft as a rule to create.
    private func draft() throws -> NewRule {
        NewRule(
            enabled: enabled,
            name: trimmedName,
            matcher: try matcher(),
            action: try action(),
            priority: try priorityValue()
        )
    }

    /// What the draft changed, against the rule as it was loaded.
    ///
    /// Internal rather than private so the change an edit sends can be tested
    /// as the value it is.
    func change() throws -> RuleUpdate {
        guard let original else { return RuleUpdate() }

        var update = RuleUpdate()

        let name = trimmedName
        if name != original.name { update.name = name }
        if enabled != original.enabled { update.enabled = enabled }

        let priority = try priorityValue()
        if priority != original.priority { update.priority = priority }

        let matcher = try matcher()
        if matcher != original.matcher { update.matcher = matcher }

        let action = try action()
        if action != original.action { update.action = action }

        return update
    }

    /// The name without the whitespace around it.
    private var trimmedName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The matcher the three criteria describe.
    ///
    /// A criterion left empty is not a criterion: it is left unset, which is
    /// what makes it match anything.
    private func matcher() throws -> RuleMatcher {
        var matcher = RuleMatcher()

        let host = self.host.trimmingCharacters(in: .whitespaces)
        if !host.isEmpty { matcher.host = .exact(host) }

        let path = self.path.trimmingCharacters(in: .whitespaces)
        if !path.isEmpty { matcher.path = pathIsPrefix ? .prefix(path) : .exact(path) }

        let method = self.method.trimmingCharacters(in: .whitespaces)
        if !method.isEmpty { matcher.method = .exact(method.uppercased()) }

        return matcher
    }

    /// The action the chosen kind and its fields describe.
    private func action() throws -> RuleAction {
        switch actionKind {
        case .allow:
            return .allow
        case .block:
            return .block(BlockAction(status: try status(blockStatus, field: .blockStatus)))
        case .mapLocal:
            let contentType = localContentType.trimmingCharacters(in: .whitespaces)

            return .mapLocal(
                MapLocalAction(
                    path: localPath.trimmingCharacters(in: .whitespacesAndNewlines),
                    contentType: contentType.isEmpty ? nil : contentType,
                    status: try status(localStatus, field: .localStatus)
                )
            )
        case .mapRemote:
            return .mapRemote(try RemoteTarget.parse(destination))
        case .redirect:
            return .redirect(
                RedirectAction(target: try RemoteTarget.parse(redirectURL), status: redirectStatus)
            )
        case .modifyRequest:
            return .modifyRequest(modifyRequest.action)
        case .modifyResponse:
            return .modifyResponse(modifyResponse.action)
        }
    }

    /// The priority as the number it has to be.
    private func priorityValue() throws -> Int32 {
        guard let priority = Int32(priority.trimmingCharacters(in: .whitespaces)) else {
            throw DraftError.field(.priority)
        }

        return priority
    }

    /// One status field as the number it has to be.
    private func status(_ text: String, field: RuleEditorField) throws -> UInt16 {
        guard
            let status = UInt16(text.trimmingCharacters(in: .whitespaces)),
            (100...599).contains(status)
        else {
            throw DraftError.field(field)
        }

        return status
    }

    /// Raised when a field cannot be turned into what the rule needs.
    ///
    /// Never surfaced: ``validate()`` has already refused anything that would
    /// raise one, and it exists so the builders above can be total.
    private enum DraftError: Error {
        case field(RuleEditorField)
    }

    // MARK: - Checking the draft

    /// What is wrong with the draft, field by field.
    ///
    /// The core's own checks, made here so they can be shown where they
    /// happened. The core makes them again.
    public func validate() -> [RuleEditorField: String] {
        var errors: [RuleEditorField: String] = [:]

        let name = trimmedName
        if name.isEmpty {
            errors[.name] = "A rule must have a name."
        } else if name.count > maxRuleNameLength {
            errors[.name] = "A rule name must be at most \(maxRuleNameLength) characters."
        } else if name.unicodeScalars.contains(where: { $0.properties.generalCategory == .control })
        {
            errors[.name] = "A rule name must not contain control characters."
        }

        if Int32(priority.trimmingCharacters(in: .whitespaces)) == nil {
            errors[.priority] = "A priority is a whole number between "
                + "\(Int32.min) and \(Int32.max)."
        }

        let host = self.host.trimmingCharacters(in: .whitespaces)
        if !host.isEmpty, host.contains(where: \.isWhitespace) || host.contains("/") {
            errors[.host] = "A host is a name, with no spaces and no slashes."
        }

        let path = self.path.trimmingCharacters(in: .whitespaces)
        if !path.isEmpty, !path.hasPrefix("/") || path.contains(where: \.isWhitespace) {
            errors[.path] = "A path starts with a slash and has no spaces."
        }

        let method = self.method.trimmingCharacters(in: .whitespaces)
        if !method.isEmpty, !Self.isHTTPToken(method) {
            errors[.method] = "A method is one word, with no spaces."
        }

        switch actionKind {
        case .allow:
            break
        case .block:
            if !Self.isStatus(blockStatus) {
                errors[.blockStatus] = "A status is between 100 and 599."
            }
        case .mapLocal:
            if let error = LocalFilePath.check(localPath, in: fileManager) {
                errors[.localPath] = error.message
            }
            let contentType = localContentType.trimmingCharacters(in: .whitespaces)
            if !localContentType.isEmpty, contentType.isEmpty {
                errors[.localContentType] = "A content type must be a header value."
            }
            if !Self.isStatus(localStatus) {
                errors[.localStatus] = "A status is between 100 and 599."
            }
        case .mapRemote:
            do {
                _ = try RemoteTarget.parse(destination)
            } catch let error as RemoteDestinationError {
                errors[.destination] = error.message
            } catch {
                errors[.destination] = "That is not a destination."
            }
        case .redirect:
            do {
                _ = try RemoteTarget.parse(redirectURL)
            } catch RemoteDestinationError.empty {
                errors[.redirectURL] = "A redirect must name where to send the client."
            } catch let error as RemoteDestinationError {
                errors[.redirectURL] = error.message
            } catch {
                errors[.redirectURL] = "That is not a URL."
            }
        case .modifyRequest:
            errors.merge(modifyRequest.problems()) { current, _ in current }
        case .modifyResponse:
            errors.merge(modifyResponse.problems()) { current, _ in current }
        }

        return errors
    }

    /// Whether `text` is a status an HTTP response could carry.
    private static func isStatus(_ text: String) -> Bool {
        guard let status = UInt16(text.trimmingCharacters(in: .whitespaces)) else { return false }

        return (100...599).contains(status)
    }

    /// Whether `text` is one HTTP token, which is what a method is.
    private static func isHTTPToken(_ text: String) -> Bool {
        !text.isEmpty
            && text.utf8.allSatisfy { byte in
                (byte >= 0x30 && byte <= 0x39)
                    || (byte >= 0x41 && byte <= 0x5A)
                    || (byte >= 0x61 && byte <= 0x7A)
                    || "!#$%&'*+-.^_`|~".utf8.contains(byte)
            }
    }
}
