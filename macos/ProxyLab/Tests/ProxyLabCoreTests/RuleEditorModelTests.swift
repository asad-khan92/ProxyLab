//  The editor: what it holds, what it refuses, and what it sends.
//
//  The two things worth being sure of are that a draft becomes exactly the rule
//  it looks like, and that a save that did not work leaves everything still in
//  the editor. Nothing here opens a socket or a panel.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Rule editor")
struct RuleEditorModelTests {
    /// A temporary file a Map Local rule can legitimately name.
    private func fixtureFile() throws -> String {
        let path = NSTemporaryDirectory() + "proxylab-editor-\(UUID().uuidString).json"
        try Data(#"{"ok":true}"#.utf8).write(to: URL(fileURLWithPath: path))

        return path
    }

    private func editor(
        creating kind: RuleActionKind,
        session: FakeRulesSession = FakeRulesSession(),
        chooser: FakeFileChooser = FakeFileChooser()
    ) -> RuleEditorModel {
        RuleEditorModel(creating: kind, session: session, chooser: chooser)
    }

    // MARK: - What a new rule starts as

    @Test("a new rule starts enabled, at priority zero, matching everything")
    func newRuleDefaults() {
        let editor = editor(creating: .block)

        #expect(editor.mode == .create)
        #expect(editor.title == "New Rule")
        #expect(editor.saveTitle == "Create Rule")
        #expect(editor.enabled)
        #expect(editor.priority == "0")
        #expect(editor.host.isEmpty)
        #expect(editor.path.isEmpty)
        #expect(editor.method.isEmpty)
        #expect(editor.blockStatus == "403")
    }

    // MARK: - Creating

    @Test("a block rule is created as the rule it was written as")
    func createBlock() async throws {
        let session = FakeRulesSession()
        let created = RuleFixtures.rule(id: "new", name: "block analytics")
        await session.answerCreate(with: .success(created))

        let editor = editor(creating: .block, session: session)
        editor.name = "block analytics"
        editor.host = "analytics.example.com"
        editor.path = "/collect"
        editor.pathIsPrefix = true
        editor.method = "POST"
        editor.priority = "10"
        editor.blockStatus = "451"

        let saved = await editor.save()

        #expect(saved == created)
        let calls = await session.calls
        guard case let .create(new) = calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(new.name == "block analytics")
        #expect(new.priority == 10)
        #expect(new.matcher.host == .exact("analytics.example.com"))
        #expect(new.matcher.path == .prefix("/collect"))
        #expect(new.matcher.method == .exact("POST"))
        #expect(new.action == .block(BlockAction(status: 451)))
    }

    @Test("a criterion left empty is left unset, which is what matches anything")
    func emptyCriteriaAreUnset() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))

        let editor = editor(creating: .block, session: session)
        editor.name = "block everything"
        _ = await editor.save()

        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(new.matcher.isAny)
    }

    @Test("a map-local rule is created with the file that was chosen")
    func createMapLocal() async throws {
        let path = try fixtureFile()
        defer { try? FileManager.default.removeItem(atPath: path) }

        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))
        let chooser = FakeFileChooser(answer: path)

        let editor = editor(creating: .mapLocal, session: session, chooser: chooser)
        editor.name = "mock the config"
        editor.chooseLocalFile()
        editor.localContentType = "application/json"
        _ = await editor.save()

        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(
            new.action
                == .mapLocal(
                    MapLocalAction(path: path, contentType: "application/json", status: 200)
                )
        )
    }

    @Test("a map-remote rule is created with the destination split into parts")
    func createMapRemote() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))

        let editor = editor(creating: .mapRemote, session: session)
        editor.name = "point at staging"
        editor.destination = "https://staging.example.com:8443/v2"
        _ = await editor.save()

        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(
            new.action
                == .mapRemote(
                    RemoteTarget(
                        scheme: .https,
                        host: "staging.example.com",
                        port: 8443,
                        path: "/v2"
                    )
                )
        )
    }

    @Test("a create the core refuses leaves the draft exactly as it was")
    func createRefused() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(
            with: .failure(RuleFixtures.refusal("a rule must have a name"))
        )

        let editor = editor(creating: .block, session: session)
        // A name the editor's own check accepts, so the refusal is the core's.
        editor.name = "a name"
        editor.host = "api.example.com"

        let saved = await editor.save()

        #expect(saved == nil)
        #expect(editor.name == "a name")
        #expect(editor.host == "api.example.com")
        #expect(editor.fieldErrors[.name] == "A rule must have a name.")
    }

    @Test("a refusal that names no field is shown on the form")
    func refusalWithoutAField() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .failure(.connectionClosed))

        let editor = editor(creating: .block, session: session)
        editor.name = "a name"

        #expect(await editor.save() == nil)
        guard case let .failed(reason) = editor.saveState else {
            Issue.record("expected a failed save")
            return
        }

        #expect(reason.contains("closed"))
        #expect(editor.fieldErrors.isEmpty)
    }

    @Test("the core's refusals land under the field they are about")
    func refusalsFindTheirField() async throws {
        let cases: [(RuleActionKind, String, RuleEditorField)] = [
            (.block, "a block status must be between 100 and 599", .blockStatus),
            (.block, "a rule name must be at most 200 characters", .name),
            (.mapLocal, "a map-local path must be absolute", .localPath),
            (.mapLocal, "a map-local content type must be a header value", .localContentType),
            (.mapLocal, "a map-local status must be between 100 and 599", .localStatus),
            (.mapRemote, "a map-remote host is not a host", .destination),
        ]

        let path = try fixtureFile()
        defer { try? FileManager.default.removeItem(atPath: path) }

        for (kind, message, field) in cases {
            let session = FakeRulesSession()
            await session.answerCreate(with: .failure(RuleFixtures.refusal(message)))

            let editor = editor(
                creating: kind,
                session: session,
                chooser: FakeFileChooser(answer: path)
            )
            editor.name = "a name"
            if kind == .mapLocal { editor.chooseLocalFile() }
            if kind == .mapRemote { editor.destination = "https://example.com" }

            _ = await editor.save()

            #expect(editor.fieldErrors[field] != nil, "\(message)")
            #expect(editor.saveState == .editing, "\(message)")
        }
    }

    // MARK: - Editing

    @Test("an existing rule opens with every field as the core holds it")
    func editingPopulates() throws {
        let rule = try JSONDecoder().decode(
            Rule.self,
            from: Data(RuleFixtures.mapRemoteRuleJSON.utf8)
        )
        let editor = RuleEditorModel(
            editing: rule,
            session: FakeRulesSession(),
            chooser: FakeFileChooser()
        )

        #expect(editor.mode == .edit(ruleID: rule.id))
        #expect(editor.title == "Edit Rule")
        #expect(editor.saveTitle == "Save Changes")
        #expect(editor.name == "point at staging")
        #expect(editor.enabled)
        #expect(editor.priority == "30")
        #expect(editor.host == "api.example.com")
        #expect(editor.path.isEmpty)
        #expect(editor.method.isEmpty)
        #expect(editor.actionKind == .mapRemote)
        #expect(editor.destination == "https://staging.example.com:8443/v2?debug=1")
    }

    @Test("a map-local rule opens with its file and content type")
    func editingMapLocal() throws {
        let rule = try JSONDecoder().decode(
            Rule.self,
            from: Data(RuleFixtures.mapLocalRuleJSON.utf8)
        )
        let editor = RuleEditorModel(
            editing: rule,
            session: FakeRulesSession(),
            chooser: FakeFileChooser()
        )

        #expect(!editor.enabled)
        #expect(editor.actionKind == .mapLocal)
        #expect(editor.localPath == "/Users/dev/mocks/config.json")
        #expect(editor.localContentType.isEmpty)
        #expect(editor.localStatus == "200")
        #expect(editor.pathIsPrefix == false)
        #expect(editor.path == "/config")
    }

    @Test("an edit sends only what was changed")
    func updateSendsOnlyChanges() async throws {
        let rule = RuleFixtures.rule(id: "rule-a", name: "old", priority: 10)
        let session = FakeRulesSession()
        await session.answerUpdate(with: .success(rule))

        let editor = RuleEditorModel(
            editing: rule,
            session: session,
            chooser: FakeFileChooser()
        )
        editor.name = "new"
        _ = await editor.save()

        let updates = await session.updates()
        #expect(updates.count == 1)
        #expect(updates.first?.id == "rule-a")
        #expect(updates.first?.update == RuleUpdate(name: "new"))
    }

    @Test("an edit that changed nothing sends nothing at all")
    func unchangedEditSendsNothing() async throws {
        let rule = RuleFixtures.rule(id: "rule-a", name: "unchanged", priority: 7)
        let session = FakeRulesSession()

        let editor = RuleEditorModel(
            editing: rule,
            session: session,
            chooser: FakeFileChooser()
        )

        #expect(!editor.hasChanges)
        #expect(await editor.save() == rule)
        #expect(await session.calls.isEmpty)
    }

    @Test("a rule removed while it was open says so rather than being recreated")
    func editedRuleVanished() async throws {
        let rule = RuleFixtures.rule(id: "rule-a", name: "old")
        let session = FakeRulesSession()
        await session.answerUpdate(with: .failure(RuleFixtures.notFound))

        let editor = RuleEditorModel(
            editing: rule,
            session: session,
            chooser: FakeFileChooser()
        )
        editor.name = "new"

        #expect(await editor.save() == nil)
        #expect(editor.isGone)
        #expect(editor.saveState == .gone)
    }

    @Test("an unusual method a rule already names survives being opened")
    func unusualMethodKept() {
        let rule = RuleFixtures.rule(
            id: "rule-a",
            matcher: RuleMatcher(method: .exact("PURGE"))
        )
        let editor = RuleEditorModel(
            editing: rule,
            session: FakeRulesSession(),
            chooser: FakeFileChooser()
        )

        #expect(editor.method == "PURGE")
        #expect(editor.methodChoices.contains("PURGE"))
        #expect(editor.methodChoices.count == commonHTTPMethods.count + 1)
    }

    @Test("a common method does not appear in the menu twice")
    func commonMethodNotDuplicated() {
        let editor = editor(creating: .block)
        editor.method = "GET"

        #expect(editor.methodChoices == commonHTTPMethods)
    }

    // MARK: - Checking a draft

    @Test("a rule with no name is refused before anything is sent")
    func nameRequired() async throws {
        let session = FakeRulesSession()
        let editor = editor(creating: .block, session: session)
        editor.name = "   "

        #expect(await editor.save() == nil)
        #expect(editor.fieldErrors[.name] == "A rule must have a name.")
        #expect(await session.calls.isEmpty)
    }

    @Test("a name longer than the core allows is refused")
    func nameTooLong() async throws {
        let editor = editor(creating: .block)
        editor.name = String(repeating: "x", count: maxRuleNameLength + 1)

        #expect(await editor.save() == nil)
        #expect(editor.fieldErrors[.name]?.contains("200") == true)
    }

    @Test("a name carrying a control character is refused")
    func nameControlCharacter() async throws {
        let editor = editor(creating: .block)
        editor.name = "block\u{0007}it"

        #expect(await editor.save() == nil)
        #expect(editor.fieldErrors[.name] != nil)
    }

    @Test("a priority that is not a whole number is refused")
    func priorityMustBeANumber() async throws {
        for priority in ["", "high", "1.5", "99999999999"] {
            let editor = editor(creating: .block)
            editor.name = "a name"
            editor.priority = priority

            #expect(await editor.save() == nil, "\(priority)")
            #expect(editor.fieldErrors[.priority] != nil, "\(priority)")
        }
    }

    @Test("a negative priority is a priority: it is consulted first")
    func negativePriority() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))

        let editor = editor(creating: .block, session: session)
        editor.name = "a name"
        editor.priority = "-5"
        _ = await editor.save()

        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(new.priority == -5)
    }

    @Test("a matching criterion that could never match is refused")
    func impossibleCriteria() async throws {
        let cases: [(String, RuleEditorField)] = [
            ("host", .host),
            ("path", .path),
            ("method", .method),
        ]

        for (field, expected) in cases {
            let editor = editor(creating: .block)
            editor.name = "a name"
            switch field {
            case "host": editor.host = "api example.com"
            case "path": editor.path = "no-leading-slash"
            default: editor.method = "GET POST"
            }

            #expect(await editor.save() == nil, "\(field)")
            #expect(editor.fieldErrors[expected] != nil, "\(field)")
        }
    }

    @Test("a host with a slash in it is a URL and not a host")
    func hostWithSlash() async throws {
        let editor = editor(creating: .block)
        editor.name = "a name"
        editor.host = "api.example.com/v1"

        #expect(await editor.save() == nil)
        #expect(editor.fieldErrors[.host] != nil)
    }

    @Test("a status outside what a response can carry is refused")
    func statusRange() async throws {
        for status in ["0", "99", "600", "", "ok"] {
            let editor = editor(creating: .block)
            editor.name = "a name"
            editor.blockStatus = status

            #expect(await editor.save() == nil, "\(status)")
            #expect(editor.fieldErrors[.blockStatus] != nil, "\(status)")
        }
    }

    @Test("a map-local rule with no file chosen is refused")
    func mapLocalNeedsAFile() async throws {
        let editor = editor(creating: .mapLocal)
        editor.name = "a name"

        #expect(await editor.save() == nil)
        #expect(editor.fieldErrors[.localPath] == LocalFileError.empty.message)
    }

    @Test("a map-local file that is not there is refused, and says so")
    func mapLocalMissingFile() async throws {
        let chooser = FakeFileChooser(answer: "/definitely/not/here.json")
        let editor = editor(creating: .mapLocal, chooser: chooser)
        editor.name = "a name"
        editor.chooseLocalFile()

        #expect(editor.fieldErrors[.localPath] == LocalFileError.missing.message)
        #expect(await editor.save() == nil)
    }

    @Test("a folder chosen instead of a file is refused")
    func mapLocalDirectory() async throws {
        let chooser = FakeFileChooser(answer: NSTemporaryDirectory())
        let editor = editor(creating: .mapLocal, chooser: chooser)
        editor.name = "a name"
        editor.chooseLocalFile()

        #expect(editor.fieldErrors[.localPath] == LocalFileError.isDirectory.message)
    }

    @Test("cancelling the picker leaves the rule pointing where it did")
    func pickerCancelled() async throws {
        let path = try fixtureFile()
        defer { try? FileManager.default.removeItem(atPath: path) }

        let chooser = FakeFileChooser(answer: path)
        let editor = editor(creating: .mapLocal, chooser: chooser)
        editor.chooseLocalFile()
        #expect(editor.localPath == path)

        // Cancelled: nothing chosen, nothing changed, nothing reported.
        chooser.answer = nil
        editor.chooseLocalFile()

        #expect(editor.localPath == path)
        #expect(editor.fieldErrors[.localPath] == nil)
        #expect(chooser.openCount == 2)
    }

    @Test("the picker opens beside the file the rule already names")
    func pickerOpensWhereTheRulePoints() {
        let chooser = FakeFileChooser()
        let rule = RuleFixtures.rule(
            id: "rule-a",
            action: .mapLocal(MapLocalAction(path: "/Users/dev/mocks/config.json"))
        )
        let editor = RuleEditorModel(editing: rule, session: FakeRulesSession(), chooser: chooser)
        editor.chooseLocalFile()

        #expect(chooser.openedAt == ["/Users/dev/mocks/config.json"])
    }

    @Test("a map-remote destination that is not one is refused, in the core's words")
    func mapRemoteDestination() async throws {
        let cases: [(String, RemoteDestinationError)] = [
            ("", .empty),
            ("staging.example.com", .noScheme),
            ("ftp://staging.example.com", .unsupportedScheme),
            ("https://user:pw@staging.example.com", .userinfo),
        ]

        for (destination, expected) in cases {
            let editor = editor(creating: .mapRemote)
            editor.name = "a name"
            editor.destination = destination

            #expect(await editor.save() == nil, "\(destination)")
            #expect(editor.fieldErrors[.destination] == expected.message, "\(destination)")
        }
    }

    @Test("an allow rule needs nothing but a name")
    func allowNeedsNothing() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new", action: .allow)))

        let editor = editor(creating: .allow, session: session)
        editor.name = "except the health check"
        editor.path = "/health"

        #expect(await editor.save() != nil)
        guard case let .create(new) = await session.calls.first else {
            Issue.record("expected a create")
            return
        }

        #expect(new.action == .allow)
    }

    @Test("every field is kept when the action is switched and switched back")
    func switchingActionKeepsFields() {
        let editor = editor(creating: .block)
        editor.blockStatus = "451"
        editor.actionKind = .mapRemote
        editor.destination = "https://example.com"
        editor.actionKind = .block

        #expect(editor.blockStatus == "451")
        #expect(editor.destination == "https://example.com")
    }

    @Test("only the chosen action's fields are checked")
    func onlyTheChosenActionIsChecked() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))

        let editor = editor(creating: .block, session: session)
        editor.name = "a name"
        // Nonsense in the fields of an action this rule does not have.
        editor.destination = "not a url"
        editor.localStatus = "0"

        #expect(await editor.save() != nil)
    }
}
