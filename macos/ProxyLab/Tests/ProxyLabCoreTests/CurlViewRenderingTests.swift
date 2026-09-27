//  The import sheet, actually drawn.
//
//  A SwiftUI view that compiles is not a SwiftUI view that lays out: a body
//  that traps, a binding read while it is being written, or a frame that
//  resolves to nothing all happen when the view is rendered. `ImageRenderer`
//  runs a body and lays it out with no window, no run loop, and nobody
//  watching, which is what makes that testable here.
//
//  Smoke tests, deliberately not snapshot comparisons: what is asserted is
//  that each face of the sheet draws something at a sensible size, not that it
//  draws the same pixels as last time.

import SwiftUI
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("cURL views")
struct CurlViewRenderingTests {
    /// Lays `view` out offscreen and gives back the size it came out at.
    private func rendered(_ view: some View, width: CGFloat = 560) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    private func model() async -> TrafficInspectorModel {
        let model = TrafficInspectorModel(
            session: FakeSession(),
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()

        return model
    }

    @Test("the paste face draws, empty and with a command in it")
    func pasteDraws() async throws {
        let model = await model()
        model.beginImport()
        let importer = try #require(model.importer)

        let empty = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(empty.width > 0 && empty.height > 0)

        importer.curlText = "curl 'https://example.com/api' \\\n  -H 'Accept: application/json'"
        let filled = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(filled.width > 0 && filled.height > 0)
    }

    @Test("the paste face draws the reason a command was refused")
    func problemDraws() async throws {
        let session = FakeSession()
        await session.answerParses(with: [
            .success(
                CurlParseResult(
                    request: nil,
                    problem: CurlProblem(
                        code: .shellExpansion,
                        message: "this command asks the shell to substitute something",
                        option: nil
                    )
                ))
        ])
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        model.beginImport()
        let importer = try #require(model.importer)
        importer.curlText = "curl https://example.com/$(whoami)"
        await importer.parse()

        let size = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(size.height > 0)
    }

    @Test("the editor draws, with headers, a body, and the notes about them")
    func editorDraws() async throws {
        let model = await model()
        model.beginImport()
        let importer = try #require(model.importer)

        importer.show(
            ReplayRequest(
                method: "POST",
                url: "https://api.example.com/v1/users?active=true",
                headers: Headers([
                    Header(name: "Content-Type", value: .text("application/json")),
                    Header(name: "Authorization", value: .text("Bearer TEST_TOKEN")),
                    Header(name: "X-Trace", value: .text("one")),
                    Header(name: "X-Trace", value: .text("two")),
                ]),
                body: Data(#"{"name":"Ada Lovelace"}"#.utf8)
            ),
            notes: [.compressed, .insecure]
        )

        let size = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(size.width > 0 && size.height > 0)
    }

    @Test("the editor draws a body and a header that are not text")
    func binaryDraws() async throws {
        let model = await model()
        model.beginImport()
        let importer = try #require(model.importer)

        importer.show(
            ReplayRequest(
                method: "PUT",
                url: "https://example.com/blob",
                headers: Headers([Header(name: "X-Raw", value: .bytes(Data([0xff, 0x00])))]),
                body: Data((0...255).map(UInt8.init))
            ))

        let size = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(size.height > 0)
    }

    @Test("the editor draws an empty request, with no headers at all")
    func emptyEditorDraws() async throws {
        let model = await model()
        model.beginImport()
        let importer = try #require(model.importer)
        importer.show(ReplayRequest(method: "GET", url: ""))

        let size = try #require(rendered(CurlImportView(importer: importer, model: model)))
        #expect(size.height > 0)
    }

    @Test("the traffic detail draws with the cURL actions in it")
    func detailDraws() async throws {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "a")]))])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")

        let size = try #require(rendered(TrafficDetailView(model: model), width: 520))
        #expect(size.width > 0 && size.height > 0)
    }
}
