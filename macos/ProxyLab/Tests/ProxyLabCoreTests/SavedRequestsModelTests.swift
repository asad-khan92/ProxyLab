//  What the saved requests model does with what the core answers.
//
//  Four operations, an editor, and the one rule that makes the list
//  trustworthy: nothing on screen changes before the core says it changed. So
//  what is asserted is the request that was made and what was shown afterwards
//  — never a delay, and never a socket.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Saved requests")
@MainActor
struct SavedRequestsModelTests {
    /// A model over doubles for both halves: the saved requests are one
    /// session, and sending one is the traffic session's replay.
    private func model(
        _ session: FakeSavedRequestsSession,
        traffic: FakeSession = FakeSession()
    ) -> SavedRequestsModel {
        SavedRequestsModel(session: session, traffic: traffic)
    }

    @Test("the list is what the core answered, in the order it answered it")
    func listing() async {
        let session = FakeSavedRequestsSession()
        await session.answerList(with: [
            SavedRequestFixtures.request(id: "1", name: "First"),
            SavedRequestFixtures.request(id: "2", name: "Second"),
        ])
        let model = model(session)

        await model.reload()

        #expect(model.requests.map(\.name) == ["First", "Second"])
        #expect(model.state == .loaded)
        #expect(!model.isEmpty)
    }

    @Test("a list that could not be fetched is said to have failed")
    func listingFails() async {
        let session = FakeSavedRequestsSession()
        await session.answerLists(with: [.failure(.notConnected)])
        let model = model(session)

        await model.reload()

        #expect(model.requests.isEmpty)
        #expect(model.state == .failed(IPCError.notConnected.message))
        #expect(model.lastError == IPCError.notConnected.message)
    }

    @Test("nothing is asked for until the list is loaded")
    func nothingUntilLoaded() async {
        // The section asks when the connection comes up, and not before: a
        // model that fetched on construction would ask a core that is not
        // there.
        let session = FakeSavedRequestsSession()
        let model = model(session)

        #expect(model.state == .idle)
        #expect(await session.calls.isEmpty)

        await model.reload()

        #expect(await session.calls == [.list])
    }

    // MARK: - Keeping one

    @Test("saving a captured request asks the core for it by exchange id")
    func savingFromTraffic() async {
        // The request is the core's: it stored the request and is what knows
        // what it was, so nothing here assembles one out of a row.
        let session = FakeSavedRequestsSession()
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "GET /orders")
        #expect(model.saving?.name == "GET /orders")
        await model.confirmSave()

        #expect(
            await session.calls.first == .fromTraffic(exchangeID: "e1", name: "GET /orders")
        )
        #expect(model.saving == nil)
        #expect(model.lastError == nil)
    }

    @Test("saving reloads the list, so what is shown is what the core has")
    func savingReloads() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "A name")
        await model.confirmSave()

        #expect(await session.calls.contains(.list))
    }

    @Test("a save with no name is refused here rather than sent")
    func savingNeedsAName() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "   ")
        await model.confirmSave()

        #expect(await session.calls.isEmpty)
        #expect(model.lastError != nil)
    }

    @Test("a truncated capture is said to send less than the request did")
    func truncatedIsReported() async {
        let session = FakeSavedRequestsSession()
        await session.answerFromTraffic(with: .success(SavedRequestFixtures.truncated))
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "A big upload")
        await model.confirmSave()

        #expect(model.lastAction?.contains("truncated") == true)
    }

    @Test("a save the core refused is shown and changes nothing")
    func savingFails() async {
        let session = FakeSavedRequestsSession()
        await session.answerFromTraffic(
            with: .failure(
                .server(IPCErrorBody(code: .invalidSavedRequest, message: "that is not a URL"))
            )
        )
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "No")
        await model.confirmSave()

        #expect(model.lastError == "that is not a URL")
        #expect(model.lastAction == nil)
    }

    @Test("cancelling a save sends nothing")
    func cancellingSave() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        model.beginSave(exchange: "e1", suggestedName: "A name")
        model.cancelSave()
        await model.confirmSave()

        #expect(await session.calls.isEmpty)
        #expect(model.saving == nil)
    }

    // MARK: - Renaming and removing

    @Test("a rename sends the name and nothing else")
    func renaming() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        await model.rename("s1", to: "  Create Ada  ")

        #expect(await session.calls.first == .update("s1", .rename("Create Ada")))
    }

    @Test("a rename to nothing is refused here rather than sent")
    func renamingNeedsAName() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        await model.rename("s1", to: " ")

        #expect(await session.calls.isEmpty)
        #expect(model.lastError != nil)
    }

    @Test("a rename in progress is dropped when it is cancelled")
    func cancellingRename() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)

        model.beginRename(SavedRequestFixtures.request())
        #expect(model.renaming != nil)
        model.cancelRename()

        #expect(model.renaming == nil)
        #expect(await session.calls.isEmpty)
    }

    @Test("deleting asks the core and then reloads")
    func deleting() async {
        let session = FakeSavedRequestsSession()
        await session.answerList(with: [SavedRequestFixtures.request(id: "s1")])
        let model = model(session)
        await model.reload()

        await model.delete("s1")

        #expect(await session.calls.contains(.delete("s1")))
        #expect(model.lastAction?.contains("Create a user") == true)
    }

    @Test("a delete the core refused leaves the list alone")
    func deletingFails() async {
        let session = FakeSavedRequestsSession()
        await session.answerList(with: [SavedRequestFixtures.request(id: "s1")])
        await session.answerDelete(with: .failure(.notConnected))
        let model = model(session)
        await model.reload()

        await model.delete("s1")

        #expect(model.requests.count == 1)
        #expect(model.lastError == IPCError.notConnected.message)
    }

    // MARK: - The editor

    @Test("opening reads the request back from the core rather than the row")
    func opening() async {
        let session = FakeSavedRequestsSession()
        await session.answerGet(with: .success(SavedRequestFixtures.request(id: "s1")))
        let model = model(session)

        await model.open(SavedRequestFixtures.request(id: "s1", name: "a stale name"))

        #expect(await session.calls == [.get("s1")])
        #expect(model.editor?.name == "Create a user")
        #expect(model.editor?.request.method == "POST")
        #expect(model.editor?.request.url == "https://example.com/users")
        #expect(model.editor?.request.headers.count == 2)
        #expect(model.editor?.request.bodyText == #"{"name":"ada"}"#)
    }

    @Test("a request that is no longer there is said to be gone rather than opened")
    func openingSomethingDeleted() async {
        let session = FakeSavedRequestsSession()
        await session.answerGet(
            with: .failure(.server(IPCErrorBody(code: .notFound, message: "no such request")))
        )
        let model = model(session)

        await model.open(SavedRequestFixtures.request(id: "s1"))

        #expect(model.editor == nil)
        #expect(model.lastError?.contains("no longer exists") == true)
    }

    @Test("saving an edit sends every field, under the name in the sheet")
    func savingEdits() async {
        let session = FakeSavedRequestsSession()
        await session.answerGet(with: .success(SavedRequestFixtures.request(id: "s1")))
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))

        let editor = try! #require(model.editor)
        editor.name = "Create Ada"
        editor.request.url = "https://example.com/users/1"
        editor.request.method = "PUT"
        await model.saveEdits()

        let update = SavedRequestUpdate(
            name: "Create Ada",
            method: "PUT",
            url: "https://example.com/users/1",
            headers: Headers([
                Header(name: "Content-Type", value: .text("application/json")),
                Header(name: "Authorization", value: .text("Bearer sk-secret")),
            ]),
            body: Data(#"{"name":"ada"}"#.utf8)
        )
        #expect(await session.calls.contains(.update("s1", update)))
        #expect(editor.saveState == .saved)
    }

    @Test("the sheet stays open after a save, because sending is the next thing")
    func savingKeepsTheSheetOpen() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))

        await model.saveEdits()

        #expect(model.editor != nil)
    }

    @Test("a save the core refused is shown in the sheet and keeps it open")
    func savingEditsFails() async {
        let session = FakeSavedRequestsSession()
        await session.answerUpdate(
            with: .failure(
                .server(IPCErrorBody(code: .invalidSavedRequest, message: "that is not a URL"))
            )
        )
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))

        await model.saveEdits()

        #expect(model.editor?.saveState == .failed("that is not a URL"))
        #expect(model.editor != nil)
    }

    @Test("an edit with no name is refused before it is sent")
    func editNeedsAName() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))
        model.editor?.name = "  "

        await model.saveEdits()

        #expect(await !session.calls.contains { call in
            if case .update = call { return true }
            return false
        })
        #expect(model.editor?.saveState != .saved)
    }

    @Test("a truncated saved request says so in the editor")
    func editorSaysTruncated() async {
        let session = FakeSavedRequestsSession()
        await session.answerGet(with: .success(SavedRequestFixtures.truncated))
        let model = model(session)

        await model.open(SavedRequestFixtures.truncated)

        #expect(model.editor?.sendsLessThanItWas == true)
    }

    @Test("closing the editor keeps nothing that was not saved")
    func closing() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))
        model.editor?.name = "Never saved"

        model.closeEditor()

        #expect(model.editor == nil)
        #expect(await !session.calls.contains { call in
            if case .update = call { return true }
            return false
        })
    }

    @Test("deleting the request an editor is open on closes it")
    func deletingWhatIsOpen() async {
        let session = FakeSavedRequestsSession()
        let model = model(session)
        await model.open(SavedRequestFixtures.request(id: "s1"))

        await model.delete("s1")

        #expect(model.editor == nil)
    }

    // MARK: - Sending one

    @Test("replaying a saved request sends it through the traffic session")
    func replaying() async {
        // The one replay path there is: the core's rules, TLS, capture and
        // storage, and a new exchange in the list.
        let session = FakeSavedRequestsSession()
        let traffic = FakeSession()
        let model = model(session, traffic: traffic)

        await model.replay(SavedRequestFixtures.request())

        let sent = await traffic.replayed
        #expect(sent.count == 1)
        #expect(sent.first?.method == "POST")
        #expect(sent.first?.url == "https://example.com/users")
        #expect(sent.first?.headers.text("authorization") == "Bearer sk-secret")
        #expect(sent.first?.body == Data(#"{"name":"ada"}"#.utf8))
        #expect(model.lastAction?.contains("Replayed") == true)
    }

    @Test("a truncated saved request sends the bytes the core kept")
    func replayingTruncated() async {
        let session = FakeSavedRequestsSession()
        let traffic = FakeSession()
        let model = model(session, traffic: traffic)

        await model.replay(SavedRequestFixtures.truncated)

        #expect(await traffic.replayed.first?.body == Data("the first bit".utf8))
    }

    @Test("replaying from the editor sends what is on screen and writes nothing down")
    func replayingEdits() async {
        let session = FakeSavedRequestsSession()
        let traffic = FakeSession()
        let model = model(session, traffic: traffic)
        await model.open(SavedRequestFixtures.request(id: "s1"))
        model.editor?.request.url = "https://example.com/users/2"

        await model.replayEdits()

        #expect(await traffic.replayed.first?.url == "https://example.com/users/2")
        // Not saved: an edit made in order to send it once is not a change to
        // what is kept.
        #expect(await !session.calls.contains { call in
            if case .update = call { return true }
            return false
        })
        #expect(model.editor != nil)
    }

    @Test("a replay the core would not send is shown and nothing is claimed")
    func replayingFails() async {
        let session = FakeSavedRequestsSession()
        let traffic = FakeSession()
        await traffic.answerReplay(with: .failure(.notConnected))
        let model = model(session, traffic: traffic)

        await model.replay(SavedRequestFixtures.request())

        #expect(model.lastError == IPCError.notConnected.message)
        #expect(model.lastAction == nil)
    }
}

@Suite("Saved request values")
struct SavedRequestValueTests {
    @Test("a saved request describes the request it would send")
    func replayRequest() {
        let request = SavedRequestFixtures.request().replayRequest

        #expect(request.method == "POST")
        #expect(request.url == "https://example.com/users")
        #expect(request.headers.count == 2)
        #expect(request.body == Data(#"{"name":"ada"}"#.utf8))
    }

    @Test("a truncated capture is one that sends less than the request did")
    func truncated() {
        #expect(SavedRequestFixtures.truncated.sendsLessThanItWas)
        #expect(!SavedRequestFixtures.request().sendsLessThanItWas)
    }

    @Test("an update that names nothing is empty, and the core is not asked")
    func emptyUpdate() {
        #expect(SavedRequestUpdate().isEmpty)
        #expect(!SavedRequestUpdate.rename("x").isEmpty)
        #expect(!SavedRequestUpdate.request(ReplayRequest(method: "GET", url: "x")).isEmpty)
    }

    @Test("a new saved request can be built from a request and a name")
    func fromRequest() {
        let new = NewSavedRequest(
            name: "Create a user",
            request: ReplayRequest(
                method: "POST",
                url: "https://example.com/users",
                body: Data("{}".utf8)
            )
        )

        #expect(new.name == "Create a user")
        #expect(new.method == "POST")
        #expect(new.body == Data("{}".utf8))
    }
}
