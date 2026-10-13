//  One exchange, whole.
//
//      GET   api.example.com                              [⋯]
//            /api/v1/customer/category
//      200 OK · HTTP/2 · 84 ms · application/json
//      ┌ Request ┬ Response ┬ Messages ┐
//      │ Overview│ headers, body, …    │
//      │ Headers │                     │
//      └─────────┴─────────────────────┘
//
//  Fetched one exchange at a time, by id, when a row is selected. The list
//  never carries headers or bodies, and nothing here is reconstructed from the
//  row: what is shown is the stored exchange as the core sent it.
//
//  Request and Response are the two halves. Messages is the same pane when
//  the exchange is a WebSocket handshake or an event stream: a timeline of
//  captured frames or events, and the selected payload shown the way an HTTP
//  body already is.
//
//  The bodies are not decoded here. They were read when the exchange arrived —
//  once, off the main actor — and this pane draws what that reading came to.
//  Headers, query, and cookies are prepared once per selection as well.

import SwiftUI

/// The detail pane: whichever exchange is selected, or why there is none.
struct TrafficDetailView: View {
    let model: TrafficInspectorModel
    var saved: SavedRequestsModel?
    var rules: RulesModel?
    var breakpoints: BreakpointsModel?

    var body: some View {
        if let breakpoints, breakpoints.showingHold, let editor = breakpoints.editor {
            PauseEditorView(editor: editor, model: breakpoints)
                .id(editor.paused.pauseID)
        } else {
            stored
        }
    }

    @ViewBuilder
    private var stored: some View {
        switch model.detail {
        case .none:
            PlaceholderView(
                symbol: "sidebar.right",
                title: "No exchange selected",
                detail: "Select a request on the left to see its headers, body, and timing."
            )

        case .loading:
            LoadingView(title: "Loading exchange…")

        case .unavailable:
            PlaceholderView(
                symbol: "trash",
                title: "This exchange is gone",
                detail: "It was deleted or cleared from the core's traffic store."
            )

        case let .failed(reason):
            PlaceholderView(
                symbol: "exclamationmark.triangle",
                title: "This exchange could not be loaded",
                detail: reason
            )

        case let .loaded(exchange):
            if let inspection = model.inspection, inspection.exchangeID == exchange.id {
                ExchangeView(
                    exchange: exchange,
                    inspection: inspection,
                    model: model,
                    saved: saved,
                    rules: rules,
                    breakpoints: breakpoints
                )
            } else {
                // The two are written together, so this is the one frame
                // between a new selection arriving and its bodies being read.
                LoadingView(title: "Reading bodies…")
            }
        }
    }
}

/// The faces of one exchange, with the summary above them.
private struct ExchangeView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel
    let saved: SavedRequestsModel?
    let rules: RulesModel?
    let breakpoints: BreakpointsModel?

    /// Which face is showing. Held here rather than in the model: it is a
    /// state of this view and of nothing else.
    @State private var face: InspectorFace = .request
    /// Built for one exchange id. A redraw uses this instead of parsing again.
    @State private var reading: InspectorReading

    init(
        exchange: TrafficExchange,
        inspection: ExchangeInspection,
        model: TrafficInspectorModel,
        saved: SavedRequestsModel?,
        rules: RulesModel?,
        breakpoints: BreakpointsModel?
    ) {
        self.exchange = exchange
        self.inspection = inspection
        self.model = model
        self.saved = saved
        self.rules = rules
        self.breakpoints = breakpoints
        _reading = State(
            initialValue: InspectorReading.make(exchange: exchange, inspection: inspection)
        )
    }

    /// The reading for the exchange on screen. The stored one when it matches,
    /// and a fresh one in the frame before that store catches up.
    private var currentReading: InspectorReading {
        if reading.sourceID == exchange.id {
            return reading
        }

        return InspectorReading.make(exchange: exchange, inspection: inspection)
    }

    private var faces: [InspectorFace] {
        InspectorSections.faces(of: exchange)
    }

    private var resolvedFace: InspectorFace {
        if face == .messages, !exchange.carriesStreamMessages {
            return .request
        }

        return face
    }

    var body: some View {
        let prepared = currentReading

        VStack(alignment: .leading, spacing: 0) {
            ExchangeHeadline(
                exchange: exchange,
                inspection: inspection,
                reading: prepared,
                model: model,
                saved: saved,
                rules: rules,
                breakpoints: breakpoints
            )

            if let failure = prepared.failure {
                FailureSummary(failure: failure)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)
            }

            Picker("", selection: $face) {
                ForEach(faces) { item in
                    Text(item.rawValue).tag(item)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .controlSize(.small)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            .onChange(of: exchange.id) { _, _ in
                reading = InspectorReading.make(exchange: exchange, inspection: inspection)
                if face == .messages, !exchange.carriesStreamMessages {
                    face = .request
                }
            }

            Divider()

            switch resolvedFace {
            case .request:
                RequestView(
                    exchange: exchange,
                    inspection: inspection,
                    reading: prepared,
                    model: model
                )
            case .response:
                ResponseView(
                    exchange: exchange,
                    inspection: inspection,
                    reading: prepared,
                    model: model
                )
            case .messages:
                ScrollView {
                    StreamMessagesView(exchange: exchange, model: model)
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .frame(minWidth: 380, maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

/// The method, where it went, how it ended, and what can be done with it.
private struct ExchangeHeadline: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let reading: InspectorReading
    let model: TrafficInspectorModel
    let saved: SavedRequestsModel?
    let rules: RulesModel?
    let breakpoints: BreakpointsModel?

    private var headline: InspectorHeadline { reading.headline }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(headline.method)
                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                    Text(headline.host)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }

                Text(headline.path)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)

                metadata
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            ExchangeActions(
                exchange: exchange,
                inspection: inspection,
                reading: reading,
                model: model,
                saved: saved,
                rules: rules,
                breakpoints: breakpoints
            )
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }

    private var metadata: some View {
        ViewThatFits(in: .horizontal) {
            chipRow(primary + secondary)
            VStack(alignment: .leading, spacing: 2) {
                chipRow(primary)
                chipRow(secondary)
            }
        }
    }

    private var primary: [HeadlineChip] {
        var chips = [
            HeadlineChip(
                text: headline.statusText,
                tone: headline.statusTone,
                help: headline.statusText
            ),
            HeadlineChip(text: headline.version, tone: nil, help: headline.versionHelp),
        ]
        if let duration = headline.duration {
            chips.append(
                HeadlineChip(text: duration, tone: nil, help: "How long the exchange took")
            )
        }
        if let type = headline.contentType {
            chips.append(HeadlineChip(text: type, tone: nil, help: "Response content type"))
        }

        return chips
    }

    private var secondary: [HeadlineChip] {
        var chips = [
            HeadlineChip(text: headline.time, tone: nil, help: "When the request arrived"),
            HeadlineChip(
                text: headline.secure ? "HTTPS" : "HTTP",
                tone: nil,
                help: headline.secure ? "Intercepted TLS" : "Plaintext"
            ),
        ]
        if let size = headline.requestSize {
            chips.append(HeadlineChip(text: "↑ \(size)", tone: nil, help: "Request body"))
        }
        if let size = headline.responseSize {
            chips.append(HeadlineChip(text: "↓ \(size)", tone: nil, help: "Response body"))
        }

        return chips
    }

    private func chipRow(_ chips: [HeadlineChip]) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(chips.enumerated()), id: \.offset) { index, chip in
                if index > 0 {
                    Text("  ·  ")
                        .font(.system(size: 11))
                        .foregroundStyle(.quaternary)
                }
                Text(chip.text)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(chip.tone.map(Self.color) ?? AnyShapeStyle(.secondary))
                    .help(chip.help)
                    .lineLimit(1)
            }
        }
        .fixedSize(horizontal: true, vertical: false)
    }

    private static func color(_ tone: OverviewTone) -> AnyShapeStyle {
        switch tone {
        case .success: AnyShapeStyle(.green)
        case .redirect: AnyShapeStyle(.teal)
        case .clientError: AnyShapeStyle(.orange)
        case .serverError, .failed: AnyShapeStyle(.red)
        case .informational, .pending, .other: AnyShapeStyle(.secondary)
        }
    }
}

/// One fact in the summary line.
private struct HeadlineChip {
    let text: String
    let tone: OverviewTone?
    let help: String
}

/// Replay, copy, save, and apply a rule. The same paths the traffic row uses.
private struct ExchangeActions: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let reading: InspectorReading
    let model: TrafficInspectorModel
    let saved: SavedRequestsModel?
    let rules: RulesModel?
    let breakpoints: BreakpointsModel?

    private var summary: ExchangeSummary {
        model.exchanges.first { $0.id == exchange.id } ?? ExchangeSummary(exchange: exchange)
    }

    private var actions: TrafficRowMenu {
        TrafficRowMenu(summary: summary, isPinned: false)
    }

    var body: some View {
        Menu {
            Button("Replay Request") {
                Task { await actions.replay(using: model) }
            }
            .disabled(!actions.canReplay)

            Button("Copy as cURL") {
                Task { await actions.copyCurl(using: model) }
            }
            .disabled(!actions.canCopyCurl)

            Button("Save Request") {
                if let saved {
                    actions.save(using: model, saved: saved)
                }
            }
            .disabled(saved == nil || !actions.canSave)

            Divider()

            Button("Breakpoint…") {
                if let breakpoints {
                    actions.beginBreakpoint(using: breakpoints)
                }
            }
            .disabled(breakpoints == nil || !actions.canBreakpoint)

            Menu("Apply Rule") {
                ForEach(trafficRuleActions) { kind in
                    Button("\(kind.title)…") {
                        apply(kind)
                    }
                }
            }
            .disabled(rules == nil)

            Divider()

            Button("Copy URL") {
                model.copy(exchange.request.url, as: "the URL")
            }
            .disabled(exchange.request.url.isEmpty)

            copyParts
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 12, weight: .semibold))
                .frame(width: 22, height: 16)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Replay, copy, save, breakpoint, or apply a rule")
        .accessibilityLabel("Actions")
    }

    @ViewBuilder
    private var copyParts: some View {
        Divider()

        if !reading.query.isEmpty {
            Button("Copy Query") {
                model.copy(
                    ExchangeText.queryParameters(reading.query),
                    as: "the query parameters"
                )
            }
        }

        Button("Copy Request Headers") {
            model.copy(reading.requestHeaders.copyAll, as: "the request headers")
        }
        if !inspection.request.isEmpty {
            Button(inspection.request.copyBodyLabel) {
                model.copy(ExchangeText.body(inspection.request), as: "the request body")
            }
        }

        if exchange.response != nil, let body = inspection.response {
            Button("Copy Response Headers") {
                model.copy(reading.responseHeaders.copyAll, as: "the response headers")
            }
            if !body.isEmpty {
                Button(body.copyBodyLabel) {
                    model.copy(ExchangeText.body(body), as: "the response body")
                }
            }
        }
    }

    private func apply(_ kind: RuleActionKind) {
        guard let rules else { return }

        rules.beginTrafficRule(
            TrafficRuleRequest(
                kind: kind,
                exchangeID: summary.id,
                seed: RuleDraftSeed.from(summary: summary, kind: kind)
            )
        )
    }
}

extension BodyInspection {
    /// What a menu item copying this body should be called.
    ///
    /// Base64 when the bytes are not text, so nobody is promised characters
    /// that would arrive as something the server never sent.
    var copyBodyLabel: String {
        switch content {
        case .image, .binary, .undecodable: "Body as Base64"
        case .empty, .text: "Body"
        }
    }
}
