//  Copy as cURL, and replaying, from the traffic screen.
//
//  What is asserted is what the traffic model does around those two: what
//  reaches the clipboard, what reaches the core, and — the part that is easy
//  to get wrong — what happens to the list and the detail pane afterwards.
//
//  A replay is deliberately *not* selected when it comes back. The new
//  exchange arrives on the traffic subscription like any other and takes its
//  place in the list; selecting it here would mean asking for an exchange the
//  core's storage worker may not have written yet. These tests are where that
//  decision is written down.

import AppKit
import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("cURL from traffic")
struct CurlTrafficTests {
    /// A connected model over a core that answers whatever the test set.
    private func started(
        _ session: FakeSession,
        clipboard: FakeClipboard
    ) async -> TrafficInspectorModel {
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: clipboard,
            reconnect: .manual
        )
        await model.start()

        return model
    }

    /// A summary for `id`, so the list has a row to select.
    private func summary(_ id: String) -> ExchangeSummary {
        Fixtures.summary(id: id)
    }

    // MARK: - Copying

    @Test("copying puts the core's own command on the clipboard")
    func copies() async throws {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        let command = "curl 'https://example.com/api' \\\n  -H 'Authorization: Bearer TEST_TOKEN'"
        await session.answerCurl("a", with: .success(CurlGenerated(curl: command, truncated: false)))
        let model = await started(session, clipboard: clipboard)

        await model.copyAsCurl("a")

        #expect(clipboard.copies == [command])
        // The multiline shape, and the credential, go across exactly.
        #expect(clipboard.last?.contains("\\\n") == true)
        #expect(clipboard.last?.contains("Bearer TEST_TOKEN") == true)
        #expect(model.lastAction == "Copied as cURL.")
        #expect(model.lastError == nil)
    }

    @Test("the app never assembles the command itself")
    func asksTheCore() async {
        let session = FakeSession()
        await session.answerCurl("a", with: .success(CurlGenerated(curl: "curl 'x'", truncated: false)))
        let model = await started(session, clipboard: FakeClipboard())

        await model.copyAsCurl("a")

        #expect(await session.curlRequests == ["a"])
    }

    @Test("a command that sends less than the request did says so")
    func truncated() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        await session.answerCurl(
            "a",
            with: .success(CurlGenerated(curl: "# truncated\ncurl 'x'", truncated: true))
        )
        let model = await started(session, clipboard: clipboard)

        await model.copyAsCurl("a")

        #expect(clipboard.last == "# truncated\ncurl 'x'")
        #expect(model.lastAction?.contains("truncated") == true)
    }

    @Test("copying the selection copies the selected exchange")
    func copiesSelection() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [summary("a")], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerCurl("a", with: .success(CurlGenerated(curl: "curl 'x'", truncated: false)))
        let model = await started(session, clipboard: clipboard)

        await model.select("a")
        await model.copySelectedAsCurl()

        #expect(clipboard.copies == ["curl 'x'"])
    }

    @Test("copying nothing copies nothing")
    func copiesNothingWithNoSelection() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        let model = await started(session, clipboard: clipboard)

        await model.copySelectedAsCurl()

        #expect(clipboard.copies.isEmpty)
        #expect(await session.curlRequests.isEmpty)
    }

    @Test("an exchange that is gone is reported and nothing is copied")
    func copyFails() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        let model = await started(session, clipboard: clipboard)

        await model.copyAsCurl("missing")

        #expect(clipboard.copies.isEmpty)
        #expect(model.lastError == "there is no stored exchange with that id")
        #expect(model.lastAction == nil)
    }

    // MARK: - The import sheet

    @Test("the sheet opens empty and closes with nothing kept")
    func opensAndCloses() async {
        let session = FakeSession()
        let model = await started(session, clipboard: FakeClipboard())

        #expect(model.importer == nil)

        model.beginImport()
        let importer = try? #require(model.importer)
        #expect(importer?.stage == .pasting)
        #expect(importer?.curlText.isEmpty == true)

        model.cancelImport()
        #expect(model.importer == nil)
    }

    @Test("editing an exchange opens the sheet on the core's own command")
    func editSelected() async {
        let session = FakeSession()
        let command = "curl 'https://example.com/api' -X POST"
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [summary("a")], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerCurl("a", with: .success(CurlGenerated(curl: command, truncated: false)))
        await session.answerParses(with: [
            .success(
                CurlParseResult(
                    request: ReplayRequest(method: "POST", url: "https://example.com/api")
                ))
        ])
        let model = await started(session, clipboard: FakeClipboard())

        await model.select("a")
        await model.editSelectedAsRequest()

        let importer = try? #require(model.importer)
        #expect(importer?.stage == .editing)
        #expect(importer?.method == "POST")
        #expect(importer?.url == "https://example.com/api")
        // The command came from the core and went back to the core: the app
        // did not read it itself.
        #expect(await session.parsedCommands == [command])
    }

    // MARK: - Replaying

    @Test("replaying sends what the sheet describes and closes it")
    func replays() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .success(ReplayResult(exchangeID: "new", status: 201, error: nil))
        )
        let model = await started(session, clipboard: FakeClipboard())

        model.beginImport()
        model.importer?.show(
            ReplayRequest(
                method: "POST",
                url: "https://example.com/api",
                body: Data(#"{"hello":"world"}"#.utf8)
            ))
        await model.replayImported()

        #expect(model.importer == nil)
        #expect(model.lastError == nil)
        #expect(model.lastAction?.contains("201") == true)

        let sent = await session.replayed
        #expect(sent.count == 1)
        #expect(sent.first?.body == Data(#"{"hello":"world"}"#.utf8))
    }

    @Test("a replay the core refuses leaves the sheet open, with everything in it")
    func refused() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .failure(
                .server(IPCErrorBody(code: .invalidRequest, message: "that URL names no host")))
        )
        let model = await started(session, clipboard: FakeClipboard())

        model.beginImport()
        model.importer?.show(ReplayRequest(method: "GET", url: "http:///nowhere"))
        await model.replayImported()

        #expect(model.importer != nil)
        #expect(model.importer?.replayState == .failed("that URL names no host"))
        #expect(model.importer?.url == "http:///nowhere")
        #expect(model.lastAction == nil)
    }

    @Test("a replay that failed at the destination still closes the sheet")
    func failedAtTheDestination() async {
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
        let model = await started(session, clipboard: FakeClipboard())

        model.beginImport()
        model.importer?.show(ReplayRequest(method: "GET", url: "http://nowhere.test/"))
        await model.replayImported()

        // It was sent, there is an exchange for it, and the failure is in the
        // traffic list where every other failure is.
        #expect(model.importer == nil)
        #expect(model.lastAction?.contains("could not connect") == true)
    }

    @Test("replaying nothing sends nothing")
    func nothingToReplay() async {
        let session = FakeSession()
        let model = await started(session, clipboard: FakeClipboard())

        await model.replayImported()

        #expect(await session.replayed.isEmpty)
    }

    // MARK: - What the list and the detail do afterwards

    @Test("a replay does not disturb what is selected")
    func selectionSurvives() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [summary("a")], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerReplay(
            with: .success(ReplayResult(exchangeID: "new", status: 200, error: nil))
        )
        let model = await started(session, clipboard: FakeClipboard())

        await model.select("a")
        guard case let .loaded(before) = model.detail else {
            Issue.record("the selected exchange did not load")
            return
        }

        model.beginImport()
        model.importer?.show(ReplayRequest(method: "GET", url: "https://example.com/"))
        await model.replayImported()

        #expect(model.selectedID == "a")
        guard case let .loaded(after) = model.detail else {
            Issue.record("the detail stopped showing the exchange")
            return
        }
        #expect(after == before)
    }

    @Test("the replayed exchange arrives as a row, like every other exchange")
    func arrivesAsARow() async {
        let session = FakeSession()
        await session.answerReplay(
            with: .success(ReplayResult(exchangeID: "new", status: 200, error: nil))
        )
        let model = await started(session, clipboard: FakeClipboard())

        model.beginImport()
        model.importer?.show(ReplayRequest(method: "GET", url: "https://example.com/"))
        await model.replayImported()

        #expect(model.exchanges.isEmpty)

        // The core publishes it on the traffic stream, as it does for a
        // request a browser made.
        await model.handle(.event(.trafficAdded(summary("new"))))

        #expect(model.exchanges.map(\.id) == ["new"])
        #expect(model.totalExchanges == 1)
        // Still not selected: it is a row, and rows are clicked.
        #expect(model.selectedID == nil)
    }

    @Test("a failure clears the last confirmation rather than sitting beside it")
    func failureReplacesConfirmation() async {
        let session = FakeSession()
        let clipboard = FakeClipboard()
        await session.answerCurl("a", with: .success(CurlGenerated(curl: "curl 'x'", truncated: false)))
        let model = await started(session, clipboard: clipboard)

        await model.copyAsCurl("a")
        #expect(model.lastAction != nil)

        await model.copyAsCurl("missing")

        #expect(model.lastAction == nil)
        #expect(model.lastError != nil)
    }
}

@MainActor
@Suite("Clipboard")
struct ClipboardTests {
    /// A pasteboard of this suite's own, so nothing here touches
    /// `NSPasteboard.general` — which is the developer's own clipboard, and is
    /// not a thing a test run gets to replace.
    private func privatePasteboard() -> NSPasteboard {
        NSPasteboard(name: .init("ProxyLabTests-\(UUID().uuidString)"))
    }

    @Test("a command reaches the pasteboard as the text it is")
    func copies() throws {
        let pasteboard = privatePasteboard()
        let command = "curl 'https://example.com/api' \\\n  -H 'Accept: application/json'"

        SystemClipboard(pasteboard: pasteboard).copy(command)

        // The newlines and the backslashes that continue them go across
        // unchanged, so what is pasted into a terminal is the command.
        #expect(pasteboard.string(forType: .string) == command)
    }

    @Test("quotes and shell characters survive being copied")
    func quoting() throws {
        let pasteboard = privatePasteboard()
        let command = "curl 'x' --data-raw '{\"it'\\''s\":\"$(not a command)\"}'"

        SystemClipboard(pasteboard: pasteboard).copy(command)

        #expect(pasteboard.string(forType: .string) == command)
    }

    @Test("copying replaces what was there rather than adding to it")
    func replaces() throws {
        let pasteboard = privatePasteboard()
        let clipboard = SystemClipboard(pasteboard: pasteboard)

        clipboard.copy("curl 'first'")
        clipboard.copy("curl 'second'")

        #expect(pasteboard.string(forType: .string) == "curl 'second'")
        #expect(pasteboard.pasteboardItems?.count == 1)
    }
}

@MainActor
@Suite("Editing an exchange as a request")
struct EditAsRequestTests {
    @Test("an exchange that cannot be read opens no sheet")
    func noSheetOnFailure() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [Fixtures.summary(id: "a")], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        await model.start()
        await model.select("a")

        // No command was set for "a", so the core answers not_found.
        await model.editSelectedAsRequest()

        #expect(model.importer == nil)
        #expect(model.lastError == "there is no stored exchange with that id")
    }

    @Test("editing nothing opens no sheet")
    func noSelection() async {
        let session = FakeSession()
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/test.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )

        await model.editSelectedAsRequest()

        #expect(model.importer == nil)
        #expect(await session.curlRequests.isEmpty)
    }
}
