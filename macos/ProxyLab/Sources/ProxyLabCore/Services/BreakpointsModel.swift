//  What the Breakpoints screen is showing, and how it got there.
//
//      open Breakpoints ──▶ list_breakpoints ──▶ the configured breakpoints
//                      └──▶ watch_breakpoints ──▶ every exchange held still
//                      └──▶ throttle_status   ──▶ how traffic is being shaped
//
//  # Watching is what makes the core pause
//
//  `watch_breakpoints` is not a convenience: a core nobody is watching holds
//  nothing still, and the connection going away releases everything it was
//  holding. So this model asks to watch when it loads, and the app quitting,
//  crashing, or losing its socket cannot strand a request — there is no
//  timeout here to get right and no state to reconcile.
//
//  # Several pauses, one of them on screen
//
//  Pauses arrive as events and are kept in the order they arrived. The editor
//  shows one of them. The first to arrive is that one until a person picks
//  another; a later arrival does not replace it. A pause the core releases —
//  because it timed out, or because another window answered it — is taken off
//  the queue when the `breakpoint_released` event says so, so the editor never
//  offers a decision about something that is no longer being held.
//
//  # Nothing changes on screen before the core says it changed
//
//  Turning a breakpoint off, deleting one, and changing the throttling all go
//  to the core first, for the reason the Rules screen does the same: a switch
//  that flips and flips back is worse than one that takes a moment.

import Foundation
import Observation

/// What the breakpoints list is doing.
public enum BreakpointsListState: Equatable, Sendable {
    /// The list has not arrived yet.
    case loading
    /// The list is what the core has.
    case loaded
    /// It could not be fetched.
    case failed(String)
}

/// Everything the Breakpoints screen shows.
@MainActor
@Observable
public final class BreakpointsModel {
    /// What the list is doing.
    public private(set) var state: BreakpointsListState = .loading
    /// The breakpoints, in the order the core consults them in.
    public private(set) var breakpoints: [Breakpoint] = []
    /// Whether the core is sending this app the exchanges it holds still.
    public private(set) var watching = false
    /// How the core is shaping traffic.
    public private(set) var throttle: ThrottleSettings = .none
    /// The exchanges being held right now, oldest first.
    public private(set) var paused: [PausedExchange] = []
    /// Which held exchange the editor is showing, when one has been chosen.
    private var selectedPauseID: String?
    /// The edits for each held exchange, so choosing another one does not
    /// throw away what was already typed.
    private var editors: [String: PauseEditorModel] = [:]
    /// Whether the traffic detail should show the held exchange.
    ///
    /// Set when the first pause of a quiet queue arrives, and when a person
    /// asks to see one. A later pause does not set it again.
    private(set) var showingHold = false
    /// The breakpoint being written from a traffic row, if that sheet is open.
    var trafficDraft: TrafficBreakpointDraft?
    /// The last thing that went wrong, for the status bar.
    public private(set) var lastError: String?
    /// The last thing that happened, for the status bar.
    public private(set) var lastAction: String?
    /// Which breakpoints have a change in flight, so their controls can be
    /// disabled rather than clicked twice.
    public private(set) var busyIDs: Set<String> = []

    private let session: any BreakpointsSession

    /// Which load is the current one, so a slow answer to a superseded load
    /// does not replace the list a newer one produced.
    private var loadGeneration = 0

    public init(session: any BreakpointsSession) {
        self.session = session
    }

    /// The pause a person is being asked about, if there is one.
    ///
    /// The one they selected, or the oldest when they have not selected.
    public var current: PausedExchange? {
        if let selectedPauseID, let selected = paused.first(where: { $0.pauseID == selectedPauseID }) {
            return selected
        }

        return paused.first
    }

    /// The edits for the pause on screen, if there is one.
    public var editor: PauseEditorModel? {
        guard let pauseID = current?.pauseID else { return nil }

        return editors[pauseID]
    }

    /// How many exchanges are waiting behind the one on screen.
    public var queuedCount: Int { max(paused.count - (current == nil ? 0 : 1), 0) }

    /// How many breakpoints are enabled.
    public var enabledCount: Int { breakpoints.count { $0.enabled } }

    // MARK: - Loading

    /// Asks the core for the breakpoints, the throttling, and to be told
    /// about pauses.
    ///
    /// Watching is asked for last and separately: a failure to load the list
    /// should not leave the app watching without showing what it is watching
    /// for, and a failure to watch is worth saying out loud because it means
    /// nothing will ever pause.
    public func load() async {
        loadGeneration += 1
        let generation = loadGeneration

        if breakpoints.isEmpty { state = .loading }

        do {
            let breakpoints = try await session.listBreakpoints()
            let throttle = try await session.throttleStatus()
            guard generation == loadGeneration else { return }

            self.breakpoints = breakpoints
            self.throttle = throttle
            state = .loaded
            lastError = nil
        } catch {
            guard generation == loadGeneration else { return }

            let failure = message(for: error)
            state = .failed(failure)
            lastError = failure

            return
        }

        await startWatching()
    }

    /// Asks the core to send this app every exchange a breakpoint holds.
    ///
    /// Idempotent from this model's side: asking twice on one connection is
    /// refused by the core, so it is asked once and again only after a
    /// reconnection.
    public func startWatching() async {
        guard !watching else { return }

        do {
            try await session.watchBreakpoints()
            watching = true
        } catch {
            watching = false
            lastError = "Breakpoints will not pause anything: \(message(for: error))"
        }
    }

    /// Forgets that this app is watching, after the connection was lost.
    ///
    /// Everything the core was holding for this app has already been released
    /// by the disconnection itself, so the queue goes with it.
    public func connectionLost() {
        watching = false
        paused = []
        editors = [:]
        selectedPauseID = nil
        showingHold = false
    }

    // MARK: - Events

    /// Takes in one exchange the core has just paused.
    ///
    /// The first pause of an empty queue is the one on screen. A pause that
    /// arrives while another is already showing is kept, and left off screen
    /// until it is chosen.
    public func received(_ exchange: PausedExchange) {
        guard !paused.contains(where: { $0.pauseID == exchange.pauseID }) else { return }

        let first = paused.isEmpty
        paused.append(exchange)
        if first {
            show(exchange)
            showingHold = true
        }
    }

    /// Shows `pauseID` and keeps every other held exchange.
    func selectPause(_ pauseID: String) {
        guard let exchange = paused.first(where: { $0.pauseID == pauseID }) else { return }

        show(exchange)
        showingHold = true
    }

    /// Leaves the traffic detail on the selected exchange.
    ///
    /// The pause stays held. The Breakpoints screen still has the editor.
    func dismissHoldInspector() {
        showingHold = false
    }

    /// Takes one exchange off the queue, because the core is not holding it
    /// any more.
    public func released(pauseID: String, reason: ReleaseReason) {
        let wasCurrent = current?.pauseID == pauseID
        paused.removeAll { $0.pauseID == pauseID }
        editors[pauseID] = nil

        if paused.isEmpty {
            selectedPauseID = nil
            showingHold = false
        } else if wasCurrent, let next = paused.first {
            show(next)
        }

        if wasCurrent {
            switch reason {
            case .continued, .cancelled:
                break
            case .abandoned:
                lastAction = "That exchange was released: nothing was left watching it."
            case .timedOut:
                lastAction = "That exchange was released: nothing decided in time."
            }
        }
    }

    /// Remembers an editor for `exchange` and makes it the one on screen.
    private func show(_ exchange: PausedExchange) {
        if editors[exchange.pauseID] == nil {
            editors[exchange.pauseID] = PauseEditorModel(exchange)
        }
        selectedPauseID = exchange.pauseID
    }

    // MARK: - Deciding

    /// Lets the exchange on screen carry on, with whatever was edited.
    @discardableResult
    public func continuePause() async -> Bool {
        guard let editor, let current else { return false }

        guard let edits = editor.edits() else {
            lastError = editor.refusal
            return false
        }

        return await decide(current.pauseID) { [session] in
            try await session.continueBreakpoint(
                pauseID: current.pauseID,
                request: edits.request,
                response: edits.response,
                websocket: edits.websocket
            )
        }
    }

    /// Lets the exchange on screen carry on as it was captured.
    ///
    /// Whatever is typed in the editor is left unsent. A field the core is
    /// not given stays as it was, which is also how a body too long to edit
    /// is forwarded in full.
    @discardableResult
    public func continueUnchanged() async -> Bool {
        guard let current else { return false }

        return await decide(current.pauseID) { [session] in
            try await session.continueBreakpoint(
                pauseID: current.pauseID,
                request: nil,
                response: nil,
                websocket: nil
            )
        }
    }

    /// Stops the exchange on screen: nothing is sent, and nothing delivered.
    @discardableResult
    public func cancelPause() async -> Bool {
        guard let current else { return false }

        return await decide(current.pauseID) { [session] in
            try await session.cancelBreakpoint(pauseID: current.pauseID)
        }
    }

    /// Sends one decision and takes the pause off the queue either way.
    ///
    /// The `breakpoint_released` event does the same thing a moment later, and
    /// removing it here too is what keeps the button from being clickable
    /// twice while the answer is in flight. Both are idempotent.
    private func decide(
        _ pauseID: String,
        _ send: @escaping () async throws -> PauseResolvedResult
    ) async -> Bool {
        guard !busyIDs.contains(pauseID) else { return false }

        busyIDs.insert(pauseID)
        defer { busyIDs.remove(pauseID) }

        do {
            let result = try await send()
            released(pauseID: pauseID, reason: .continued)
            lastError = nil
            if !result.resolved {
                lastAction = "That exchange was no longer being held."
            }

            return true
        } catch {
            lastError = message(for: error)

            return false
        }
    }

    // MARK: - Configuring

    /// Configures a breakpoint on `direction` for what `matcher` describes.
    @discardableResult
    public func create(direction: BreakpointDirection, matcher: RuleMatcher) async -> Bool {
        do {
            _ = try await session.createBreakpoint(
                NewBreakpoint(direction: direction, matcher: matcher)
            )
            await load()
            lastError = nil

            return true
        } catch {
            lastError = message(for: error)

            return false
        }
    }

    /// Opens the sheet that writes a breakpoint from one captured exchange.
    ///
    /// The exchange itself is left alone. The draft matches the next request
    /// that looks like it.
    func beginBreakpoint(from summary: ExchangeSummary) {
        trafficDraft = TrafficBreakpointDraft.from(summary: summary)
    }

    /// Drops the traffic sheet without configuring anything.
    func cancelTrafficDraft() {
        trafficDraft = nil
    }

    /// Configures the draft, one breakpoint per direction, through the same
    /// create the Breakpoints screen uses.
    ///
    /// Request and response are two breakpoints because a breakpoint has one
    /// direction. Both stay until they are disabled or deleted.
    @discardableResult
    func createTrafficDraft() async -> Bool {
        guard let draft = trafficDraft else { return false }

        if let refusal = draft.refusal {
            lastError = refusal

            return false
        }

        let matcher = draft.matcher
        let directions = draft.directions

        do {
            for direction in directions {
                _ = try await session.createBreakpoint(
                    NewBreakpoint(direction: direction, matcher: matcher)
                )
            }
            trafficDraft = nil
            await load()
            lastError = nil
            lastAction = directions.count > 1
                ? "The next matching request will pause, and then its response can."
                : "The next matching \(directions[0].title.lowercased()) will pause."

            return true
        } catch {
            await load()
            lastError = message(for: error)

            return false
        }
    }

    /// Turns `breakpoint` on or off, and shows the state the core confirmed.
    public func setEnabled(_ enabled: Bool, for breakpoint: Breakpoint) async {
        guard !busyIDs.contains(breakpoint.id) else { return }

        busyIDs.insert(breakpoint.id)
        defer { busyIDs.remove(breakpoint.id) }

        do {
            let updated = try await session.updateBreakpoint(
                id: breakpoint.id,
                .enabled(enabled)
            )
            replace(updated)
            lastError = nil
        } catch let error as IPCError where error.serverCode == .notFound {
            await load()
            lastError = "That breakpoint no longer exists."
        } catch {
            lastError = message(for: error)
        }
    }

    /// Removes one breakpoint, leaving anything it is already holding alone.
    @discardableResult
    public func delete(_ breakpoint: Breakpoint) async -> Bool {
        guard !busyIDs.contains(breakpoint.id) else { return false }

        busyIDs.insert(breakpoint.id)
        defer { busyIDs.remove(breakpoint.id) }

        do {
            let result = try await session.deleteBreakpoint(id: breakpoint.id)
            await load()
            lastError = result.deleted ? nil : "That breakpoint no longer existed."

            return true
        } catch {
            lastError = message(for: error)

            return false
        }
    }

    // MARK: - Throttling

    /// Shapes traffic the way `settings` says, from the next exchange on.
    @discardableResult
    public func applyThrottle(_ settings: ThrottleSettings) async -> Bool {
        if let refusal = settings.refusal {
            lastError = refusal

            return false
        }

        do {
            throttle = try await session.setThrottle(settings)
            lastError = nil
            lastAction = throttle.isShaping
                ? "Throttling is on."
                : "Throttling is off."

            return true
        } catch {
            lastError = message(for: error)

            return false
        }
    }

    /// Turns throttling on or off without changing the numbers.
    public func setThrottleEnabled(_ enabled: Bool) async {
        var settings = throttle
        settings.enabled = enabled

        await applyThrottle(settings)
    }

    // MARK: - Housekeeping

    /// Puts one changed breakpoint back in the list where it already sits.
    private func replace(_ breakpoint: Breakpoint) {
        guard let index = breakpoints.firstIndex(where: { $0.id == breakpoint.id }) else { return }

        breakpoints[index] = breakpoint
    }

    /// What a person is told about `error`.
    private func message(for error: any Error) -> String {
        (error as? IPCError)?.message ?? "Something went wrong talking to the ProxyLab core."
    }
}
