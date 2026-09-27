//  The import sheet's behaviour, without a sheet.
//
//  What is asserted is what the model does with an answer from the core — it
//  moves to the editor or it shows a reason, it builds the request the fields
//  describe, it keeps a body that is not text as the bytes it is. Whether a
//  cURL command is a cURL command is the core's to say, and the core is stood
//  in for here, because the app never decides that.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("cURL import")
struct CurlImportModelTests {
    /// A model over a core that answers whatever the test set.
    private func model(_ session: FakeSession = FakeSession()) -> CurlImportModel {
        CurlImportModel(session: session)
    }

    /// A parsed request, as the core would send one back.
    private func parsed(
        method: String = "GET",
        url: String = "https://example.com/api",
        headers: [(String, String)] = [],
        body: Data = Data(),
        notes: [CurlNote] = []
    ) -> CurlParseResult {
        CurlParseResult(
            request: ReplayRequest(
                method: method,
                url: url,
                headers: Headers(headers.map { Header(name: $0.0, value: .text($0.1)) }),
                body: body
            ),
            notes: notes
        )
    }

    // MARK: - Reading a command

    @Test("the sheet starts on the command, with nothing to send")
    func startsPasting() {
        let importer = model()

        #expect(importer.stage == .pasting)
        #expect(!importer.canParse)
        #expect(!importer.canReplay)
        #expect(importer.problem == nil)
    }

    @Test("a blank command is not something to import")
    func blank() {
        let importer = model()

        importer.curlText = "   \n  "
        #expect(!importer.canParse)

        importer.curlText = "curl https://example.com/"
        #expect(importer.canParse)
    }

    @Test("a command the core reads fills the editor in")
    func fillsIn() async {
        let session = FakeSession()
        await session.answerParses(with: [
            .success(
                parsed(
                    method: "POST",
                    url: "https://example.com/api",
                    headers: [
                        ("Content-Type", "application/json"),
                        ("Authorization", "Bearer TEST_TOKEN"),
                    ],
                    body: Data(#"{"name":"Ada"}"#.utf8),
                    notes: [.compressed]
                ))
        ])
        let importer = model(session)
        importer.curlText = "curl ..."

        await importer.parse()

        #expect(importer.stage == .editing)
        #expect(importer.method == "POST")
        #expect(importer.url == "https://example.com/api")
        #expect(importer.headers.map(\.name) == ["Content-Type", "Authorization"])
        #expect(importer.headers.map(\.value) == ["application/json", "Bearer TEST_TOKEN"])
        #expect(importer.bodyText == #"{"name":"Ada"}"#)
        #expect(importer.notes == [.compressed])
        #expect(importer.problem == nil)
        #expect(importer.canReplay)
    }

    @Test("the command is sent to the core exactly as it was pasted")
    func sendsTheCommand() async {
        let session = FakeSession()
        await session.answerParses(with: [.success(parsed())])
        let importer = model(session)
        let command = "curl 'https://example.com/api' \\\n  -H 'Accept: */*'"
        importer.curlText = command

        await importer.parse()

        #expect(await session.parsedCommands == [command])
    }

    @Test("a command that describes no request says why, and stays on the command")
    func problem() async {
        let session = FakeSession()
        await session.answerParses(with: [
            .success(
                CurlParseResult(
                    request: nil,
                    problem: CurlProblem(
                        code: .unsupportedOption,
                        message: "ProxyLab does not implement this cURL option (--foo)",
                        option: "--foo"
                    )
                ))
        ])
        let importer = model(session)
        importer.curlText = "curl --foo https://example.com/"

        await importer.parse()

        #expect(importer.stage == .pasting)
        #expect(importer.problem == "ProxyLab does not implement this cURL option (--foo)")
        #expect(!importer.canReplay)
        // The command is still there to be fixed.
        #expect(importer.curlText == "curl --foo https://example.com/")
    }

    @Test("a core that cannot be reached is reported as one")
    func parseFails() async {
        let session = FakeSession()
        await session.answerParses(with: [.failure(.notConnected)])
        let importer = model(session)
        importer.curlText = "curl https://example.com/"

        await importer.parse()

        #expect(importer.stage == .pasting)
        #expect(importer.problem == IPCError.notConnected.message)
    }

    @Test("going back to the command keeps it as it was typed")
    func backToCommand() async {
        let session = FakeSession()
        await session.answerParses(with: [.success(parsed())])
        let importer = model(session)
        importer.curlText = "curl https://example.com/api"

        await importer.parse()
        #expect(importer.stage == .editing)

        importer.backToCommand()

        #expect(importer.stage == .pasting)
        #expect(importer.curlText == "curl https://example.com/api")
    }

    // MARK: - Editing

    @Test("a header can be added, changed, and removed")
    func editHeaders() async {
        let session = FakeSession()
        await session.answerParses(with: [
            .success(parsed(headers: [("Accept", "*/*"), ("X-Trace", "one")]))
        ])
        let importer = model(session)
        importer.curlText = "curl ..."
        await importer.parse()

        importer.addHeader()
        #expect(importer.headers.count == 3)
        importer.headers[2].name = "X-New"
        importer.headers[2].value = "yes"

        importer.headers[0].value = "application/json"
        importer.removeHeader(importer.headers[1].id)

        let headers = importer.request.headers
        #expect(headers.map(\.name) == ["Accept", "X-New"])
        #expect(headers.text("accept") == "application/json")
        #expect(headers.text("x-new") == "yes")
    }

    @Test("a header row with no name is not a header")
    func blankHeaderRow() {
        let importer = model()
        importer.show(ReplayRequest(method: "GET", url: "https://example.com/"))

        importer.addHeader()
        importer.addHeader()
        importer.headers[1].name = "  Accept  "
        importer.headers[1].value = "*/*"

        let headers = importer.request.headers
        #expect(headers.count == 1)
        // The name is trimmed, because a header name with spaces around it is
        // one the core would refuse and one nobody meant to type.
        #expect(headers[0].name == "Accept")
    }

    @Test("duplicate headers stay duplicates, in the order they are shown")
    func duplicateHeaders() {
        let importer = model()
        importer.show(
            ReplayRequest(
                method: "GET",
                url: "https://example.com/",
                headers: Headers([
                    Header(name: "X-Trace", value: .text("one")),
                    Header(name: "X-Trace", value: .text("two")),
                ])
            ))

        #expect(importer.headers.count == 2)
        #expect(importer.request.headers.map(\.name) == ["X-Trace", "X-Trace"])
        #expect(
            importer.request.headers.compactMap(\.value.text) == ["one", "two"]
        )
    }

    @Test("a header value that is not text is kept and not editable")
    func binaryHeader() {
        let bytes = Data([0xc3, 0x28, 0xff])
        let importer = model()
        importer.show(
            ReplayRequest(
                method: "GET",
                url: "https://example.com/",
                headers: Headers([Header(name: "X-Binary", value: .bytes(bytes))])
            ))

        #expect(importer.headers[0].isBinary)
        #expect(importer.headers[0].bytes == bytes)
        #expect(importer.request.headers[0].value == .bytes(bytes))
    }

    @Test("the method is whatever is typed, not a list the app decides")
    func anyMethod() {
        let importer = model()
        importer.show(ReplayRequest(method: "GET", url: "https://example.com/"))

        importer.method = "PURGE"

        #expect(importer.canReplay)
        #expect(importer.request.method == "PURGE")
        // The common ones are offered, and are a convenience.
        #expect(CurlImportModel.commonMethods.contains("PATCH"))
    }

    @Test("there is nothing to send without a URL or a method")
    func needsBoth() {
        let importer = model()
        importer.show(ReplayRequest(method: "GET", url: "https://example.com/"))
        #expect(importer.canReplay)

        importer.url = "   "
        #expect(!importer.canReplay)

        importer.url = "https://example.com/"
        importer.method = ""
        #expect(!importer.canReplay)
    }

    // MARK: - Bodies

    @Test("a text body is shown and edited as text")
    func textBody() {
        let importer = model()
        importer.show(
            ReplayRequest(method: "POST", url: "https://example.com/", body: Data("hello".utf8))
        )

        #expect(importer.isBodyEditable)
        #expect(importer.bodyText == "hello")

        importer.bodyText = "goodbye"
        #expect(importer.request.body == Data("goodbye".utf8))
    }

    @Test("a body that is not text is kept exactly and cannot be edited")
    func binaryBody() {
        let bytes = Data([0x00, 0x01, 0xff, 0xfe])
        let importer = model()
        importer.show(ReplayRequest(method: "PUT", url: "https://example.com/", body: bytes))

        #expect(!importer.isBodyEditable)
        #expect(importer.binaryBody == bytes)
        #expect(importer.bodyText.isEmpty)
        #expect(importer.binaryBodyDescription?.contains("4 bytes") == true)
        // Sent unchanged, which is the whole point of not offering an edit.
        #expect(importer.request.body == bytes)
    }

    @Test("bytes are only let go of when somebody asks")
    func replaceBinaryBody() {
        let importer = model()
        importer.show(
            ReplayRequest(method: "PUT", url: "https://example.com/", body: Data([0xff, 0x00]))
        )

        importer.makeBodyEditable()

        #expect(importer.isBodyEditable)
        #expect(importer.binaryBody == nil)
        #expect(importer.request.body.isEmpty)
    }

    // MARK: - Sending it

    @Test("replaying sends the request the editor describes")
    func replays() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .success(ReplayResult(exchangeID: "new", status: 201, error: nil))
        )
        let importer = model(session)
        importer.show(
            ReplayRequest(
                method: "POST",
                url: "https://example.com/api",
                headers: Headers([Header(name: "Authorization", value: .text("Bearer TEST_TOKEN"))]),
                body: Data(#"{"a":1}"#.utf8)
            ))
        importer.url = "https://example.com/api/v2"

        let result = await importer.replay()

        #expect(result?.exchangeID == "new")
        #expect(result?.status == 201)
        #expect(importer.replayState == .idle)

        let sent = try? #require(await session.replayed.first)
        #expect(sent?.method == "POST")
        #expect(sent?.url == "https://example.com/api/v2")
        // The credential goes exactly as it stands. Nothing refreshes it and
        // nothing logs it.
        #expect(sent?.headers.text("authorization") == "Bearer TEST_TOKEN")
        #expect(sent?.body == Data(#"{"a":1}"#.utf8))
    }

    @Test("a request the core will not send leaves the editor as it was")
    func refused() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .failure(
                .server(
                    IPCErrorBody(
                        code: .invalidRequest,
                        message: "ProxyLab sends http:// and https:// requests"
                    )))
        )
        let importer = model(session)
        importer.show(ReplayRequest(method: "GET", url: "ftp://example.com/"))

        let result = await importer.replay()

        #expect(result == nil)
        #expect(importer.replayState == .failed("ProxyLab sends http:// and https:// requests"))
        // Still there, so it can be fixed rather than typed again.
        #expect(importer.url == "ftp://example.com/")
        #expect(importer.stage == .editing)
    }

    @Test("a replay that was sent and failed is a result, not a refusal")
    func sentAndFailed() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .success(
                ReplayResult(
                    exchangeID: "new",
                    status: nil,
                    error: TrafficError(
                        kind: .upstreamConnect,
                        summary: "could not connect to the destination server"
                    )
                ))
        )
        let importer = model(session)
        importer.show(ReplayRequest(method: "GET", url: "http://nowhere.test/"))

        let result = await importer.replay()

        #expect(result?.exchangeID == "new")
        #expect(result?.succeeded == false)
        #expect(importer.replayState == .idle)
    }

    @Test("nothing is sent from the paste stage")
    func nothingSentWhilePasting() async {
        let session = FakeSession()
        let importer = model(session)
        importer.curlText = "curl https://example.com/"

        let result = await importer.replay()

        #expect(result == nil)
        #expect(await session.replayed.isEmpty)
    }
}

@MainActor
@Suite("What the core will read")
struct CurlImportLimitTests {
    private func importer() -> CurlImportModel {
        CurlImportModel(session: FakeSession())
    }

    @Test("a request the core could not read is refused before it is sent")
    func tooLargeToSend() async {
        let session = FakeSession()
        let model = CurlImportModel(session: session)
        // Comfortably past 64 KiB once it is base64, which is what the line
        // would carry.
        model.show(
            ReplayRequest(
                method: "POST",
                url: "https://example.com/upload",
                body: Data(repeating: 0x41, count: 100_000)
            ))

        #expect(model.isTooLargeToSend)
        #expect(!model.canReplay)

        let result = await model.replay()

        #expect(result == nil)
        #expect(model.replayState == .failed(CurlImportModel.tooLargeMessage))
        // Nothing reached the core, because an oversized line is answered
        // with no request id and would never be correlated.
        #expect(await session.replayed.isEmpty)
    }

    @Test("a command the core could not read is refused before it is sent")
    func tooLargeToParse() async {
        let session = FakeSession()
        let model = CurlImportModel(session: session)
        model.curlText = "curl 'https://example.com/' -d '" + String(repeating: "a", count: 70_000) + "'"

        #expect(model.isCommandTooLarge)
        #expect(!model.canParse)

        await model.parse()

        #expect(model.problem == CurlImportModel.tooLargeMessage)
        #expect(await session.parsedCommands.isEmpty)
    }

    @Test("an ordinary request is nowhere near the limit")
    func ordinaryFits() {
        let model = importer()
        model.show(
            ReplayRequest(
                method: "POST",
                url: "https://api.example.com/v1/users",
                headers: Headers([
                    Header(name: "Authorization", value: .text("Bearer TEST_TOKEN")),
                    Header(name: "Content-Type", value: .text("application/json")),
                ]),
                body: Data(#"{"name":"Ada Lovelace"}"#.utf8)
            ))

        #expect(!model.isTooLargeToSend)
        #expect(model.canReplay)
    }

    @Test("the limit is measured on the encoded request, not guessed at")
    func measured() {
        // A body just under the budget goes; the same body with enough added
        // to cross it does not. Nothing here assumes a particular overhead.
        let model = importer()
        let request = { (count: Int) in
            ReplayRequest(
                method: "POST",
                url: "https://example.com/u",
                body: Data(repeating: 0x41, count: count)
            )
        }

        model.show(request(40_000))
        #expect(model.canReplay)

        model.show(request(60_000))
        #expect(!model.canReplay)
    }
}
