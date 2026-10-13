//  What a traffic row's context menu can do with one exchange.
//
//      Replay Request
//      Copy as cURL
//      Save Request
//      ──────────────
//      Breakpoint…
//      ──────────────
//      Apply Rule
//      ──────────────
//      Pin Domain
//      Filter by Host
//      Filter by Method
//      Filter by Status
//      ──────────────
//      Delete Exchange
//
//  Availability is decided here. The actions are the ones the toolbar, the
//  detail pane, the filter bar, and the pinned-domain list already perform:
//  nothing in this menu is a second replay, a second clipboard command, a
//  second saved-request store, or a second filter.

import Foundation

/// The context menu for one traffic row, and the calls it makes.
struct TrafficRowMenu: Equatable, Sendable {
    /// The exchange the menu was opened on.
    let summary: ExchangeSummary

    /// Whether Replay Request can open the replay sheet.
    ///
    /// A WebSocket handshake and an event stream are not requests that sheet
    /// sends. HTTPS, HTTP/2, and a failed exchange still are: the request was
    /// captured, and the sheet is the same one Edit & Replay opens.
    let canReplay: Bool
    /// Whether Copy as cURL can ask the core for the command.
    ///
    /// The same exchanges Replay Request can open. The command, when it is
    /// asked for, is the core's.
    let canCopyCurl: Bool
    /// Whether Breakpoint… can open the sheet.
    ///
    /// The sheet configures a breakpoint for the next match. It does not
    /// change this exchange. A WebSocket handshake and an event stream can
    /// open it too: the breakpoint is still a request or a response pause,
    /// which is the one the core already has, and a text-message pause is
    /// left as it is.
    let canBreakpoint: Bool
    /// Whether Save Request can open the naming prompt.
    ///
    /// The saved-request list does not record which exchange a request came
    /// from, so this does not become "Saved". Saving again keeps another
    /// request, which is what the toolbar already does.
    let canSave: Bool
    /// "Pin Domain" or "Unpin Domain", when the row has a destination host.
    let domainActionTitle: String?
    /// The host the core's host filter compares, when the row has one.
    let host: String?
    /// Whether Filter by Method has a method to set.
    let canFilterMethod: Bool
    /// The method Filter by Method would set.
    let method: String
    /// The status class Filter by Status would set, when the row has a status.
    let statusClass: StatusClass?
    /// Whether Delete Exchange is offered. Deletion already exists.
    let canDelete: Bool

    init(summary: ExchangeSummary, isPinned: Bool) {
        self.summary = summary
        let sendable = Self.canSendAgain(summary)
        canReplay = sendable
        canCopyCurl = sendable
        canBreakpoint = !summary.method.isEmpty || !summary.url.isEmpty
        canSave = true
        host = TrafficDisplay.filterHost(summary)
        domainActionTitle = host.map { _ in isPinned ? "Unpin Domain" : "Pin Domain" }
        method = summary.method
        canFilterMethod = !summary.method.isEmpty
        if let status = summary.status {
            statusClass = StatusClass(rawValue: Int(status) / 100)
        } else {
            statusClass = nil
        }
        canDelete = true
    }

    /// Whether the pin and filter items have anything to show.
    var showsNarrowing: Bool {
        domainActionTitle != nil || canFilterMethod || statusClass != nil
    }

    /// Opens the replay sheet on this exchange, when it can be replayed.
    @MainActor
    func replay(using model: TrafficInspectorModel) async {
        guard canReplay else { return }

        await model.editAsRequest(summary.id)
    }

    /// Copies this exchange as cURL, when that command is offered.
    @MainActor
    func copyCurl(using model: TrafficInspectorModel) async {
        guard canCopyCurl else { return }

        await model.copyAsCurl(summary.id)
    }

    /// Opens the breakpoint sheet on this exchange.
    ///
    /// The same sheet the inspector and the Traffic menu open. The exchange
    /// is finished; the breakpoint waits for the next match.
    @MainActor
    func beginBreakpoint(using breakpoints: BreakpointsModel) {
        guard canBreakpoint else { return }

        breakpoints.beginBreakpoint(from: summary)
    }

    /// Asks what to call this request, through the saved-request prompt.
    @MainActor
    func save(using model: TrafficInspectorModel, saved: SavedRequestsModel) {
        guard canSave else { return }

        saved.beginSave(
            exchange: summary.id,
            suggestedName: model.suggestedSaveName(for: summary)
        )
    }

    /// Pins the destination host, or unpins it when it is already pinned.
    @MainActor
    func pin(using pinned: PinnedDomainsStore) {
        guard let host else { return }

        pinned.toggle(host)
    }

    /// Narrows the list to this destination host.
    ///
    /// The same host filter the domain sidebar sets. Search, method, status,
    /// content type, HTTP version, WebSocket, SSE, failed, and sort stay as
    /// they are.
    @MainActor
    func filterHost(using model: TrafficInspectorModel) async {
        guard let host else { return }

        await model.setHost(host)
    }

    /// Narrows the list to this method, leaving every other filter as it is.
    @MainActor
    func filterMethod(using model: TrafficInspectorModel) async {
        guard canFilterMethod else { return }

        await model.setMethod(method)
    }

    /// Narrows the list to this status class.
    ///
    /// An exact status code already set is cleared, because that is the same
    /// status filter and an exact code would hide the class. Failed, search,
    /// and the rest are left set.
    @MainActor
    func filterStatus(using model: TrafficInspectorModel) async {
        guard let statusClass else { return }

        if model.filters.statusCode != nil {
            await model.setStatusCode(nil)
        }
        await model.setStatus(statusClass)
    }

    /// Removes this exchange, once the core says it is gone.
    @MainActor
    func delete(using model: TrafficInspectorModel) async {
        guard canDelete else { return }

        await model.deleteExchange(summary.id)
    }

    /// Whether this exchange is a request the replay sheet and Copy as cURL send.
    ///
    /// The list summary has no `Upgrade` header. Status 101 is the signal the
    /// WebSocket filter already uses, and a handshake is a connection rather
    /// than a request to send again. An event stream is the same: the
    /// response does not end, and the SSE filter already recognises it by
    /// content type.
    private static func canSendAgain(_ summary: ExchangeSummary) -> Bool {
        if summary.status == 101 { return false }

        let type = summary.responseContentType ?? ""
        let essence = type.split(separator: ";", maxSplits: 1).first.map(String.init) ?? type
        let media = essence.trimmingCharacters(in: .whitespaces).lowercased()

        return !media.hasPrefix("text/event-stream")
    }
}
