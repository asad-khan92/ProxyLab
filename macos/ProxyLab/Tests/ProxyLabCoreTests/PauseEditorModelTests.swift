//  The fields a person edits while one exchange is held, and the edit they
//  come to.
//
//  Two things are worth being sure of. The first is that an untouched field is
//  left out of the edit: the core leaves a field it was not sent exactly as it
//  was, so "I only changed the status" has to mean only the status is sent.
//  The second is that a body which cannot honestly be edited — one the core
//  truncated, or one that is not text — is refused here rather than being
//  replaced with a lossy decoding of itself.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Pause editor")
struct PauseEditorModelTests {
    private func request(
        body: Data = Data("{\"id\":1}".utf8),
        size: UInt64? = nil,
        truncated: Bool = false,
        headers: Headers = Headers([Header(name: "Accept", value: .text("application/json"))])
    ) -> PausedExchange {
        PausedExchange(
            pauseID: "pause-1",
            breakpointID: "bp-1",
            direction: .request,
            method: "POST",
            url: "https://api.example.com/orders",
            request: PausedRequest(
                method: "POST",
                url: "https://api.example.com/orders",
                headers: headers,
                body: BodyCapture(
                    data: body,
                    size: size ?? UInt64(body.count),
                    truncated: truncated
                )
            )
        )
    }

    private func response(status: UInt16 = 200) -> PausedExchange {
        PausedExchange(
            pauseID: "pause-2",
            breakpointID: "bp-2",
            direction: .response,
            method: "GET",
            url: "https://api.example.com/orders",
            response: PausedResponse(
                status: status,
                headers: Headers([Header(name: "Content-Type", value: .text("text/plain"))]),
                body: BodyCapture.complete(Data("ok".utf8))
            )
        )
    }

    // MARK: - What it starts with

    @Test("the editor starts holding exactly what the core sent")
    func startsWithWhatArrived() {
        let editor = PauseEditorModel(request())

        #expect(editor.method == "POST")
        #expect(editor.url == "https://api.example.com/orders")
        #expect(editor.headerText == "Accept: application/json")
        #expect(editor.bodyText == "{\"id\":1}")
        #expect(editor.isBodyEditable)
        #expect(editor.refusal == nil)
    }

    @Test("a response editor starts on the status the origin sent")
    func startsWithTheStatus() {
        let editor = PauseEditorModel(response(status: 201))

        #expect(editor.status == "201")
        #expect(editor.direction == .response)
    }

    // MARK: - What is sent

    @Test("changing nothing produces an edit that changes nothing")
    func untouchedSendsNothing() {
        let editor = PauseEditorModel(request())

        let edits = editor.edits()

        #expect(edits?.request == nil)
        #expect(edits?.response == nil)
    }

    @Test("only what was changed is in the edit")
    func onlyChangedFields() {
        let editor = PauseEditorModel(request())
        editor.url = "https://api.example.com/v2/orders"

        let edit = editor.edits()?.request

        #expect(edit?.url == "https://api.example.com/v2/orders")
        #expect(edit?.method == nil)
        #expect(edit?.headers == nil)
        #expect(edit?.body == nil)
    }

    @Test("an edited header section replaces the whole section")
    func editedHeaders() {
        let editor = PauseEditorModel(request())
        editor.headerText = "Accept: text/plain\nX-Edited: yes"

        let headers = editor.edits()?.request?.headers

        #expect(headers?.count == 2)
        #expect(headers?.text("x-edited") == "yes")
        #expect(headers?.text("accept") == "text/plain")
    }

    @Test("a header line that is not one is refused before anything is sent")
    func malformedHeaders() {
        let editor = PauseEditorModel(request())
        editor.headerText = "this is not a header"

        #expect(editor.refusal?.contains("Name: value") == true)
        #expect(editor.edits() == nil)
    }

    @Test("blank lines between headers are not headers")
    func blankHeaderLines() {
        let editor = PauseEditorModel(request())
        editor.headerText = "Accept: text/plain\n\n   \nX-Edited: yes"

        #expect(editor.refusal == nil)
        #expect(editor.edits()?.request?.headers?.count == 2)
    }

    @Test("an edited body is sent as the bytes it is")
    func editedBody() {
        let editor = PauseEditorModel(request())
        editor.bodyText = "{\"id\":2}"

        #expect(editor.edits()?.request?.body == Data("{\"id\":2}".utf8))
    }

    @Test("an edited status is sent, and an unchanged one is not")
    func editedStatus() {
        let editor = PauseEditorModel(response(status: 200))
        #expect(editor.edits()?.response == nil)

        editor.status = "503"
        #expect(editor.edits()?.response?.status == 503)
    }

    // MARK: - What it refuses

    @Test("a status that is not one is refused")
    func badStatus() {
        for status in ["", "99", "600", "two hundred"] {
            let editor = PauseEditorModel(response())
            editor.status = status

            #expect(editor.refusal?.contains("100 to 599") == true, "\(status)")
            #expect(editor.edits() == nil)
        }
    }

    @Test("a request with no method or no URL is refused")
    func emptyRequestLine() {
        let editor = PauseEditorModel(request())
        editor.method = "  "
        #expect(editor.refusal?.contains("method") == true)

        editor.method = "POST"
        editor.url = ""
        #expect(editor.refusal?.contains("URL") == true)
    }

    @Test("a body the core truncated cannot be edited and is sent as it was")
    func truncatedBody() {
        let editor = PauseEditorModel(
            request(body: Data(repeating: 0x78, count: 16), size: 99_999, truncated: true)
        )

        #expect(!editor.isBodyEditable)
        #expect(editor.bodyRefusal?.contains("longer than ProxyLab shows") == true)

        // Even with text in the field, nothing is sent for it.
        editor.bodyText = "short"
        #expect(editor.edits()?.request?.body == nil)
    }

    @Test("a body that is not text cannot be edited")
    func binaryBody() {
        let editor = PauseEditorModel(request(body: Data([0xFF, 0xFE, 0x00, 0x01])))

        #expect(!editor.isBodyEditable)
        #expect(editor.bodyRefusal?.contains("not text") == true)
    }

    @Test("an empty body is editable, because there is nothing to lose")
    func emptyBody() {
        let editor = PauseEditorModel(request(body: Data()))

        #expect(editor.isBodyEditable)
        #expect(editor.bodyRefusal == nil)

        editor.bodyText = "now it has one"
        #expect(editor.edits()?.request?.body == Data("now it has one".utf8))
    }

    @Test("a query edit is a change to the URL, and the query is not its own edit")
    func queryEditsTheURL() {
        let editor = PauseEditorModel(
            request(body: Data("{}".utf8))
        )
        editor.url = "https://api.example.com/orders?since=1"

        #expect(PauseEditorModel.query(in: editor.url) == "since=1")
        editor.url = PauseEditorModel.url(editor.url, replacingQuery: "since=2")

        #expect(editor.url == "https://api.example.com/orders?since=2")
        #expect(editor.edits()?.request?.url == "https://api.example.com/orders?since=2")
        #expect(editor.edits()?.request?.method == nil)
        #expect(editor.edits()?.request?.body == nil)
    }

    @Test("a body longer than the core will take is refused before it is sent")
    func oversizedBody() {
        let editor = PauseEditorModel(request())
        editor.bodyText = String(repeating: "x", count: maxBreakpointBodyBytes + 1)

        #expect(editor.refusal?.contains("at most") == true)
        #expect(editor.edits() == nil)
    }

    @Test("a header value that is not text is left out rather than typed back as nonsense")
    func binaryHeaderValue() {
        let editor = PauseEditorModel(
            request(
                headers: Headers([
                    Header(name: "Accept", value: .text("application/json")),
                    Header(name: "X-Binary", value: .bytes(Data([0xFF, 0x00]))),
                ])
            )
        )

        #expect(editor.headerText == "Accept: application/json")
    }

    private func websocket(
        payload: Data = Data("{\"id\":1}".utf8),
        truncated: Bool = false
    ) -> PausedExchange {
        PausedExchange(
            pauseID: "pause-ws",
            breakpointID: "bp-ws",
            direction: .websocket,
            method: "GET",
            url: "http://example.com/socket",
            websocket: PausedWebSocketMessage(
                direction: .clientToServer,
                payload: BodyCapture(
                    data: payload,
                    size: UInt64(payload.count),
                    truncated: truncated
                )
            )
        )
    }

    @Test("a WebSocket editor starts holding the payload the core sent")
    func websocketStartsWithPayload() {
        let editor = PauseEditorModel(websocket())

        #expect(editor.direction == .websocket)
        #expect(editor.bodyText == "{\"id\":1}")
        #expect(editor.isBodyEditable)
        #expect(editor.edits()?.websocket == nil)
        #expect(editor.edits()?.request == nil)
        #expect(editor.edits()?.response == nil)
    }

    @Test("editing a text WebSocket payload is the only field that is sent")
    func websocketPayloadEdit() {
        let editor = PauseEditorModel(websocket())
        editor.bodyText = "{\"id\":2}"

        #expect(editor.edits()?.websocket?.payload == Data("{\"id\":2}".utf8))
        #expect(editor.edits()?.request == nil)
        #expect(editor.edits()?.response == nil)
    }

    @Test("a WebSocket pause does not demand HTTP headers")
    func websocketIgnoresHeaders() {
        let editor = PauseEditorModel(websocket())
        editor.headerText = "this is not a header"

        #expect(editor.refusal == nil)
        #expect(editor.edits()?.websocket == nil)
    }
}
