//  Modify Request, Modify Response and Redirect, on the Swift side of the
//  boundary.
//
//  Two things are pinned. The JSON is byte-for-byte what `core/rules` writes —
//  the same objects `rules/tests/transformations.rs` asserts on — because the
//  core refuses a field it does not know and a key spelled differently would
//  be a rule that cannot be saved. And the editor turns rows of text into
//  exactly the transformation they look like, leaves out rows nobody filled
//  in, and refuses what the core would refuse under the field it belongs to.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Rule transformations on the wire")
struct RuleTransformationWireTests {
    private func json(_ action: RuleAction) throws -> NSDictionary {
        let data = try JSONEncoder().encode(action)
        return try #require(JSONSerialization.jsonObject(with: data) as? NSDictionary)
    }

    private func decoded(_ text: String) throws -> RuleAction {
        try JSONDecoder().decode(RuleAction.self, from: Data(text.utf8))
    }

    @Test("a modify-request action is written as the core writes it")
    func modifyRequestJSON() throws {
        let action = RuleAction.modifyRequest(
            ModifyRequestAction(
                method: "POST",
                path: .replacePrefix(from: "/v1/", to: "/v2/"),
                query: [QueryEdit(.set, name: "env", value: "staging")],
                headers: [HeaderEdit(.remove, name: "Cookie")],
                body: .setJSON(path: "user.role", value: "\"admin\"")
            )
        )

        let expected: NSDictionary = [
            "type": "modify_request",
            "method": "POST",
            "path": ["op": "replace_prefix", "from": "/v1/", "to": "/v2/"],
            "query": [["op": "set", "name": "env", "value": "staging"]],
            "headers": [["op": "remove", "name": "Cookie"]],
            "body": ["op": "set_json", "path": "user.role", "value": "\"admin\""],
        ]

        #expect(try json(action) == expected)
    }

    @Test("a modify-response action leaves out what it does not change")
    func modifyResponseJSON() throws {
        let action = RuleAction.modifyResponse(ModifyResponseAction(status: 404))

        #expect(try json(action) == ["type": "modify_response", "status": 404])
    }

    @Test("a redirect is its target and its status")
    func redirectJSON() throws {
        let action = RuleAction.redirect(
            RedirectAction(target: RemoteTarget(scheme: .https, host: "new.example.com"), status: 301)
        )

        let expected: NSDictionary = [
            "type": "redirect",
            "target": ["scheme": "https", "host": "new.example.com"],
            "status": 301,
        ]
        #expect(try json(action) == expected)
    }

    @Test("what the core sends back reads as the same action")
    func roundTrip() throws {
        let text = """
            {"type":"modify_response","status":503,
             "headers":[{"op":"add","name":"X-A","value":"1"},{"op":"remove","name":"Set-Cookie"}],
             "body":{"op":"replace","text":"{\\"down\\":true}"}}
            """

        let action = try decoded(text)

        #expect(
            action
                == .modifyResponse(
                    ModifyResponseAction(
                        status: 503,
                        headers: [HeaderEdit(.add, name: "X-A", value: "1"), HeaderEdit(.remove, name: "Set-Cookie")],
                        body: .replace("{\"down\":true}")
                    )
                )
        )
        #expect(action.kind == .modifyResponse)
        #expect(action.kind.isTransformation)
    }

    @Test("a redirect the core wrote without a status is temporary")
    func redirectDefault() throws {
        let action = try decoded(
            #"{"type":"redirect","target":{"scheme":"https","host":"new.example.com"}}"#
        )

        guard case let .redirect(redirect) = action else {
            Issue.record("not a redirect")
            return
        }
        #expect(redirect.status == 302)
    }

    @Test("the list says what a modify rule changes, and never a header's value")
    func summary() {
        let summary = RuleDisplay.actionSummary(
            .modifyRequest(
                ModifyRequestAction(headers: [HeaderEdit(.set, name: "Authorization", value: "Bearer secret")])
            )
        )

        #expect(summary == "Set Authorization")
        #expect(!summary.contains("secret"))
    }
}

@MainActor
@Suite("Writing a transformation")
struct RuleTransformationEditorTests {
    private func editor(
        _ kind: RuleActionKind,
        session: FakeRulesSession = FakeRulesSession()
    ) -> RuleEditorModel {
        let editor = RuleEditorModel(creating: kind, session: session, chooser: FakeFileChooser())
        editor.name = "a transformation"
        return editor
    }

    private func created(_ session: FakeRulesSession) async -> NewRule? {
        guard case let .create(new) = await session.calls.first else { return nil }
        return new
    }

    @Test("rows become the edits they look like, and empty rows are left out")
    func rowsBecomeEdits() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))
        let editor = editor(.modifyRequest, session: session)

        editor.modifyRequest.method = "put"
        editor.modifyRequest.pathMode = .set
        editor.modifyRequest.path = "/v2/users"
        editor.modifyRequest.query = [
            EditableQueryEdit(operation: .set, name: "env", value: "staging"),
            EditableQueryEdit(),
        ]
        editor.modifyRequest.headers = [
            EditableHeaderEdit(operation: .set, name: " Authorization ", value: "Bearer t"),
            EditableHeaderEdit(operation: .remove, name: "Cookie", value: "ignored"),
            EditableHeaderEdit(),
        ]
        editor.modifyRequest.body.mode = .setJSON
        editor.modifyRequest.body.jsonPath = "user.role"
        editor.modifyRequest.body.jsonValue = "\"admin\""

        _ = await editor.save()

        let new = try #require(await created(session))
        #expect(
            new.action
                == .modifyRequest(
                    ModifyRequestAction(
                        method: "PUT",
                        path: .set("/v2/users"),
                        query: [QueryEdit(.set, name: "env", value: "staging")],
                        headers: [
                            HeaderEdit(.set, name: "Authorization", value: "Bearer t"),
                            HeaderEdit(.remove, name: "Cookie"),
                        ],
                        body: .setJSON(path: "user.role", value: "\"admin\"")
                    )
                )
        )
    }

    @Test("a transformation that changes nothing is refused before it is sent")
    func changesNothing() async {
        let session = FakeRulesSession()
        let editor = editor(.modifyResponse, session: session)
        editor.modifyResponse.headers = [EditableHeaderEdit()]

        let saved = await editor.save()

        #expect(saved == nil)
        #expect(editor.fieldErrors[.transformation] != nil)
        #expect(await session.calls.isEmpty)
    }

    @Test("the headers the proxy keeps for itself are refused by name")
    func protectedHeader() {
        let editor = editor(.modifyRequest)
        editor.modifyRequest.headers = [EditableHeaderEdit(operation: .set, name: "Host", value: "x")]

        #expect(editor.validate()[.requestHeaders]?.contains("Host") == true)
    }

    @Test("a header value that would split the header is refused")
    func headerLineBreak() {
        let editor = editor(.modifyResponse)
        editor.modifyResponse.headers = [
            EditableHeaderEdit(operation: .add, name: "X-A", value: "1\r\nSet-Cookie: x"),
        ]

        #expect(editor.validate()[.responseHeaders] != nil)
    }

    @Test("a query edit that would smuggle a second parameter is refused")
    func querySmuggling() {
        let editor = editor(.modifyRequest)
        editor.modifyRequest.query = [EditableQueryEdit(operation: .set, name: "a", value: "1&b=2")]

        #expect(editor.validate()[.modifyQuery] != nil)
    }

    @Test("a JSON value has to be JSON")
    func jsonValue() {
        let editor = editor(.modifyResponse)
        editor.modifyResponse.body.mode = .setJSON
        editor.modifyResponse.body.jsonPath = "status"
        editor.modifyResponse.body.jsonValue = "not quoted"

        #expect(editor.validate()[.responseBody] != nil)

        editor.modifyResponse.body.jsonValue = "\"quoted\""
        #expect(editor.validate()[.responseBody] == nil)
    }

    @Test("a response status is a final one")
    func responseStatus() {
        let editor = editor(.modifyResponse)
        editor.modifyResponse.status = "101"

        #expect(editor.validate()[.responseStatus] != nil)
    }

    @Test("a redirect is created with its location and status")
    func redirect() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(with: .success(RuleFixtures.rule(id: "new")))
        let editor = editor(.redirect, session: session)
        editor.redirectURL = "https://new.example.com/moved"
        editor.redirectStatus = 308

        _ = await editor.save()

        let new = try #require(await created(session))
        #expect(
            new.action
                == .redirect(
                    RedirectAction(
                        target: RemoteTarget(scheme: .https, host: "new.example.com", path: "/moved"),
                        status: 308
                    )
                )
        )
    }

    @Test("a redirect with nowhere to go says so under the location")
    func redirectEmpty() {
        let editor = editor(.redirect)

        #expect(editor.validate()[.redirectURL] == "A redirect must name where to send the client.")
    }

    @Test("an existing transformation opens with every edit in it, and sends only what changed")
    func editing() async throws {
        let rule = RuleFixtures.rule(
            id: "r",
            action: .modifyRequest(
                ModifyRequestAction(
                    path: .replacePrefix(from: "/v1/", to: "/v2/"),
                    headers: [HeaderEdit(.set, name: "X-Env", value: "staging")]
                )
            )
        )
        let session = FakeRulesSession()
        await session.answerUpdate(with: .success(rule))
        let editor = RuleEditorModel(editing: rule, session: session, chooser: FakeFileChooser())

        #expect(editor.modifyRequest.pathMode == .replacePrefix)
        #expect(editor.modifyRequest.pathFrom == "/v1/")
        #expect(editor.modifyRequest.headers.map(\.name) == ["X-Env"])
        #expect(!editor.hasChanges)

        editor.modifyRequest.headers[0].value = "production"
        _ = await editor.save()

        let update = try #require(await session.updates().first?.update)
        #expect(update.name == nil)
        #expect(update.matcher == nil)
        guard case let .modifyRequest(sent) = update.action else {
            Issue.record("expected the action to be sent")
            return
        }
        #expect(sent.headers == [HeaderEdit(.set, name: "X-Env", value: "production")])
    }

    @Test("a refusal from the core about a header goes under the headers")
    func coreRefusal() async {
        let session = FakeRulesSession()
        await session.answerCreate(
            with: .failure(RuleFixtures.refusal("a modify header value must not contain line breaks"))
        )
        let editor = editor(.modifyResponse, session: session)
        editor.modifyResponse.status = "404"

        _ = await editor.save()

        #expect(editor.fieldErrors[.responseHeaders] == "A modify header value must not contain line breaks.")
    }
}
