//  Menu and shortcut availability, and that a command uses the path the
//  buttons already use.
//
//  Replay of a handshake stays disabled without fetching the exchange.
//  Delete Rule stops at the confirmation. Clear Traffic stops at its
//  confirmation. Start and Stop follow the capture control that is already
//  showing.

import Foundation
import Testing

@testable import ProxyLabCore

@MainActor
@Suite("Menu commands")
struct AppCommandTests {
    private func summary(
        id: String = "a",
        status: UInt16? = 200,
        contentType: String? = "application/json"
    ) -> ExchangeSummary {
        ExchangeSummary(
            id: id,
            startedAt: Timestamp(seconds: 1_700_000_000, nanos: 0),
            client: "127.0.0.1:1",
            scheme: .https,
            destination: Destination(host: "api.example.com", port: 443),
            method: "GET",
            url: "https://api.example.com/products",
            version: .http11,
            status: status,
            responseContentType: contentType,
            completed: true
        )
    }

    private func state(
        section: AppSection = .traffic,
        captureAction: CaptureControlAction? = nil,
        connected: Bool = true,
        summary: ExchangeSummary? = nil,
        selected: Bool = false,
        editor: Bool = false,
        choosing: Bool = false,
        rule: Bool = false,
        narrowed: Bool = false
    ) -> AppCommandState {
        AppCommandState(
            section: section,
            captureAction: captureAction,
            trafficConnected: connected,
            trafficNarrowed: narrowed,
            selectedSummary: summary,
            hasTrafficSelection: selected || summary != nil,
            rulesEditorOpen: editor,
            choosingRuleKind: choosing,
            hasSelectedRule: rule
        )
    }

    private func router(
        traffic: TrafficInspectorModel? = nil,
        rules: RulesModel? = nil,
        breakpoints: BreakpointsModel? = nil,
        performed: @escaping @MainActor (CaptureControlAction) -> Void = { _ in }
    ) -> AppCommandRouter {
        let session = FakeSession()
        let traffic = traffic ?? TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        let rules = rules ?? RulesModel(session: FakeRulesSession(), chooser: FakeFileChooser())
        let saved = SavedRequestsModel(session: FakeSavedRequestsSession(), traffic: session)
        return AppCommandRouter(
            traffic: traffic,
            rules: rules,
            saved: saved,
            breakpoints: breakpoints,
            performCapture: performed
        )
    }

    // MARK: - Shortcuts

    @Test("the shortcuts that are registered, and the ones that are not")
    func keys() {
        #expect(AppCommand.toggleCapture.key == .character("s", shift: true))
        #expect(AppCommand.find.key == .character("f"))
        #expect(AppCommand.replay.key == .character("r"))
        #expect(AppCommand.copyCurl.key == .character("c", shift: true))
        #expect(AppCommand.deleteSelection.key == .delete)
        #expect(AppCommand.clearTraffic.key == .character("k"))
        #expect(AppCommand.newRule.key == .character("n"))
        #expect(AppCommand.duplicateRule.key == .character("d"))
        #expect(AppCommand.showTraffic.key == .character("1"))
        #expect(AppCommand.showRules.key == .character("2"))
        #expect(AppCommand.showBreaks.key == .character("3"))
        #expect(AppCommand.saveRequest.key == .none)
        #expect(AppCommand.breakpoint.key == .none)
        #expect(AppCommand.applyRule.key == .none)
    }

    // MARK: - Capture

    @Test("start and stop follow the capture control, and a transition offers neither")
    func capture() async {
        let stopped = state(captureAction: .start)
        #expect(stopped.canToggleCapture)
        #expect(stopped.captureTitle == "Start Capture")
        #expect(stopped.captureSymbol == "play.fill")
        #expect(AppCommand.toggleCapture.isEnabled(stopped))

        let listening = state(captureAction: .stop)
        #expect(listening.captureTitle == "Stop Capture")
        #expect(listening.captureSymbol == "stop.fill")

        let starting = state(captureAction: nil)
        #expect(!starting.canToggleCapture)
        #expect(!AppCommand.toggleCapture.isEnabled(starting))

        var performed: [CaptureControlAction] = []
        let commands = router { performed.append($0) }
        commands.replaceState(listening)
        await commands.perform(.toggleCapture)
        commands.replaceState(starting)
        await commands.perform(.toggleCapture)

        #expect(performed == [.stop])
    }

    // MARK: - Traffic

    @Test("replay and copy follow the row menu, and a handshake does not fetch")
    func replayAvailability() {
        let request = state(summary: summary())
        #expect(request.canReplay)
        #expect(request.canCopyCurl)
        #expect(request.canBreakpoint)
        #expect(AppCommand.breakpoint.isEnabled(request))
        #expect(!state(section: .rules, summary: summary()).canBreakpoint)
        #expect(AppCommand.replay.isEnabled(request))
        #expect(AppCommand.copyCurl.isEnabled(request))

        let handshake = state(summary: summary(status: 101, contentType: nil))
        #expect(!handshake.canReplay)
        #expect(!handshake.canCopyCurl)

        let events = state(summary: summary(contentType: "text/event-stream"))
        #expect(!events.canReplay)

        let elsewhere = state(section: .rules, summary: summary())
        #expect(!elsewhere.canReplay)
        #expect(!AppCommand.find.isEnabled(state(section: .breakpoints)))
        #expect(AppCommand.find.isEnabled(state(section: .rules, connected: false)))
    }

    @Test("Breakpoint opens the same draft the row menu opens")
    func breakpointOpensTheDraft() async {
        let breakpoints = BreakpointsModel(session: FakeBreakpointsSession())
        let commands = router(breakpoints: breakpoints)
        commands.replaceState(state(summary: summary(), selected: true))

        await commands.perform(.breakpoint)

        #expect(breakpoints.trafficDraft?.host == "api.example.com")
        #expect(breakpoints.trafficDraft?.path == "/products")
        #expect(breakpoints.trafficDraft?.method == "GET")
    }

    @Test("a disabled replay does not ask for the command")
    func disabledReplayDoesNotFetch() async {
        let session = FakeSession()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [summary()], nextCursor: nil, total: 1))
        ])
        await session.answerCurl("a", with: .success(CurlGenerated(curl: "curl 'https://example.com'", truncated: false)))
        let traffic = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await traffic.start()
        await traffic.select("a")

        let commands = router(traffic: traffic)
        commands.replaceState(state(summary: summary(status: 101, contentType: nil), selected: true))
        await commands.perform(.replay)
        await commands.perform(.copyCurl)

        #expect(await session.curlRequests.isEmpty)
        #expect(traffic.importer == nil)
    }

    @Test("replay of a selected request opens the existing sheet")
    func replayOpensTheSheet() async {
        let session = FakeSession()
        let row = summary()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [row], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        await session.answerCurl(
            "a",
            with: .success(CurlGenerated(curl: "curl 'https://api.example.com/products'", truncated: false))
        )
        let traffic = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await traffic.start()
        await traffic.select("a")

        let commands = router(traffic: traffic)
        commands.replaceState(state(summary: row, selected: true))
        await commands.perform(.replay)

        #expect(await session.curlRequests == ["a"])
        #expect(traffic.importer != nil)
    }

    @Test("delete exchange uses the existing delete, and clear stops at confirmation")
    func deleteAndClearTraffic() async {
        let session = FakeSession()
        let row = summary()
        await session.answerLists(with: [
            .success(TrafficListResult(exchanges: [row], nextCursor: nil, total: 1))
        ])
        await session.answerExchange("a", with: .success(Fixtures.exchange(id: "a")))
        let traffic = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab-test.sock",
            reconnect: .manual
        )
        await traffic.start()
        await traffic.select("a")

        let commands = router(traffic: traffic)
        commands.replaceState(state(connected: false))
        await commands.perform(.deleteSelection)
        await commands.perform(.clearTraffic)
        #expect(await session.deletedIDs.isEmpty)
        #expect(await session.clearCount == 0)
        #expect(commands.confirmClearToken == 0)

        commands.replaceState(state(summary: row, selected: true))
        await commands.perform(.clearTraffic)
        #expect(commands.confirmClearToken == 1)
        #expect(await session.clearCount == 0)

        await commands.perform(.deleteSelection)
        #expect(await session.deletedIDs == ["a"])
        #expect(traffic.selectedID == nil)
    }

    // MARK: - Rules

    @Test("new, duplicate, and delete follow the selected rule")
    func ruleCommands() {
        let idle = state(section: .rules)
        #expect(idle.canNewRule)
        #expect(!idle.canDuplicateRule)
        #expect(!idle.canDeleteRule)
        #expect(idle.deleteTitle == "Delete Rule")

        let selected = state(section: .rules, rule: true)
        #expect(selected.canDuplicateRule)
        #expect(selected.canDeleteRule)
        #expect(AppCommand.duplicateRule.isEnabled(selected))

        let editing = state(section: .rules, editor: true, rule: true)
        #expect(!editing.canNewRule)
        #expect(!editing.canDuplicateRule)
        #expect(!editing.canDeleteRule)

        let choosing = state(section: .rules, choosing: true, rule: true)
        #expect(!choosing.canDeleteRule)
        #expect(!AppCommand.newRule.isEnabled(state(section: .traffic)))
    }

    @Test("delete rule asks for confirmation and does not delete yet")
    func deleteRuleConfirms() async {
        let session = FakeRulesSession()
        await session.answerList(with: [RuleFixtures.rule(id: "rule-a", name: "Block ads")])
        let rules = RulesModel(session: session, chooser: FakeFileChooser())
        await rules.load()
        rules.selectedID = "rule-a"

        let commands = router(rules: rules)
        commands.replaceState(state(section: .rules, choosing: true, rule: true))
        await commands.perform(.deleteSelection)
        #expect(rules.deleting == nil)

        commands.replaceState(state(section: .rules, rule: true))
        await commands.perform(.deleteSelection)
        #expect(rules.deleting?.id == "rule-a")
        #expect(await session.calls.contains(.delete("rule-a")) == false)

        _ = await rules.confirmDelete()
        #expect(await session.calls.contains(.delete("rule-a")))
    }

    @Test("duplicate rule opens the existing editor")
    func duplicateRule() async {
        let session = FakeRulesSession()
        await session.answerList(with: [RuleFixtures.rule(id: "rule-a", name: "Block ads")])
        let rules = RulesModel(session: session, chooser: FakeFileChooser())
        await rules.load()
        rules.selectedID = "rule-a"

        let commands = router(rules: rules)
        commands.replaceState(state(section: .rules, rule: true))
        await commands.perform(.duplicateRule)

        #expect(rules.editor != nil)
        #expect(rules.editor?.title == "Duplicate Rule")
    }

    @Test("new rule is a request the rules screen handles")
    func newRuleRequestsTheChooser() async {
        let commands = router()
        commands.replaceState(state(section: .traffic))
        await commands.perform(.newRule)
        #expect(commands.newRuleToken == 0)

        commands.replaceState(state(section: .rules))
        await commands.perform(.newRule)
        #expect(commands.newRuleToken == 1)
    }

    // MARK: - Navigation

    @Test("the section commands switch the window")
    func navigation() async {
        let commands = router()
        await commands.perform(.showRules)
        #expect(commands.section == .rules)
        await commands.perform(.showBreaks)
        #expect(commands.section == .breakpoints)
        await commands.perform(.showTraffic)
        #expect(commands.section == .traffic)
        #expect(AppCommand.showTraffic.isEnabled(state(section: .rules)))
    }
}

@Suite("Empty states")
struct EmptyStateCopyTests {
    @Test("traffic, rules, breakpoints, saved requests, and domains say when they are empty")
    func words() {
        #expect(TrafficEmptyCopy.content(.noTraffic)?.title == "No traffic yet")
        #expect(TrafficEmptyCopy.content(.noTraffic)?.action == nil)
        #expect(TrafficEmptyCopy.content(.noMatches)?.title == "No traffic matches these filters")
        #expect(TrafficEmptyCopy.content(.noMatches)?.action == .clearFilters)
        #expect(TrafficEmptyCopy.content(.noMatches)?.action?.title == "Clear Filters")
        #expect(TrafficEmptyCopy.content(.noHostTraffic("api.example.com"))?.title == "No traffic for api.example.com")
        #expect(TrafficEmptyCopy.content(.rows) == nil)
        #expect(TrafficEmptyCopy.content(.loading) == nil)

        let disconnected = TrafficEmptyCopy.content(.disconnected(IPCError.cannotConnect(path: "/tmp/proxylab.sock").message))
        #expect(disconnected?.title == "Not connected to the core")
        #expect(disconnected?.detail.contains("/tmp/") == false)
        #expect(disconnected?.action == .retryConnection)

        #expect(RulesEmptyCopy.forPlaceholder(.noRules)?.title == "No rules yet")
        #expect(RulesEmptyCopy.forPlaceholder(.noMatches)?.title == "No matching rules")
        #expect(BreakpointEmptyCopy.title == "No breakpoints")
        #expect(SavedEmptyCopy.title == "No saved requests")
        #expect(SavedEmptyCopy.detail.contains("Save Request"))
        #expect(DomainEmptyCopy.noneCaptured == "No domains captured yet")
        #expect(DomainEmptyCopy.noMatches == "No domains match.")
    }
}

@Suite("Status wording")
struct UserFacingNoticeTests {
    @Test("a short sentence stays, and a path or status code goes behind details")
    func splits() {
        let plain = UserFacingNotice("A saved request must have a name.")
        #expect(plain.summary == "A saved request must have a name.")
        #expect(plain.detail == nil)

        let path = UserFacingNotice(IPCError.cannotConnect(path: "/tmp/proxylab.sock").message)
        #expect(path.summary == "Could not reach the ProxyLab core.")
        #expect(path.detail?.contains("/tmp/proxylab.sock") == true)

        let status = UserFacingNotice("OSStatus -60005")
        #expect(status.summary == "Something went wrong.")
        #expect(status.detail == "OSStatus -60005")

        let written = UserFacingNotice("The body could not be written: /Users/ada/body.bin")
        #expect(written.summary == "The body could not be written.")
        #expect(written.detail?.contains("/Users/ada/body.bin") == true)
    }

    @Test("capture, sources, and the connection share one tone")
    func tones() {
        #expect(CaptureTint.active.tone == .ready)
        #expect(CaptureTint.pending.tone == .pending)
        #expect(CaptureTint.fault.tone == .fault)
        #expect(CaptureTint.idle.tone == .idle)
        #expect(CaptureSourceTone.ready.tone == StatusTone.ready)
        #expect(CaptureSourceTone.attention.tone == StatusTone.pending)
        #expect(CaptureSourceTone.fault.tone == StatusTone.fault)
        #expect(CaptureSourceTone.idle.tone == StatusTone.idle)
    }
}
