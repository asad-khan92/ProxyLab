//  What the window is showing, and how it got there.
//
//      connect ──▶ subscribe ──▶ list the first page of the current query
//                      │
//                      └──▶ traffic_added ──▶ one row, if the query wants it
//
//  Subscribing comes before listing on purpose. An exchange that finishes
//  between the two arrives as an event and is merged into the page when it
//  lands; an exchange that finished before either is in the page. Listing
//  first would leave a gap where an exchange is in neither.
//
//  # The query lives here
//
//  Search, filters, and sort are one value — `TrafficFilters` — held in one
//  place, and the core does the narrowing. The list is a page of an indexed
//  query rather than everything fetched and then sieved, which is the
//  difference between a filter that works on a hundred thousand exchanges and
//  one that works on whatever happens to be loaded.
//
//  # A row at a time, not a list at a time
//
//  An event carries the whole row, so a new exchange is inserted where it
//  belongs rather than being the reason to ask for the list again. The same
//  filters are evaluated against it locally, so a row that the current query
//  excludes never appears. The list is re-fetched for exactly two things: the
//  query changing, and `traffic_lost` — which says rows were dropped for this
//  subscriber and the list it holds is now wrong.
//
//  # cURL and replay
//
//  Copying a request out as cURL asks the core for the command: the core
//  stored the request and is what knows what it was, so nothing here
//  assembles one out of the parts of a row. Sending a request again asks the
//  core to send it, and what comes back is an exchange id and a status.
//
//  A replay does not change what is selected. The new exchange arrives on the
//  traffic subscription like any other and takes its place in the list; the
//  detail pane goes on showing whatever was being looked at. Selecting the
//  new row here would mean asking for an exchange the storage worker may not
//  have written yet, and answering that race with a spinner or a wrong
//  "this exchange is gone" is worse than letting the row appear and be
//  clicked.
//
//  # Nothing is removed before the core says it was
//
//  Delete and clear change what is shown only after a successful response.
//  An optimistic removal would need somewhere to put the row back, and a
//  debugging tool that loses a request because a delete failed is worse than
//  one that takes a moment.

import Foundation
import Observation

/// Whether the app is talking to the core.
public enum ConnectionState: Equatable, Sendable {
    /// Nothing has been tried yet.
    case idle
    /// A connection is being opened.
    case connecting
    /// Connected, and subscribed to traffic events.
    case connected
    /// Not connected, with what went wrong when something did.
    case disconnected(String?)
}

/// What the traffic list is doing.
public enum TrafficListState: Equatable, Sendable {
    /// The first page has not arrived yet.
    case loading
    /// The list is what the core has.
    case loaded
    /// The list could not be fetched.
    case failed(String)
}

/// What a list with no rows in it should say.
///
/// An empty list means one of several quite different things, and a developer
/// staring at a proxy that is showing nothing needs to know which. Computed
/// here rather than worked out in a view, so it is one decision and it is
/// testable as one.
public enum TrafficListPlaceholder: Equatable, Sendable {
    /// There are rows; the list is showing them.
    case rows
    /// The first page has not arrived yet.
    case loading
    /// There is no connection to ask.
    case disconnected(String?)
    /// The list could not be fetched.
    case failed(String)
    /// The core has carried nothing since its history was last cleared.
    case noTraffic
    /// The core has traffic, and none of it matches the current query.
    case noMatches
    /// The list is narrowed to this host alone, and that host has no traffic.
    ///
    /// Its own case rather than ``noMatches`` because it is its own answer: a
    /// pinned domain that is quiet is not a filter that went wrong, and the
    /// thing to offer is "show everything" rather than "clear your filters".
    case noHostTraffic(String)
}

/// What the selected stream message's payload pane is showing.
public enum StreamMessageDetailState: Equatable, Sendable {
    /// Nothing is selected.
    case none
    /// The selected message is being fetched.
    case loading(id: String)
    /// The selected message, payload included.
    case loaded(StreamMessage)
    /// The selected message is not there any more.
    case unavailable
    /// It could not be fetched.
    case failed(String)
}

/// What the detail pane is showing.
public enum ExchangeDetailState: Equatable, Sendable {
    /// Nothing is selected.
    case none
    /// The selected exchange is being fetched.
    case loading(id: String)
    /// The selected exchange, whole.
    case loaded(TrafficExchange)
    /// The selected exchange is not there any more.
    case unavailable
    /// It could not be fetched.
    case failed(String)
}

/// How long to wait before trying the core again.
///
/// A fixed, increasing sequence, and never faster than the first entry: a
/// developer who has not started the core yet should not have an app hammering
/// a socket that is not there. The last delay repeats for as long as the core
/// stays away.
public struct ReconnectPolicy: Sendable {
    /// The delay before each attempt, in order.
    public let delays: [Duration]

    public init(delays: [Duration]) {
        self.delays = delays
    }

    /// One second, then two, five, ten, and thirty from then on.
    public static let standard = ReconnectPolicy(
        delays: [.seconds(1), .seconds(2), .seconds(5), .seconds(10), .seconds(30)]
    )

    /// No automatic reconnection at all.
    public static let manual = ReconnectPolicy(delays: [])

    /// How long to wait before attempt number `attempt`, counting from zero.
    func delay(forAttempt attempt: Int) -> Duration? {
        guard let last = delays.last else { return nil }

        return attempt < delays.count ? delays[attempt] : last
    }
}

/// Everything the traffic inspector shows.
@MainActor
@Observable
public final class TrafficInspectorModel {
    /// How many exchanges one page of the list holds.
    ///
    /// The core's own maximum is a thousand; a page this size fills a window
    /// several times over, arrives as one message, and leaves room to page on
    /// without the first request being the expensive one.
    public static let pageSize = 200

    /// How long typing settles before the core is asked.
    ///
    /// Long enough that a word typed at speed is one request and not eight,
    /// short enough that the list does not feel detached from the field.
    public static let searchSettlingTime = Duration.milliseconds(250)

    /// Whether the app is talking to the core.
    public private(set) var connection: ConnectionState = .idle
    /// What the traffic list is doing.
    public private(set) var listState: TrafficListState = .loading
    /// The exchanges, in the order the current sort asks for.
    public private(set) var exchanges: [ExchangeSummary] = []
    /// How many the core has that match the current filters.
    public private(set) var totalExchanges: UInt64 = 0
    /// The exchange the detail pane is showing.
    public private(set) var selectedID: String?
    /// What the detail pane is showing.
    public private(set) var detail: ExchangeDetailState = .none
    /// The selected exchange's bodies, read once when it was fetched.
    ///
    /// Deciding what a body is — decoding it, laying JSON out — is the
    /// expensive half of showing an exchange, and it is done away from this
    /// actor when the exchange arrives rather than again on every redraw.
    /// Always about ``detail``'s own exchange: the two are set together.
    private(set) var inspection: ExchangeInspection?
    /// Stream messages on the selected exchange, oldest first.
    ///
    /// Empty when the selection is ordinary HTTP. A WebSocket or event stream
    /// fills this from `list_stream_messages` and from `stream_message_added`
    /// events, so a live socket's timeline grows without asking for the
    /// exchange again.
    public private(set) var streamMessages: [StreamMessageSummary] = []
    /// The stream message the payload pane is showing.
    public private(set) var selectedStreamID: String?
    /// What the payload pane is showing.
    public private(set) var streamDetail: StreamMessageDetailState = .none
    /// The selected stream message's payload, read once when it was fetched.
    private(set) var streamInspection: BodyInspection?
    /// Whether the selected exchange has a page of stream messages beyond
    /// what is loaded.
    public private(set) var streamHasMore = false
    /// Whether a page of stream messages is being fetched right now.
    public private(set) var isLoadingStreams = false
    /// Why the stream timeline could not be fetched, when it could not.
    public private(set) var streamError: String?
    /// The last thing that went wrong, for the status bar.
    public private(set) var lastError: String?
    /// Whether the core has a page beyond the rows that are loaded.
    public private(set) var hasMoreTraffic = false
    /// Whether a page is being fetched right now.
    public private(set) var isLoadingMore = false
    /// The import sheet, while one is open.
    public private(set) var importer: CurlImportModel?
    /// What the last copy or replay came to, for the status bar.
    ///
    /// A confirmation and not an error: a failure goes to ``lastError`` like
    /// every other failure.
    public private(set) var lastAction: String?

    /// How the list is narrowed and ordered.
    ///
    /// Read by the interface and changed only through the methods below, so
    /// that a change is always followed by the reload it needs.
    public private(set) var filters = TrafficFilters.none

    /// The socket this model is connected through, for the interface to show.
    public let socketPath: String

    /// Where an exchange a breakpoint is holding is sent.
    ///
    /// There is one notification stream and one consumer of it — this model,
    /// which owns the connection — so the two notifications that are about a
    /// held exchange are handed on rather than read a second time. Weak
    /// because the window holds both models and neither owns the other.
    public weak var breakpoints: BreakpointsModel?

    private let session: any TrafficSession
    private let clipboard: any Clipboard
    /// Where Save Body writes. macOS's own panel, unless a test replaces it,
    /// so that a test run never asks for a panel and never leaves a file
    /// behind. Nothing is constructed until something is saved.
    var destination: any BodyDestination = SavePanelDestination()
    /// Where an export goes and which file an import reads. macOS's own
    /// panels, unless a test replaces them, so that a test run never asks for
    /// a panel and never leaves a file behind. Nothing is constructed until
    /// something is exported or imported.
    var harFiles: any HarFileChooser = PanelHarFileChooser()
    private let reconnect: ReconnectPolicy
    private let searchSettlingTime: Duration

    private var nextCursor: TrafficCursor?
    /// Sequence of the last loaded stream message, for the next page.
    private var streamNextAfter: UInt64?
    private var notificationTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var reconnectAttempt = 0

    /// Which query the pages being loaded belong to.
    ///
    /// Bumped whenever the filters or the sort change, so a page that was
    /// asked for under the old query and arrives under the new one is
    /// discarded rather than mixed into a list it does not belong to.
    private var queryGeneration = 0

    /// Which detail fetch is the current one, so a slow answer for a row that
    /// is no longer selected does not replace the row that is.
    private var detailRequest = 0
    /// The same, for the selected stream message's payload.
    private var streamDetailRequest = 0

    public init(
        session: any TrafficSession,
        socketPath: String,
        clipboard: (any Clipboard)? = nil,
        reconnect: ReconnectPolicy = .standard,
        searchSettlingTime: Duration = TrafficInspectorModel.searchSettlingTime
    ) {
        self.session = session
        self.socketPath = socketPath
        // Built here rather than as a default argument, so that a test can
        // say what was copied without a test run writing over whatever the
        // developer had on their clipboard.
        self.clipboard = clipboard ?? SystemClipboard()
        self.reconnect = reconnect
        self.searchSettlingTime = searchSettlingTime
    }

    // MARK: - What an empty list means

    /// What the list should say when it has no rows.
    public var placeholder: TrafficListPlaceholder {
        guard exchanges.isEmpty else { return .rows }

        switch listState {
        case .loading:
            return .loading
        case let .failed(reason):
            return .failed(reason)
        case .loaded:
            break
        }

        if case let .disconnected(reason) = connection { return .disconnected(reason) }

        // The core answered, with nothing. Whether that is an empty proxy, a
        // quiet domain, or an unlucky query is the difference between three
        // quite different things to do next.
        guard filters.isNarrowed else { return .noTraffic }

        if filters.isNarrowedOnlyByHost, let host = filters.host {
            return .noHostTraffic(host)
        }

        return .noMatches
    }

    /// Destination hosts this window has seen, for the sidebar and the host menu.
    ///
    /// Kept apart from ``exchanges`` so narrowing the list to one host does not
    /// hide the others. There is no host index to ask the core for: a host is
    /// remembered when a page that contains it is loaded, or when a
    /// `traffic_added` event names it.
    public private(set) var domains = TrafficDomainCatalog()

    /// The hosts the host menu and the domain list offer, in alphabetical order.
    ///
    /// The catalog, plus the host the list is filtered to when that host has
    /// not been seen in traffic yet — otherwise the only way out of a filter
    /// set by hand would be Clear.
    public var knownHosts: [String] {
        var hosts = domains.hosts
        if let host = filters.host, !domains.contains(host) {
            hosts.append(host)
            hosts.sort { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
        }

        return hosts
    }

    // MARK: - Lifecycle

    /// Starts consuming notifications and opens the connection.
    public func start() async {
        guard notificationTask == nil else { return }

        // Detached so the coalesce window sleeps off the main actor. A burst
        // of traffic and stream summaries becomes one update; anything else
        // is still applied as it arrives. `handle` stays immediate, which is
        // what the tests call.
        notificationTask = Task.detached { [weak self, notifications = session.notifications] in
            guard let model = self else { return }
            await pumpArrivals(notifications, window: arrivalCoalesceWindow) { delivery in
                await model.apply(delivery)
            }
        }

        await openConnection()
    }

    /// Stops reconnecting and closes the connection.
    public func stop() async {
        reconnectTask?.cancel()
        reconnectTask = nil
        notificationTask?.cancel()
        notificationTask = nil
        searchTask?.cancel()
        searchTask = nil

        await session.disconnect()
        connection = .disconnected(nil)
    }

    /// Tries the core again now, rather than after the next backoff delay.
    public func retryConnection() async {
        reconnectTask?.cancel()
        reconnectTask = nil
        reconnectAttempt = 0

        await openConnection()
    }

    /// Connects, subscribes, and loads the first page.
    private func openConnection() async {
        connection = .connecting
        listState = .loading

        do {
            try await session.connect()
            // Before the list: an exchange that finishes between the two
            // arrives as an event, and one that finishes after this line but
            // before the list is in the list.
            try await session.subscribe()
        } catch {
            let failure = message(for: error)
            connection = .disconnected(failure)
            listState = .failed(failure)
            scheduleReconnect()

            return
        }

        connection = .connected
        lastError = nil
        reconnectAttempt = 0

        await loadFirstPage()
    }

    /// Waits, then tries again, for as long as the policy says to.
    private func scheduleReconnect() {
        guard let delay = reconnect.delay(forAttempt: reconnectAttempt) else { return }

        reconnectAttempt += 1
        reconnectTask?.cancel()
        reconnectTask = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled else { return }

            await self?.openConnection()
        }
    }

    // MARK: - Search, filters, and sort

    /// Sets the search text, and asks the core once the typing settles.
    ///
    /// Every keystroke changes what the field shows immediately and cancels
    /// the request the last one scheduled, so a word typed at speed is one
    /// `list_traffic` and not eight.
    public func setSearch(_ text: String) {
        guard text != filters.search else { return }

        filters.search = text
        scheduleSearch()
    }

    /// Applies a search that is still settling, without waiting for it.
    ///
    /// What pressing Return does — and what a test does instead of sleeping.
    public func applySearchNow() async {
        guard searchTask != nil else { return }

        searchTask?.cancel()
        searchTask = nil

        await applyQueryChange()
    }

    /// Narrows to one method, or to every method when given `nil`.
    public func setMethod(_ method: String?) async {
        guard method != filters.method else { return }

        filters.method = method
        await applyQueryChange()
    }

    /// Narrows to one class of status, or to every status when given `nil`.
    public func setStatus(_ status: StatusClass?) async {
        guard status != filters.status else { return }

        filters.status = status
        await applyQueryChange()
    }

    /// Narrows to one status code exactly, or to every status when given `nil`.
    public func setStatusCode(_ code: UInt16?) async {
        guard code != filters.statusCode else { return }

        filters.statusCode = code
        await applyQueryChange()
    }

    /// Narrows to exchanges whose Content-Type contains `contentType`.
    public func setContentType(_ contentType: String?) async {
        guard contentType != filters.contentType else { return }

        filters.contentType = contentType
        await applyQueryChange()
    }

    /// Narrows to HTTP/1.1 or HTTP/2, or to every version when given `nil`.
    public func setHTTPVersion(_ version: HTTPVersion?) async {
        guard version != filters.httpVersion else { return }

        filters.httpVersion = version
        await applyQueryChange()
    }

    /// Narrows to WebSocket, SSE, or every kind.
    public func setStreamKind(_ kind: String?) async {
        let websocket = kind == "websocket"
        let sse = kind == "sse"
        guard websocket != filters.websocket || sse != filters.sse else { return }

        filters.websocket = websocket
        filters.sse = sse
        await applyQueryChange()
    }

    /// Keeps only exchanges that ended in a proxy error.
    public func setFailed(_ failed: Bool) async {
        guard failed != filters.failed else { return }

        filters.failed = failed
        await applyQueryChange()
    }

    /// Treats the search as a regular expression.
    public func setSearchRegex(_ enabled: Bool) async {
        guard enabled != filters.searchRegex else { return }

        filters.searchRegex = enabled
        await applyQueryChange()
    }

    /// Distinguishes letter case in the search.
    public func setSearchCaseSensitive(_ enabled: Bool) async {
        guard enabled != filters.searchCaseSensitive else { return }

        filters.searchCaseSensitive = enabled
        await applyQueryChange()
    }

    /// Narrows to one host, or to every host when given `nil`.
    public func setHost(_ host: String?) async {
        guard host != filters.host else { return }

        filters.host = host
        if let host { domains.observe(host) }
        await applyQueryChange()
    }

    /// Reads the same rows from the other end of the order.
    public func setSort(_ sort: TrafficSort) async {
        guard sort != filters.sort else { return }

        filters.sort = sort
        await applyQueryChange()
    }

    /// Clears the search and every filter, keeping the sort.
    public func clearFilters() async {
        guard filters.isNarrowed else { return }

        searchTask?.cancel()
        searchTask = nil
        filters = filters.cleared

        await applyQueryChange()
    }

    /// Schedules the settling delay after a keystroke.
    private func scheduleSearch() {
        searchTask?.cancel()
        searchTask = Task { [weak self, searchSettlingTime] in
            try? await Task.sleep(for: searchSettlingTime)
            guard !Task.isCancelled else { return }

            await self?.searchSettled()
        }
    }

    /// The typing stopped long enough; ask the core.
    private func searchSettled() async {
        searchTask = nil

        await applyQueryChange()
    }

    /// Reloads for a query that has changed, and tidies up the selection.
    ///
    /// A cursor names a place in the order it was produced in, so a changed
    /// query starts again from the first page rather than paging on from
    /// where the last one stopped.
    private func applyQueryChange() async {
        queryGeneration += 1

        await loadFirstPage()

        // A selected exchange the new query does not show would leave the
        // detail pane describing a row that is not on screen. The list and
        // the detail say the same thing or the detail says nothing.
        if let selectedID, !exchanges.contains(where: { $0.id == selectedID }) {
            self.selectedID = nil
            show(.none)
        }
    }

    // MARK: - The list

    /// Replaces the list with the first page of the current query.
    public func loadFirstPage() async {
        listState = .loading
        nextCursor = nil

        let generation = queryGeneration

        do {
            let page = try await session.listTraffic(
                filters.listParameters(limit: Self.pageSize)
            )
            // A page for a query that has since been replaced belongs to a
            // list that no longer exists.
            guard generation == queryGeneration else { return }

            exchanges = page.exchanges
            domains.observe(page.exchanges)
            totalExchanges = page.total
            nextCursor = page.nextCursor
            hasMoreTraffic = page.nextCursor != nil
            listState = .loaded

            // The selected row may have been deleted by something else while
            // the list was being fetched.
            if let selectedID, !exchanges.contains(where: { $0.id == selectedID }) {
                await refreshDetail()
            }
        } catch {
            guard generation == queryGeneration else { return }

            let failure = message(for: error)
            listState = .failed(failure)
            lastError = failure
        }
    }

    /// Adds the page after the last row that is loaded.
    ///
    /// Does nothing when there is no next page or one is already on its way:
    /// a list that asks for the same page twice shows it twice.
    public func loadNextPage() async {
        guard let cursor = nextCursor, !isLoadingMore else { return }

        let generation = queryGeneration
        isLoadingMore = true
        defer { isLoadingMore = false }

        do {
            let page = try await session.listTraffic(
                filters.listParameters(limit: Self.pageSize, cursor: cursor)
            )
            guard generation == queryGeneration else { return }

            // Merged rather than appended: a row already held is replaced
            // where it stands, so a page that overlaps what is loaded — an
            // exchange that arrived as an event while the page was in
            // flight — cannot appear twice.
            for summary in page.exchanges {
                insert(summary)
            }
            domains.observe(page.exchanges)

            totalExchanges = page.total
            nextCursor = page.nextCursor
            hasMoreTraffic = page.nextCursor != nil
        } catch {
            guard generation == queryGeneration else { return }

            lastError = message(for: error)
        }
    }

    /// Whether the row at `index` is close enough to the end to fetch more.
    ///
    /// The list asks this as rows appear. Fetching a page before the last row
    /// is reached is what keeps scrolling from stopping at a spinner.
    public func shouldLoadMore(atRow index: Int) -> Bool {
        guard hasMoreTraffic, !isLoadingMore else { return false }

        return index >= exchanges.count - 20
    }

    // MARK: - Selection

    /// Selects `id`, and fetches the whole exchange behind it.
    public func select(_ id: String?) async {
        guard id != selectedID else { return }

        selectedID = id
        await refreshDetail()
    }

    /// Fetches whatever is selected now.
    private func refreshDetail() async {
        guard let id = selectedID else {
            show(.none)
            return
        }

        detailRequest += 1
        let request = detailRequest
        show(.loading(id: id))

        do {
            let exchange = try await session.traffic(id: id)
            guard request == detailRequest else { return }

            // Read before the pane is told there is anything to draw, and
            // read off this actor: a megabyte of JSON is laid out while the
            // window is still showing the spinner, not while it is drawing.
            let inspection = await Task.detached(priority: .userInitiated) {
                BodyInspection.of(exchange)
            }.value
            guard request == detailRequest else { return }

            show(.loaded(exchange), inspection: inspection)

            if exchange.carriesStreamMessages {
                await loadStreamMessages(for: exchange.id, request: request)
            }
        } catch let error as IPCError where error.serverCode == .notFound {
            guard request == detailRequest else { return }

            // Deleted by something else, or cleared: not a failure of this
            // app's, and not something to show as one.
            show(.unavailable)
            exchanges.removeAll { $0.id == id }
        } catch {
            guard request == detailRequest else { return }

            show(.failed(message(for: error)))
        }
    }

    /// Puts `state` in the detail pane, with the bodies that belong to it.
    ///
    /// The one place both are written, so that what the pane is showing and
    /// what has been read of its bodies can never be about two different
    /// exchanges.
    private func show(_ state: ExchangeDetailState, inspection: ExchangeInspection? = nil) {
        detail = state
        self.inspection = inspection

        if case let .loaded(exchange) = state, exchange.carriesStreamMessages {
            return
        }

        resetStreams()
    }

    /// Clears the stream timeline so it cannot belong to a previous selection.
    private func resetStreams() {
        streamDetailRequest += 1
        streamMessages = []
        selectedStreamID = nil
        streamDetail = .none
        streamInspection = nil
        streamNextAfter = nil
        streamHasMore = false
        isLoadingStreams = false
        streamError = nil
    }

    // MARK: - Stream messages

    /// Selects `id` among the selected exchange's stream messages.
    public func selectStream(_ id: String?) async {
        guard id != selectedStreamID else { return }

        selectedStreamID = id
        await refreshStreamDetail()
    }

    /// Fetches the next page of stream messages, oldest first.
    public func loadMoreStreams() async {
        guard streamHasMore, !isLoadingStreams, let exchangeID = selectedID,
            let after = streamNextAfter
        else {
            return
        }

        isLoadingStreams = true
        defer { isLoadingStreams = false }

        do {
            let page = try await session.listStreamMessages(
                exchangeID: exchangeID,
                after: after,
                limit: Self.pageSize
            )

            for summary in page.messages {
                insertStream(summary)
            }
            streamNextAfter = page.nextAfter
            streamHasMore = page.nextAfter != nil
        } catch {
            lastError = message(for: error)
        }
    }

    /// Whether the stream row at `index` is close enough to the end to fetch more.
    public func shouldLoadMoreStreams(atRow index: Int) -> Bool {
        guard streamHasMore, !isLoadingStreams else { return false }

        return index >= streamMessages.count - 20
    }

    /// Puts the selected stream payload on the clipboard.
    func copySelectedStreamPayload() {
        guard let inspection = streamInspection, !inspection.isEmpty else { return }

        let label: String
        switch inspection.content {
        case .image, .binary, .undecodable:
            label = "the payload as Base64"
        case .empty, .text:
            label = "the payload"
        }

        copy(ExchangeText.body(inspection), as: label)
    }

    /// A page of the selected exchange's stream messages, oldest first.
    private func loadStreamMessages(for exchangeID: String, request: Int) async {
        isLoadingStreams = true
        streamError = nil

        do {
            let page = try await session.listStreamMessages(
                exchangeID: exchangeID,
                after: nil,
                limit: Self.pageSize
            )
            guard request == detailRequest else { return }

            streamMessages = page.messages
            streamNextAfter = page.nextAfter
            streamHasMore = page.nextAfter != nil
            isLoadingStreams = false

            if let first = page.messages.first {
                selectedStreamID = nil
                await selectStream(first.id)
            } else {
                selectedStreamID = nil
                streamDetail = .none
                streamInspection = nil
            }
        } catch {
            guard request == detailRequest else { return }

            isLoadingStreams = false
            streamError = message(for: error)
        }
    }

    /// Fetches whatever stream message is selected now.
    private func refreshStreamDetail() async {
        guard let id = selectedStreamID else {
            streamDetail = .none
            streamInspection = nil
            return
        }

        streamDetailRequest += 1
        let request = streamDetailRequest
        streamDetail = .loading(id: id)
        streamInspection = nil

        do {
            let message = try await session.streamMessage(id: id)
            guard request == streamDetailRequest else { return }

            let inspection = await Task.detached(priority: .userInitiated) {
                BodyInspection.ofStream(message.payload, kind: message.kind)
            }.value
            guard request == streamDetailRequest else { return }

            streamDetail = .loaded(message)
            streamInspection = inspection
        } catch let error as IPCError where error.serverCode == .notFound {
            guard request == streamDetailRequest else { return }

            streamDetail = .unavailable
        } catch {
            guard request == streamDetailRequest else { return }

            streamDetail = .failed(message(for: error))
        }
    }

    /// Takes in one stream message the proxy has just captured.
    private func addedStream(_ summary: StreamMessageSummary) {
        guard selectedID == summary.exchangeID else { return }

        insertStream(summary)
    }

    /// Puts one stream summary in sequence order, or replaces the one already there.
    private func insertStream(_ summary: StreamMessageSummary) {
        if let existing = streamMessages.firstIndex(where: { $0.id == summary.id }) {
            streamMessages[existing] = summary
            return
        }

        let position =
            streamMessages.firstIndex { $0.sequence > summary.sequence } ?? streamMessages.endIndex
        streamMessages.insert(summary, at: position)
    }

    // MARK: - Removing traffic

    /// Removes the selected exchange, once the core says it is gone.
    public func deleteSelected() async {
        guard let id = selectedID else { return }

        await deleteExchange(id)
    }

    /// Removes the exchange `id` names, once the core says it is gone.
    ///
    /// The same delete the toolbar uses. The detail pane is cleared only when
    /// the exchange it is showing is the one that went.
    func deleteExchange(_ id: String) async {
        do {
            _ = try await session.deleteTraffic(id: id)

            exchanges.removeAll { $0.id == id }
            totalExchanges = totalExchanges > 0 ? totalExchanges - 1 : 0
            if selectedID == id {
                selectedID = nil
                show(.none)
            }
            lastError = nil
        } catch {
            lastError = message(for: error)
        }
    }

    /// Removes every stored exchange, once the core says they are gone.
    public func clearAll() async {
        do {
            _ = try await session.clearTraffic()

            exchanges = []
            domains.clear()
            if let host = filters.host { domains.observe(host) }
            totalExchanges = 0
            nextCursor = nil
            hasMoreTraffic = false
            selectedID = nil
            show(.none)
            listState = .loaded
            lastError = nil
        } catch {
            lastError = message(for: error)
        }
    }

    // MARK: - Copying and saving what was captured

    /// Puts `text` on the clipboard, and says in the status bar what it was.
    ///
    /// Internal, and used only by the detail pane: the copies it offers are a
    /// URL, a header block, a query, and a body, and every one of them is
    /// text this app already has on screen. Nothing copied is logged.
    func copy(_ text: String, as label: String) {
        clipboard.copy(text)

        lastError = nil
        lastAction = "Copied \(label)."
    }

    /// Asks where to write a captured body, and writes it there.
    ///
    /// The bytes go across exactly as the core captured them. A cancelled
    /// panel leaves no file and says nothing: cancelling is an answer, not a
    /// failure. The path that was written is named in the status bar, and
    /// nothing about the body itself is.
    func saveBody(_ inspection: BodyInspection, url: String, half: ExchangeText.Half) {
        let name = ExchangeText.suggestedFilename(
            url: url,
            contentType: inspection.contentType,
            half: half
        )

        do {
            guard let written = try destination.save(inspection.capture.data, suggestedName: name)
            else {
                return
            }

            lastError = nil
            lastAction = inspection.capture.truncated
                ? "Saved to \(written.path) — the capture was truncated, so this is "
                    + "\(TrafficDisplay.size(UInt64(inspection.capture.capturedByteCount))) of "
                    + "\(TrafficDisplay.size(inspection.capture.size))."
                : "Saved to \(written.path)."
        } catch {
            lastAction = nil
            lastError = "The body could not be written: \(error.localizedDescription)"
        }
    }

    // MARK: - cURL and replay

    /// Copies the selected exchange's request to the clipboard, as cURL.
    ///
    /// The command is the core's: it is written from the request the core
    /// stored, headers, credentials, body and all, because the point of the
    /// command is to send the request again.
    public func copySelectedAsCurl() async {
        guard let id = selectedID else { return }

        await copyAsCurl(id)
    }

    /// Copies the exchange with `id` to the clipboard, as cURL.
    public func copyAsCurl(_ id: String) async {
        do {
            let generated = try await session.curl(forExchange: id)
            clipboard.copy(generated.curl)

            lastError = nil
            lastAction = generated.truncated
                ? "Copied as cURL — the captured body was truncated, so the command sends less "
                    + "than the request did."
                : "Copied as cURL."
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// Opens the import sheet, empty.
    public func beginImport() {
        lastAction = nil
        importer = CurlImportModel(session: session)
    }

    /// Opens the import sheet on the selected exchange's own request.
    ///
    /// The same command Copy as cURL produces, read back by the core, so the
    /// editor is filled in from the request the core stored rather than from
    /// the row this app happens to be holding.
    public func editSelectedAsRequest() async {
        guard let id = selectedID else { return }

        await editAsRequest(id)
    }

    /// Opens the replay sheet on the exchange `id` names.
    ///
    /// The same path Edit & Replay uses: the core writes the command, and the
    /// import sheet reads it back. Nothing here is selected or sent. A
    /// WebSocket handshake and an event stream are not offered this; the row
    /// menu disables them before calling.
    func editAsRequest(_ id: String) async {
        // Fetched before the sheet opens, so a failure is a message in the
        // status bar rather than an empty sheet with an explanation behind
        // it.
        let command: String
        do {
            command = try await session.curl(forExchange: id).curl
        } catch {
            lastAction = nil
            lastError = message(for: error)

            return
        }

        let importer = CurlImportModel(session: session)
        importer.curlText = command
        self.importer = importer
        lastAction = nil
        lastError = nil

        await importer.parse()
    }

    /// Closes the import sheet, keeping nothing.
    public func cancelImport() {
        importer = nil
    }

    /// Sends what the import sheet describes, and closes it if it was sent.
    ///
    /// The sheet stays open when the core would not send the request, with
    /// everything still in it, so the reason can be read beside the field it
    /// is about. A request that *was* sent and failed closes the sheet: it
    /// was sent, there is an exchange for it, and the failure is in the
    /// traffic list where every other failure is.
    public func replayImported() async {
        guard let importer else { return }
        guard let result = await importer.replay() else { return }

        self.importer = nil
        lastError = nil
        lastAction = replayReport(result)
    }

    /// What the status bar says about a replay that happened.
    private func replayReport(_ result: ReplayResult) -> String {
        if let status = result.status {
            return "Replayed — \(status). The new exchange is in the list."
        }

        return "Replayed — \(result.error?.summary ?? "no response"). The new exchange is in "
            + "the list."
    }

    // MARK: - HAR

    /// Whether there is an export to make at all.
    ///
    /// Not "is anything loaded": the core is what holds the traffic, and a
    /// filter that hides every row does not mean there is nothing to export.
    public var canExport: Bool { connection == .connected }

    /// Writes the selected exchange out as a HAR file.
    public func exportSelectedAsHar() async {
        guard let id = selectedID else { return }

        await export(.exchanges([id]), suggesting: HarFileName.suggested(host: filters.host))
    }

    /// Writes every exchange the current filters match out as a HAR file.
    ///
    /// The filters and nothing else: the same narrowing the list is showing,
    /// evaluated by the core over the whole store rather than over the rows
    /// that happen to be loaded. With nothing narrowed it is the whole
    /// history.
    public func exportFilteredAsHar() async {
        await export(
            .filtered(filters.exportParameters),
            suggesting: HarFileName.suggested(host: filters.host)
        )
    }

    /// Asks where to write, and has the core write there.
    ///
    /// A cancelled panel leaves no file and says nothing: cancelling is an
    /// answer, not a failure. What is reported is the count and the size,
    /// which is what a person wants to know, and the path they chose, which
    /// is theirs already.
    private func export(_ selection: HarSelection, suggesting name: String) async {
        guard let path = harFiles.chooseExportPath(suggestedName: name) else { return }

        do {
            let result = try await session.exportHar(
                HarExportRequest(path: path, selection: selection)
            )

            lastError = nil
            lastAction = exportReport(result, path: path)
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// What the status bar says about an export that happened.
    private func exportReport(_ result: HarExportResult, path: String) -> String {
        let entries = result.entries == 1 ? "1 exchange" : "\(result.entries) exchanges"
        let size = TrafficDisplay.size(result.bytes)

        if result.truncated {
            return "Exported \(entries) (\(size)) to \(path) — there were more matching "
                + "exchanges than one export writes."
        }

        return "Exported \(entries) (\(size)) to \(path)."
    }

    /// Reads a HAR file, and shows what it described.
    ///
    /// Nothing in the file is sent. What the core produces is stored traffic,
    /// so the list is asked for again afterwards — imported exchanges are not
    /// published on the traffic stream, which carries the exchanges the proxy
    /// *finished*, and an imported one is not one.
    public func importHar() async {
        guard let path = harFiles.chooseImportPath() else { return }

        do {
            let result = try await session.importHar(path: path)
            await loadFirstPage()

            lastError = nil
            lastAction = importReport(result)
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// What the status bar says about an import that happened.
    private func importReport(_ result: HarImportResult) -> String {
        let entries = result.imported == 1 ? "1 exchange" : "\(result.imported) exchanges"

        if result.skipped > 0 {
            return "Imported \(entries) — \(result.skipped) entries in the file were not "
                + "requests ProxyLab could read."
        }

        return "Imported \(entries). Nothing in the file was sent."
    }

    /// What a saved request made from the selection would be called.
    ///
    /// The method and the path, which is what a person would type anyway. The
    /// host is left out: the URL is on the request, and a list of saved
    /// requests reads better as a list of what they do.
    public var suggestedSaveName: String? {
        guard let id = selectedID,
            let summary = exchanges.first(where: { $0.id == id })
        else {
            return nil
        }

        return suggestedSaveName(for: summary)
    }

    /// What a saved request made from `summary` would be called.
    ///
    /// The method and the path, which is what a person would type anyway.
    func suggestedSaveName(for summary: ExchangeSummary) -> String {
        let path = URL(string: summary.url)?.path
        let tail = path.flatMap { $0.isEmpty ? nil : $0 } ?? summary.url

        return "\(summary.method) \(tail)"
    }

    // MARK: - Events

    /// Applies a coalesced batch, or one notification that cannot be batched.
    private func apply(_ delivery: ArrivalDelivery) async {
        switch delivery {
        case let .arrivals(traffic, streams):
            applyArrivals(traffic)
            applyStreamArrivals(streams)
        case let .other(notification):
            await handle(notification)
        }
    }

    /// Inserts a burst of finished exchanges in one turn.
    ///
    /// The same `add` a single `traffic_added` uses, so a batch lands where
    /// those events would have. One search reload is scheduled for the burst:
    /// each event would have cancelled the last and asked again.
    func applyArrivals(_ summaries: [ExchangeSummary]) {
        for summary in summaries {
            add(summary)
        }
        if filters.searchTerm != nil, !summaries.isEmpty {
            scheduleSearch()
        }
    }

    /// Inserts a burst of stream summaries in one turn.
    func applyStreamArrivals(_ summaries: [StreamMessageSummary]) {
        for summary in summaries {
            addedStream(summary)
        }
    }

    /// Applies one notification from the client.
    ///
    /// Internal rather than private so the behaviour can be tested as the
    /// code path it is, rather than through a stream that has to be waited on.
    func handle(_ notification: IPCNotification) async {
        switch notification {
        case let .event(.trafficAdded(summary)):
            add(summary)
            if filters.searchTerm != nil {
                // Headers and bodies are not on the summary. A match that
                // lives only there is picked up by reloading the query, not
                // by `matches`.
                scheduleSearch()
            }

        case .event(.trafficLost):
            // Rows were dropped for this subscriber, so the list it holds is
            // missing some. There is no way to know which, so it is asked for
            // again — the one case where a whole reload is the correct answer.
            await loadFirstPage()

        case let .event(.breakpointPaused(paused)):
            // Handed on rather than acted on. Nothing about the traffic list
            // changes: a held exchange is not in it yet, and it arrives as an
            // ordinary `traffic_added` once it has finished one way or the
            // other.
            breakpoints?.received(paused)

        case let .event(.breakpointReleased(pauseID, reason)):
            breakpoints?.released(pauseID: pauseID, reason: reason)

        case let .event(.streamMessageAdded(summary)):
            addedStream(summary)

        case .event(.streamMessagesLost):
            // Summaries were dropped for this subscriber, so the timeline it
            // holds is missing some. Ask for it again, the same answer the
            // traffic list already has for `traffic_lost`.
            if case let .loaded(exchange) = detail, exchange.carriesStreamMessages {
                await loadStreamMessages(for: exchange.id, request: detailRequest)
            }

        case let .connectionLost(error):
            // Everything the core was holding for this app is released by the
            // disconnection itself, so the queue goes with the connection.
            breakpoints?.connectionLost()
            connection = .disconnected(error.message)
            lastError = error.message
            scheduleReconnect()
        }
    }

    /// The stored exchange, for a rule started from its row.
    ///
    /// The one the detail pane already holds when it is this exchange, and
    /// otherwise a fetch. `nil` when the core does not have it: the draft
    /// then keeps what the row itself knew.
    public func capturedExchange(id: String) async -> TrafficExchange? {
        if case let .loaded(exchange) = detail, exchange.id == id {
            return exchange
        }

        return try? await session.traffic(id: id)
    }

    /// Takes in one exchange the proxy has just finished.
    ///
    /// Three questions, in order: is it already here, does the query want it,
    /// and does it belong among the rows that are loaded.
    private func add(_ summary: ExchangeSummary) {
        // Remembered whether or not the current query shows it, so the domain
        // list does not collapse to the host the list is filtered to.
        domains.observe(summary.destination?.host)

        // Already listed. An exchange that finished between subscribing and
        // the page arriving is in both, and counting it twice would show a
        // total the core does not have.
        if let existing = exchanges.firstIndex(where: { $0.id == summary.id }) {
            exchanges[existing] = summary
            return
        }

        // Not one of the rows this query describes. The core would not have
        // listed it, so neither does this.
        guard filters.matches(summary) else { return }

        // It is one more matching exchange whether or not it is shown.
        totalExchanges += 1

        guard belongsInLoadedRows else { return }

        insert(summary)
    }

    /// Whether a newly finished exchange belongs among the loaded rows.
    ///
    /// Newest first, it does: the loaded rows start at the newest, so the
    /// newest exchange there is goes at the top.
    ///
    /// Oldest first, it belongs at the very end of the whole result set — and
    /// the loaded rows only reach that end once there are no more pages.
    /// Putting it after the last loaded row while pages are still unfetched
    /// would show it before exchanges that are older than it. It is not lost:
    /// paging to the end reaches it, and `total` counts it in the meantime.
    private var belongsInLoadedRows: Bool {
        switch filters.sort {
        case .newestFirst: true
        case .oldestFirst: !hasMoreTraffic
        }
    }

    /// Puts one summary in its place, or replaces the one already there.
    private func insert(_ summary: ExchangeSummary) {
        if let existing = exchanges.firstIndex(where: { $0.id == summary.id }) {
            exchanges[existing] = summary
            return
        }

        let position = exchanges.firstIndex { comesAfter(summary, $0) } ?? exchanges.endIndex
        exchanges.insert(summary, at: position)
    }

    /// Whether `summary` sorts before `other` in the order being shown.
    ///
    /// The core's own ordering: by the millisecond the exchange started in,
    /// then by its id. Ids are time-ordered UUIDs, so comparing them breaks a
    /// tie the same way the core's query does rather than in some order of
    /// this app's own — and reading the order from the other end is the same
    /// comparison reversed, which is exactly what the core does.
    private func comesAfter(_ summary: ExchangeSummary, _ other: ExchangeSummary) -> Bool {
        let moment = summary.startedAt.milliseconds
        let otherMoment = other.startedAt.milliseconds

        let earlier = moment == otherMoment ? summary.id < other.id : moment < otherMoment

        return filters.sort == .newestFirst ? !earlier : earlier
    }

    /// What a person is told about `error`.
    private func message(for error: any Error) -> String {
        (error as? IPCError)?.message ?? "Something went wrong talking to the ProxyLab core."
    }
}

/// How long a burst of rows is gathered before one list update.
///
/// Short enough that a single exchange still shows up as it finishes, long
/// enough that a busy proxy is one redraw rather than one per event.
private let arrivalCoalesceWindow: Duration = .milliseconds(32)

/// What the notification pump hands the model.
private enum ArrivalDelivery: Sendable {
    case arrivals([ExchangeSummary], [StreamMessageSummary])
    case other(IPCNotification)
}

/// The leading traffic and stream events in `notifications`.
///
/// A lost event, a breakpoint, or a dropped connection stops the batch: those
/// change what the list means, and they are applied on their own.
struct CoalescedArrivals: Equatable, Sendable {
    var traffic: [ExchangeSummary]
    var streams: [StreamMessageSummary]
    var consumed: Int
    var stoppedAtBarrier: Bool
}

func coalescedPrefix(_ notifications: [IPCNotification]) -> CoalescedArrivals {
    var traffic: [ExchangeSummary] = []
    var streams: [StreamMessageSummary] = []

    for notification in notifications {
        switch notification {
        case let .event(.trafficAdded(summary)):
            traffic.append(summary)
        case let .event(.streamMessageAdded(summary)):
            streams.append(summary)
        default:
            return CoalescedArrivals(
                traffic: traffic,
                streams: streams,
                consumed: traffic.count + streams.count,
                stoppedAtBarrier: true
            )
        }
    }

    return CoalescedArrivals(
        traffic: traffic,
        streams: streams,
        consumed: traffic.count + streams.count,
        stoppedAtBarrier: false
    )
}

/// Queues notifications so a burst can be delivered together.
///
/// The reader and the pump are different tasks. The continuation is stored
/// before this actor waits, so a push that arrives as the wait begins is not
/// missed.
private actor ArrivalBuffer {
    private var pending: [IPCNotification] = []
    private var waiting: CheckedContinuation<Void, Never>?
    private var finished = false

    func push(_ notification: IPCNotification) {
        pending.append(notification)
        waiting?.resume()
        waiting = nil
    }

    func finish() {
        finished = true
        waiting?.resume()
        waiting = nil
    }

    func next(window: Duration) async -> ArrivalDelivery? {
        if pending.isEmpty {
            if finished { return nil }
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                if !pending.isEmpty || finished {
                    continuation.resume()
                } else {
                    waiting = continuation
                }
            }
            if pending.isEmpty { return nil }
        }

        if coalescedPrefix(pending).consumed > 0 {
            do {
                try await Task.sleep(for: window)
            } catch {
                return nil
            }
        }

        let prefix = coalescedPrefix(pending)
        if prefix.consumed > 0 {
            pending.removeFirst(prefix.consumed)
            return .arrivals(prefix.traffic, prefix.streams)
        }
        guard !pending.isEmpty else { return nil }

        return .other(pending.removeFirst())
    }
}

/// Reads `notifications` and delivers bursts of rows, off the main actor.
private func pumpArrivals(
    _ notifications: AsyncStream<IPCNotification>,
    window: Duration,
    deliver: @escaping @Sendable (ArrivalDelivery) async -> Void
) async {
    await withTaskGroup(of: Void.self) { group in
        let buffer = ArrivalBuffer()
        group.addTask {
            for await notification in notifications {
                if Task.isCancelled { break }
                await buffer.push(notification)
            }
            await buffer.finish()
        }
        group.addTask {
            while let delivery = await buffer.next(window: window) {
                if Task.isCancelled { return }
                await deliver(delivery)
            }
        }
        await group.waitForAll()
    }
}
