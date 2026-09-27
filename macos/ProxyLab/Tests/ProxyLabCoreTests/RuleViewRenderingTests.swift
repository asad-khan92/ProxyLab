//  The rules views, actually drawn.
//
//  A SwiftUI view that compiles is not a SwiftUI view that lays out: a body
//  that traps, a binding that is read while it is being written, or a frame
//  that resolves to nothing all happen when the view is rendered and not when
//  it is built. `ImageRenderer` runs a body and lays it out with no window, no
//  run loop, and nobody watching, which is what makes that testable here.
//
//  These are smoke tests and deliberately not snapshot comparisons: what is
//  asserted is that each view draws something at a sensible size, not that it
//  draws the same pixels as last time. A test that fails because a font
//  shipped with a new macOS is a test that gets deleted.
//
//  `NavigationSplitView` is not one `ImageRenderer` can draw, so the screens
//  are rendered as the pieces they are built from — which is where the rule
//  drawing actually lives.

import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Rule views")
struct RuleViewRenderingTests {
    /// Lays `view` out offscreen and gives back the size it came out at.
    private func rendered(_ view: some View, width: CGFloat = 420) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    private func model(_ rules: [Rule]) async -> RulesModel {
        let session = FakeRulesSession()
        await session.answerList(with: rules)
        let model = RulesModel(session: session, chooser: FakeFileChooser())
        await model.load()

        return model
    }

    @Test("a rule row draws, whatever the rule does")
    func rowDraws() async throws {
        let rules = [
            RuleFixtures.rule(
                id: "a",
                name: "block analytics",
                priority: 10,
                matcher: RuleMatcher(
                    host: .exact("analytics.example.com"),
                    path: .prefix("/collect"),
                    method: .exact("POST")
                ),
                action: .block(BlockAction(status: 403))
            ),
            RuleFixtures.rule(
                id: "b",
                name: "mock the config",
                enabled: false,
                priority: 20,
                action: .mapLocal(MapLocalAction(path: "/Users/dev/mocks/config.json"))
            ),
            RuleFixtures.rule(
                id: "c",
                name: "point at staging",
                priority: 30,
                action: .mapRemote(RemoteTarget(scheme: .https, host: "staging.example.com"))
            ),
            RuleFixtures.rule(id: "d", name: "except health", action: .allow),
        ]
        let model = await model(rules)

        for rule in rules {
            let size = try #require(rendered(RuleRow(rule: rule, model: model)), "\(rule.name)")

            #expect(size.width > 0 && size.height > 0, "\(rule.name)")
        }
    }

    @Test("a rule with no criteria set draws, and says it matches everything")
    func anyMatcherRowDraws() async throws {
        let rule = RuleFixtures.rule(id: "a", name: "block everything", matcher: .any)
        let model = await model([rule])

        #expect(rendered(RuleRow(rule: rule, model: model)) != nil)
    }

    @Test("the detail pane draws every kind of rule")
    func detailDraws() async throws {
        let rules = [
            RuleFixtures.rule(id: "a", action: .block(BlockAction(status: 451))),
            RuleFixtures.rule(
                id: "b",
                action: .mapLocal(
                    MapLocalAction(
                        path: "/Users/dev/mocks/config.json",
                        contentType: "application/json",
                        status: 200
                    )
                )
            ),
            RuleFixtures.rule(
                id: "c",
                matcher: RuleMatcher(host: .exact("api.example.com")),
                action: .mapRemote(
                    RemoteTarget(
                        scheme: .https,
                        host: "staging.example.com",
                        port: 8443,
                        path: "/v2",
                        query: "debug=1"
                    )
                )
            ),
            RuleFixtures.rule(id: "d", action: .allow),
        ]
        let model = await model(rules)

        for rule in rules {
            model.selectedID = rule.id

            #expect(rendered(RuleDetailView(model: model), width: 640) != nil, "\(rule.id)")
        }
    }

    @Test("the detail pane draws when nothing is selected")
    func detailPlaceholderDraws() async throws {
        let model = await model(RuleFixtures.ordered())
        model.selectedID = nil

        #expect(rendered(RuleDetailView(model: model), width: 640) != nil)
    }

    @Test("the editor draws for every action it can write")
    func editorDraws() async throws {
        let model = await model(RuleFixtures.ordered())

        for kind in RuleActionKind.allCases {
            model.beginCreating(kind)
            let editor = try #require(model.editor)

            #expect(
                rendered(RuleEditorView(editor: editor, model: model), width: 520) != nil,
                "\(kind.title)"
            )
        }
    }

    @Test("the editor draws with a rule's values in it")
    func editorDrawsAnExistingRule() async throws {
        let model = await model(RuleFixtures.ordered())
        model.beginEditing(model.rules[0])
        let editor = try #require(model.editor)

        #expect(rendered(RuleEditorView(editor: editor, model: model), width: 520) != nil)
    }

    @Test("the editor draws with a refusal under a field")
    func editorDrawsARefusal() async throws {
        let session = FakeRulesSession()
        await session.answerCreate(
            with: .failure(RuleFixtures.refusal("a block status must be between 100 and 599"))
        )
        let model = RulesModel(session: session, chooser: FakeFileChooser())
        model.beginCreating(.block)
        model.editor?.name = "a name"
        _ = await model.saveEditor()

        let editor = try #require(model.editor)
        #expect(editor.fieldErrors[.blockStatus] != nil)
        #expect(rendered(RuleEditorView(editor: editor, model: model), width: 520) != nil)
    }

    @Test("the editor draws when the rule was removed from under it")
    func editorDrawsWhenTheRuleIsGone() async throws {
        let session = FakeRulesSession()
        await session.answerUpdate(with: .failure(RuleFixtures.notFound))
        let model = RulesModel(session: session, chooser: FakeFileChooser())
        model.beginEditing(RuleFixtures.rule(id: "a", name: "old"))
        model.editor?.name = "new"
        _ = await model.saveEditor()

        let editor = try #require(model.editor)
        #expect(editor.isGone)
        #expect(rendered(RuleEditorView(editor: editor, model: model), width: 520) != nil)
    }

    @Test("the editor draws a transformation with every kind of row filled in")
    func editorDrawsATransformation() async throws {
        let model = await model(RuleFixtures.ordered())
        model.beginCreating(.modifyRequest)
        let editor = try #require(model.editor)
        editor.modifyRequest.pathMode = .replacePrefix
        editor.modifyRequest.query = [EditableQueryEdit(name: "env", value: "staging")]
        editor.modifyRequest.headers = [
            EditableHeaderEdit(operation: .set, name: "Authorization", value: "Bearer t"),
            EditableHeaderEdit(operation: .remove, name: "Cookie"),
        ]
        editor.modifyRequest.body.mode = .setJSON
        _ = editor.validate()

        #expect(rendered(RuleEditorView(editor: editor, model: model), width: 520) != nil)

        model.beginCreating(.modifyResponse)
        let response = try #require(model.editor)
        response.modifyResponse.status = "404"
        response.modifyResponse.body.mode = .replace
        response.modifyResponse.body.text = "{}"

        #expect(rendered(RuleEditorView(editor: response, model: model), width: 520) != nil)
    }

    @Test("the detail pane draws each new kind of rule")
    func detailDrawsNewKinds() async throws {
        let rules = [
            RuleFixtures.rule(
                id: "r1",
                action: .redirect(
                    RedirectAction(target: RemoteTarget(scheme: .https, host: "new.example.com"))
                )
            ),
            RuleFixtures.rule(
                id: "r2",
                action: .modifyRequest(
                    ModifyRequestAction(method: "POST", headers: [HeaderEdit(.set, name: "X-A", value: "1")])
                )
            ),
            RuleFixtures.rule(
                id: "r3",
                action: .modifyResponse(ModifyResponseAction(status: 404, body: .replace("{}")))
            ),
        ]
        let model = await model(rules)

        for rule in rules {
            model.selectedID = rule.id
            #expect(rendered(RuleDetailView(model: model), width: 640) != nil, "\(rule.id)")
            #expect(rendered(RuleRow(rule: rule, model: model)) != nil)
        }
    }

    @Test("the section control draws for either section")
    func sectionPickerDraws() {
        for section in AppSection.allCases {
            #expect(rendered(SectionPicker(section: .constant(section)), width: 200) != nil)
        }
    }
}
