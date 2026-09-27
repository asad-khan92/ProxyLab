//  The detail pane against the real core.
//
//  The rest of the detail tests run with nothing installed. This one reads a
//  running `proxy-engine` over its actual socket — real captured exchanges,
//  real headers, real bodies — and is skipped unless one is pointed at:
//
//      PROXYLAB_IPC_SOCKET=~/.proxylab/run/proxylab.sock swift test
//
//  It is read-only against the developer's traffic: nothing is deleted and
//  nothing is cleared. New traffic, where a test needs some, is made by asking
//  the core to replay a request it already carried, which is the one way to
//  produce a `traffic_added` event without a browser.
//
//  What it asserts is that the reading this pane does agrees with what the
//  core stored: the same bytes, the same sizes, the same versions, the same
//  query. A core that has carried nothing yet leaves these with nothing to
//  assert, and they return rather than fail — an empty proxy is not a broken
//  client.
//
//  Nothing here prints a header, a body, or a URL. What is captured is
//  credentials as often as not, and a test log is a file.

import SwiftUI
import Testing

@testable import ProxyLabCore

/// The socket a running core is listening on, if a test run named one.
private let detailLiveSocket: String? = {
    guard
        let path = ProcessInfo.processInfo.environment[IPCSocketLocation.environmentVariable],
        !path.isEmpty,
        FileManager.default.fileExists(atPath: (path as NSString).expandingTildeInPath)
    else {
        return nil
    }

    return (path as NSString).expandingTildeInPath
}()

@Suite(
    "Traffic detail against a running core",
    .enabled(if: detailLiveSocket != nil, "set PROXYLAB_IPC_SOCKET to a running core's socket"),
    .serialized
)
@MainActor
struct TrafficDetailLiveCoreTests {
    /// How many of the newest exchanges the sweeping tests look at.
    private static let sample = 60

    /// A model reading a real core, with no reconnection and no search timer.
    private func liveModel() async throws -> TrafficInspectorModel {
        let path = try #require(detailLiveSocket)
        let model = TrafficInspectorModel(
            session: IPCClient.overSocket(at: path),
            socketPath: path,
            clipboard: FakeClipboard(),
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        try #require(model.connection == .connected, "the core did not answer")

        return model
    }

    /// Every exchange in the newest page, whole.
    private func storedExchanges(_ model: TrafficInspectorModel) async throws -> [TrafficExchange] {
        let client = IPCClient.overSocket(at: try #require(detailLiveSocket))
        try await client.connect()
        defer { Task { await client.disconnect() } }

        var exchanges: [TrafficExchange] = []
        for summary in model.exchanges.prefix(Self.sample) {
            exchanges.append(try await client.traffic(id: summary.id))
        }

        return exchanges
    }

    /// Whether there is anything here to assert about.
    ///
    /// An empty proxy is not a broken client, so a test in front of one
    /// returns. A core that *has* traffic and a list that did not load it is
    /// something else, and fails here rather than passing as a skip.
    private func hasTraffic(
        _ model: TrafficInspectorModel,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> Bool {
        guard model.exchanges.isEmpty else { return true }

        #expect(
            model.totalExchanges == 0,
            "the core has traffic the list did not load",
            sourceLocation: sourceLocation
        )

        return false
    }

    private func rendered(_ view: some View, width: CGFloat = 520) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    // MARK: - Everything the core has carried

    @Test("every stored exchange can be fetched, read, and drawn")
    func everyExchangeDraws() async throws {
        let model = try await liveModel()
        guard hasTraffic(model) else { return }

        // Snapshotted, because the core may carry more traffic while this
        // runs and the list would grow underneath the loop.
        let ids = model.exchanges.prefix(Self.sample).map(\.id)
        var drawn = 0

        for id in ids {
            await model.select(id)

            guard case let .loaded(exchange) = model.detail else {
                Issue.record("an exchange the list named could not be fetched")
                continue
            }
            let inspection = try #require(model.inspection)
            #expect(inspection.exchangeID == exchange.id)

            let size = try #require(rendered(TrafficDetailView(model: model)))
            #expect(size.width > 0 && size.height > 0)
            drawn += 1
        }

        #expect(drawn == ids.count)
    }

    @Test("what is shown of a body is what the core captured")
    func bodiesAreTheCapturedBytes() async throws {
        let model = try await liveModel()
        let exchanges = try await storedExchanges(model)
        guard !exchanges.isEmpty else { return }

        for exchange in exchanges {
            let inspection = BodyInspection.of(exchange)

            #expect(inspection.request.capture == exchange.request.body)
            #expect(inspection.response?.capture == exchange.response?.body)

            // The declared type is the header the message carried, not a
            // guess made from the bytes.
            #expect(
                inspection.request.contentType?.raw
                    == exchange.request.headers.text("Content-Type")?
                        .trimmingCharacters(in: .whitespacesAndNewlines)
            )
        }
    }

    @Test("a JSON body the core captured whole is laid out for reading")
    func jsonIsFormatted() async throws {
        let model = try await liveModel()
        let exchanges = try await storedExchanges(model)

        let json = exchanges.compactMap { exchange -> TextBody? in
            guard let response = exchange.response, !response.body.truncated else { return nil }
            guard case let .text(text) = BodyInspection.of(exchange).response?.content,
                text.language == .json
            else {
                return nil
            }

            return text
        }
        guard let body = json.first else { return }

        #expect(body.formatted != nil)
        // Laid out over lines, and the raw form still the text that arrived.
        #expect(body.formatted?.contains("\n") == true)
        #expect(!body.text.isEmpty)
    }

    @Test("the query read off a captured URL is the query that URL carried")
    func queryMatchesTheURL() async throws {
        let model = try await liveModel()
        guard hasTraffic(model) else { return }

        for summary in model.exchanges.prefix(Self.sample) {
            let parameters = QueryParameters.of(url: summary.url)
            let query = QueryParameters.queryString(of: summary.url) ?? ""

            // A URL with a query has parameters; one without has none.
            if query.contains(where: { $0 != "&" }) {
                #expect(!parameters.isEmpty)
            } else {
                #expect(parameters.isEmpty)
            }

            // Every parameter came from the URL, spelled as the URL spelled it.
            for parameter in parameters {
                #expect(query.contains(parameter.rawName))
                if let raw = parameter.rawValue, !raw.isEmpty {
                    #expect(query.contains(raw))
                }
            }
        }
    }

    @Test("the versions shown are the versions the core stored, on both halves")
    func versions() async throws {
        let model = try await liveModel()
        let exchanges = try await storedExchanges(model)
        guard !exchanges.isEmpty else { return }

        for exchange in exchanges {
            let pair = ExchangeVersions.of(exchange)
            #expect(pair.label.contains(ExchangeVersions.name(exchange.request.version)))

            if let response = exchange.response, response.version != exchange.request.version {
                // A mixed exchange names both, and neither is the other.
                #expect(pair.label.contains(ExchangeVersions.name(response.version)))
                #expect(pair.label.contains("→"))
            }
        }
    }

    @Test("a truncated capture says it is one, and says how much there was")
    func truncation() async throws {
        let model = try await liveModel()
        let exchanges = try await storedExchanges(model)

        let truncated = exchanges.compactMap { exchange -> BodyCapture? in
            guard let body = exchange.response?.body, body.truncated else { return nil }

            return body
        }
        guard let body = truncated.first else { return }

        #expect(body.size > UInt64(body.capturedByteCount))
        // Never presented as a whole body of the captured length.
        #expect(TrafficDisplay.size(body.size) != TrafficDisplay.size(UInt64(body.capturedByteCount)))
    }

    @Test("an exchange the origin never answered draws, with the request still on it")
    func failedExchange() async throws {
        let model = try await liveModel()
        guard
            let failed = model.exchanges.first(where: { $0.status == nil && $0.error != nil })
        else {
            return
        }

        await model.select(failed.id)

        guard case let .loaded(exchange) = model.detail else {
            Issue.record("a failed exchange could not be fetched")
            return
        }
        #expect(exchange.response == nil)
        #expect(exchange.error != nil)
        #expect(!exchange.request.method.isEmpty)
        #expect(try #require(rendered(TrafficDetailView(model: model))).height > 0)
    }

    // MARK: - While the proxy is working

    @Test("an exchange stays selected, and stays read, while new traffic arrives")
    func liveTrafficDoesNotDisturbTheSelection() async throws {
        let path = try #require(detailLiveSocket)
        let client = IPCClient.overSocket(at: path)
        let model = TrafficInspectorModel(
            session: client,
            socketPath: path,
            clipboard: FakeClipboard(),
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        try #require(model.connection == .connected, "the core did not answer")
        guard hasTraffic(model) else { return }

        // A row with a status is one the origin answered, and a plaintext URL
        // is one that can be sent again without a certificate. Picking blindly
        // would mean replaying to something that is not listening and waiting
        // out a TCP timeout.
        guard
            let replayable = model.exchanges.first(where: {
                $0.status != nil && $0.url.hasPrefix("http://")
            })
        else {
            await model.stop()

            return
        }

        await model.select(replayable.id)
        let before = try #require(model.inspection)
        let detail = model.detail
        let ids = model.exchanges.map(\.id)

        // Asking the core to send a request is the one way to make traffic
        // without a browser. It goes through the proxy's own pipeline and is
        // published on the subscription the model is already consuming.
        _ = try await client.replay(ReplayRequest(method: "GET", url: replayable.url))
        try await waitUntil("the replayed exchange to arrive") {
            model.exchanges.map(\.id) != ids
        }

        // New traffic landed, and none of it touched what was being inspected.
        #expect(model.exchanges.count > ids.count)
        #expect(model.selectedID == replayable.id)
        #expect(model.inspection == before)
        #expect(model.detail == detail)

        await model.stop()
    }

    /// Waits for something the core is doing, rather than for a length of time.
    private func waitUntil(
        _ what: String,
        within: Duration = .seconds(10),
        _ condition: () -> Bool
    ) async throws {
        let deadline = ContinuousClock.now.advanced(by: within)

        while ContinuousClock.now < deadline {
            if condition() { return }

            try await Task.sleep(for: .milliseconds(25))
        }

        Issue.record("timed out waiting for \(what)")
    }

    @Test("narrowing to one host leaves the pane showing the exchange it was showing")
    func pinnedHostKeepsTheDetail() async throws {
        let model = try await liveModel()
        guard hasTraffic(model),
            let summary = model.exchanges.first(where: { $0.destination?.host != nil }),
            let host = summary.destination?.host
        else {
            return
        }

        await model.select(summary.id)
        let before = try #require(model.inspection)

        await model.setHost(host)

        #expect(model.filters.host == host)
        #expect(model.selectedID == summary.id)
        #expect(model.inspection == before)
        #expect(try #require(rendered(TrafficDetailView(model: model))).height > 0)

        await model.setHost(nil)
    }

    // MARK: - Copying

    @Test("copying a URL, headers, and a body copies what the core stored")
    func copies() async throws {
        let path = try #require(detailLiveSocket)
        let clipboard = FakeClipboard()
        let model = TrafficInspectorModel(
            session: IPCClient.overSocket(at: path),
            socketPath: path,
            clipboard: clipboard,
            reconnect: .manual,
            searchSettlingTime: .seconds(600)
        )
        await model.start()
        try #require(model.connection == .connected, "the core did not answer")
        guard hasTraffic(model), let summary = model.exchanges.first else { return }

        await model.select(summary.id)
        guard case let .loaded(exchange) = model.detail else {
            Issue.record("the selected exchange could not be fetched")
            return
        }
        let inspection = try #require(model.inspection)

        model.copy(exchange.request.url, as: "the URL")
        #expect(clipboard.last == exchange.request.url)

        model.copy(ExchangeText.headers(exchange.request.headers), as: "the request headers")
        #expect(clipboard.last?.split(separator: "\n").count == exchange.request.headers.count)

        model.copy(ExchangeText.body(inspection.request), as: "the request body")
        if case .text = inspection.request.content {
            #expect(clipboard.last?.isEmpty == false)
        }

        // The cURL command is still the core's own, unchanged by any of this.
        await model.copyAsCurl(exchange.id)
        #expect(clipboard.last?.hasPrefix("curl") == true)
        #expect(model.lastError == nil)
    }
}
