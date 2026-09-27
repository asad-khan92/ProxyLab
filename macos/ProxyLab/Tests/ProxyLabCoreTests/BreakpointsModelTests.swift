//  The breakpoints screen: what it shows while an exchange is held, and what
//  it sends when somebody decides about one.
//
//  The behaviour worth being sure of is that a held exchange is answered
//  exactly once and then stops being shown — whether it was answered here, by
//  another window, or by the core giving up on it — and that only what was
//  actually changed is sent, because a field the core is not sent is a field
//  it leaves alone. Neither needs a socket or any traffic.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Breakpoints")
struct BreakpointsModelTests {
    // MARK: - Fixtures

    private func model(_ session: FakeBreakpointsSession) -> BreakpointsModel {
        BreakpointsModel(session: session)
    }

    private func breakpoint(
        _ id: String,
        enabled: Bool = true,
        direction: BreakpointDirection = .request
    ) -> Breakpoint {
        Breakpoint(
            id: id,
            enabled: enabled,
            direction: direction,
            matcher: RuleMatcher(host: .exact("api.example.com"))
        )
    }

    private func pausedRequest(
        _ pauseID: String = "pause-1",
        body: Data = Data("{\"id\":1}".utf8),
        truncated: Bool = false
    ) -> PausedExchange {
        PausedExchange(
            pauseID: pauseID,
            breakpointID: "bp-1",
            direction: .request,
            method: "POST",
            url: "https://api.example.com/orders",
            request: PausedRequest(
                method: "POST",
                url: "https://api.example.com/orders",
                headers: Headers([
                    Header(name: "Accept", value: .text("application/json"))
                ]),
                body: BodyCapture(
                    data: body,
                    size: truncated ? 99_999 : UInt64(body.count),
                    truncated: truncated
                )
            )
        )
    }

    private func pausedResponse(_ pauseID: String = "pause-2") -> PausedExchange {
        PausedExchange(
            pauseID: pauseID,
            breakpointID: "bp-2",
            direction: .response,
            method: "GET",
            url: "https://api.example.com/orders",
            response: PausedResponse(
                status: 200,
                headers: Headers([
                    Header(name: "Content-Type", value: .text("application/json"))
                ]),
                body: BodyCapture.complete(Data("{\"ok\":true}".utf8))
            )
        )
    }

    private func pausedWebSocket(_ pauseID: String = "pause-ws") -> PausedExchange {
        PausedExchange(
            pauseID: pauseID,
            breakpointID: "bp-ws",
            direction: .websocket,
            method: "GET",
            url: "http://example.com/socket",
            websocket: PausedWebSocketMessage(
                direction: .serverToClient,
                payload: BodyCapture.complete(Data("ping".utf8))
            )
        )
    }

    // MARK: - Loading and watching

    @Test("the list starts out loading, before anything has been asked")
    func startsLoading() {
        let model = model(FakeBreakpointsSession())

        #expect(model.state == .loading)
        #expect(model.breakpoints.isEmpty)
        #expect(!model.watching)
    }

    @Test("loading asks for the breakpoints, the throttling, and to watch")
    func loads() async {
        let session = FakeBreakpointsSession()
        await session.answerList(with: [breakpoint("bp-1")])
        await session.answerThrottle(with: ThrottleSettings(enabled: true, latencyMs: 200))

        let model = model(session)
        await model.load()

        #expect(model.state == .loaded)
        #expect(model.breakpoints.map(\.id) == ["bp-1"])
        #expect(model.throttle.latencyMs == 200)
        #expect(model.watching)
        #expect(await session.calls.contains(.watch))
    }

    @Test("a core that will not let this app watch says so, because nothing will pause")
    func watchingRefused() async {
        let session = FakeBreakpointsSession()
        await session.answerWatch(
            with: .failure(
                IPCError.server(IPCErrorBody(code: .internal, message: "not managing breakpoints"))
            )
        )

        let model = model(session)
        await model.load()

        #expect(!model.watching)
        #expect(model.lastError?.contains("will not pause anything") == true)
    }

    @Test("a list that fails says so and does not leave the app watching silently")
    func loadFails() async {
        let session = FakeBreakpointsSession()
        await session.answerLists(
            with: [.failure(IPCError.server(IPCErrorBody(code: .internal, message: "no")))]
        )

        let model = model(session)
        await model.load()

        #expect(model.state == .failed("no"))
        #expect(!model.watching)
        #expect(await !session.calls.contains(.watch))
    }

    // MARK: - Pauses arriving

    @Test("a paused exchange opens the editor on it")
    func pauseOpensEditor() async {
        let model = model(FakeBreakpointsSession())

        model.received(pausedRequest())

        #expect(model.current?.pauseID == "pause-1")
        #expect(model.editor?.paused.pauseID == "pause-1")
        #expect(model.queuedCount == 0)
    }

    @Test("a second pause waits behind the first rather than replacing it")
    func pausesQueue() async {
        let model = model(FakeBreakpointsSession())

        model.received(pausedRequest("pause-1"))
        model.received(pausedResponse("pause-2"))

        #expect(model.current?.pauseID == "pause-1")
        #expect(model.queuedCount == 1)
        #expect(model.editor?.paused.pauseID == "pause-1")
    }

    @Test("the same pause arriving twice is one pause")
    func pausesAreNotDuplicated() {
        let model = model(FakeBreakpointsSession())

        model.received(pausedRequest("pause-1"))
        model.received(pausedRequest("pause-1"))

        #expect(model.paused.count == 1)
    }

    @Test("a pause the core released is taken off the queue and the next one opens")
    func releaseMovesOn() {
        let model = model(FakeBreakpointsSession())
        model.received(pausedRequest("pause-1"))
        model.received(pausedResponse("pause-2"))

        model.released(pauseID: "pause-1", reason: .timedOut)

        #expect(model.current?.pauseID == "pause-2")
        #expect(model.editor?.paused.pauseID == "pause-2")
        #expect(model.lastAction?.contains("in time") == true)
    }

    @Test("losing the connection empties the queue, because the core let go of it")
    func connectionLost() {
        let model = model(FakeBreakpointsSession())
        model.received(pausedRequest())

        model.connectionLost()

        #expect(model.paused.isEmpty)
        #expect(model.editor == nil)
        #expect(!model.watching)
    }

    // MARK: - Deciding

    @Test("continuing with nothing changed sends no edit at all")
    func continueUnedited() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedRequest())

        #expect(await model.continuePause())

        let edits = await session.edits()
        #expect(edits.count == 1)
        #expect(edits[0].request == nil)
        #expect(edits[0].response == nil)
        #expect(edits[0].websocket == nil)
        #expect(model.paused.isEmpty)
        #expect(model.editor == nil)
    }

    @Test("only the fields that were changed are sent")
    func continueEdited() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedRequest())

        model.editor?.method = "PUT"
        model.editor?.bodyText = "{\"id\":2}"

        #expect(await model.continuePause())

        let edit = await session.edits()[0].request
        #expect(edit?.method == "PUT")
        #expect(edit?.body == Data("{\"id\":2}".utf8))
        // Untouched, so left out: a field the core is not sent is one it
        // leaves exactly as it was.
        #expect(edit?.url == nil)
        #expect(edit?.headers == nil)
    }

    @Test("a response edit answers a response pause, and never a request one")
    func continueResponse() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedResponse())

        model.editor?.status = "503"

        #expect(await model.continuePause())

        let edits = await session.edits()[0]
        #expect(edits.request == nil)
        #expect(edits.response?.status == 503)
    }

    @Test("continuing a WebSocket pause sends only the payload that changed")
    func continueWebSocket() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedWebSocket())

        model.editor?.bodyText = "pong"

        #expect(await model.continuePause())

        let edits = await session.edits()[0]
        #expect(edits.request == nil)
        #expect(edits.response == nil)
        #expect(edits.websocket?.payload == Data("pong".utf8))
        #expect(model.paused.isEmpty)
    }

    @Test("dropping a WebSocket message is a cancel of that pause")
    func dropWebSocketMessage() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedWebSocket())

        #expect(await model.cancelPause())

        #expect(await session.calls.contains(.cancel("pause-ws")))
        #expect(model.paused.isEmpty)
    }

    @Test("cancelling sends a cancel and stops showing the exchange")
    func cancels() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedRequest())

        #expect(await model.cancelPause())

        #expect(await session.calls.contains(.cancel("pause-1")))
        #expect(model.paused.isEmpty)
        #expect(model.editor == nil)
    }

    @Test("a pause the core had already let go of is said so rather than failing")
    func decidingSomethingAlreadyReleased() async {
        let session = FakeBreakpointsSession()
        await session.answerResolve(
            with: .success(PauseResolvedResult(pauseID: "pause-1", resolved: false))
        )
        let model = model(session)
        model.received(pausedRequest())

        #expect(await model.continuePause())

        #expect(model.lastError == nil)
        #expect(model.lastAction?.contains("no longer being held") == true)
    }

    @Test("a decision that could not be sent keeps the exchange on screen")
    func decisionFails() async {
        let session = FakeBreakpointsSession()
        await session.answerResolve(
            with: .failure(
                IPCError.server(IPCErrorBody(code: .internal, message: "the core said no"))
            )
        )
        let model = model(session)
        model.received(pausedRequest())

        #expect(await !model.continuePause())

        #expect(model.lastError == "the core said no")
        #expect(model.current?.pauseID == "pause-1")
    }

    @Test("an edit the core would refuse is not sent at all")
    func refusedEditIsNotSent() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        model.received(pausedResponse())

        model.editor?.status = "999"

        #expect(await !model.continuePause())

        #expect(await session.edits().isEmpty)
        #expect(model.lastError?.contains("100 to 599") == true)
        #expect(model.current?.pauseID == "pause-2")
    }

    // MARK: - Configuring

    @Test("turning one off shows what the core confirmed, not what was clicked")
    func setEnabled() async {
        let session = FakeBreakpointsSession()
        await session.answerList(with: [breakpoint("bp-1")])
        await session.answerUpdate(with: .success(breakpoint("bp-1", enabled: false)))

        let model = model(session)
        await model.load()
        await model.setEnabled(false, for: breakpoint("bp-1"))

        #expect(model.breakpoints[0].enabled == false)
        #expect(model.enabledCount == 0)
    }

    @Test("a breakpoint removed elsewhere is said to be gone, not to have failed")
    func updateOfSomethingDeleted() async {
        let session = FakeBreakpointsSession()
        await session.answerList(with: [breakpoint("bp-1")])
        await session.answerUpdate(
            with: .failure(
                IPCError.server(IPCErrorBody(code: .notFound, message: "no such breakpoint"))
            )
        )

        let model = model(session)
        await model.load()
        await session.answerList(with: [])
        await model.setEnabled(false, for: breakpoint("bp-1"))

        #expect(model.breakpoints.isEmpty)
        #expect(model.lastError == "That breakpoint no longer exists.")
    }

    @Test("creating one asks the core and then reads the list back")
    func creates() async {
        let session = FakeBreakpointsSession()
        await session.answerList(with: [])
        await session.answerCreate(with: .success(breakpoint("bp-new")))

        let model = model(session)
        await model.load()
        await session.answerList(with: [breakpoint("bp-new")])

        #expect(await model.create(direction: .response, matcher: .any))
        #expect(model.breakpoints.map(\.id) == ["bp-new"])
    }

    @Test("deleting one that had already gone is not a failure")
    func deleteOfSomethingGone() async {
        let session = FakeBreakpointsSession()
        await session.answerList(with: [breakpoint("bp-1")])
        await session.answerDelete(
            with: .success(BreakpointDeletedResult(breakpointID: "bp-1", deleted: false))
        )

        let model = model(session)
        await model.load()
        await session.answerList(with: [])

        #expect(await model.delete(breakpoint("bp-1")))
        #expect(model.lastError == "That breakpoint no longer existed.")
    }

    // MARK: - Throttling

    @Test("throttling is sent as one set of numbers and shown as the core confirmed them")
    func appliesThrottle() async {
        let session = FakeBreakpointsSession()
        let model = model(session)

        let settings = ThrottleSettings(
            enabled: true,
            latencyMs: 250,
            downloadBytesPerSecond: 131_072,
            uploadBytesPerSecond: 32_768
        )

        #expect(await model.applyThrottle(settings))
        #expect(model.throttle == settings)
        #expect(await session.calls.contains(.setThrottle(settings)))
    }

    @Test("numbers the core would refuse are not sent at all")
    func refusesBadThrottle() async {
        let session = FakeBreakpointsSession()
        let model = model(session)

        let settings = ThrottleSettings(enabled: true, latencyMs: maxThrottleLatencyMs + 1)

        #expect(await !model.applyThrottle(settings))
        #expect(await session.calls.isEmpty)
        #expect(model.lastError?.contains("latency") == true)
    }

    @Test("turning throttling off keeps the numbers that were typed")
    func togglesWithoutLosingNumbers() async {
        let session = FakeBreakpointsSession()
        let model = model(session)
        await model.applyThrottle(
            ThrottleSettings(enabled: true, latencyMs: 300, downloadBytesPerSecond: 4_096)
        )

        await model.setThrottleEnabled(false)

        #expect(!model.throttle.enabled)
        #expect(model.throttle.latencyMs == 300)
        #expect(model.throttle.downloadBytesPerSecond == 4_096)
        #expect(!model.throttle.isShaping)
    }
}
