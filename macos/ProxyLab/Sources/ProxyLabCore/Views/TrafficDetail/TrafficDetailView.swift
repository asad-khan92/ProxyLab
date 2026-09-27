//  One exchange, whole.
//
//      POST  https://api.example.com/v1/users?page=2           [⧉] [↻]
//      201 Created · HTTP/1.1 → HTTP/2 · 84 ms · 🔒
//      ┌ Overview ┬ Request ┬ Response ┬ Messages ┐
//      │ query, headers, body, stream messages    │
//      └──────────────────────────────────────────┘
//
//  Fetched one exchange at a time, by id, when a row is selected. The list
//  never carries headers or bodies, and nothing here is reconstructed from the
//  row: what is shown is the stored exchange as the core sent it.
//
//  Three faces for ordinary HTTP, four when the exchange is a WebSocket
//  handshake or an event stream. Messages is the same pane, not a debugger of
//  its own: a timeline of captured frames or events, and the selected payload
//  shown the way an HTTP body already is.
//
//  The bodies are not decoded here. They were read when the exchange arrived —
//  once, off the main actor — and this pane draws what that reading came to.

import SwiftUI

/// The detail pane: whichever exchange is selected, or why there is none.
struct TrafficDetailView: View {
    let model: TrafficInspectorModel

    var body: some View {
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
                ExchangeView(exchange: exchange, inspection: inspection, model: model)
            } else {
                // The two are written together, so this is the one frame
                // between a new selection arriving and its bodies being read.
                LoadingView(title: "Reading bodies…")
            }
        }
    }
}

/// The faces of one exchange, with the metadata above them.
private struct ExchangeView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel

    /// Which face is showing. Held here rather than in the model: it is a
    /// state of this view and of nothing else.
    ///
    /// Keyed to the exchange, so selecting another row opens it on the same
    /// face rather than on whatever the last one was left on.
    @State private var face: Face = .overview

    private enum Face: String, Identifiable {
        case overview = "Overview"
        case request = "Request"
        case response = "Response"
        case messages = "Messages"

        var id: Self { self }
    }

    private var faces: [Face] {
        var faces: [Face] = [.overview, .request, .response]
        if exchange.carriesStreamMessages {
            faces.append(.messages)
        }

        return faces
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ExchangeHeadline(exchange: exchange, inspection: inspection, model: model)

            Picker("", selection: $face) {
                ForEach(faces) { face in
                    Text(face.rawValue).tag(face)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.horizontal, 16)
            .padding(.bottom, 10)
            .onChange(of: exchange.id) { _, _ in
                if face == .messages, !exchange.carriesStreamMessages {
                    face = .overview
                }
            }

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    switch face {
                    case .overview:
                        OverviewView(exchange: exchange, inspection: inspection)
                    case .request:
                        RequestView(exchange: exchange, inspection: inspection, model: model)
                    case .response:
                        ResponseView(exchange: exchange, inspection: inspection, model: model)
                    case .messages:
                        StreamMessagesView(exchange: exchange, model: model)
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(minWidth: 380)
    }
}

/// The method, the URL, how the exchange ended, and what it can be copied as.
private struct ExchangeHeadline: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(exchange.request.method)
                    .font(.system(size: 13, weight: .semibold, design: .monospaced))

                outcome

                Spacer(minLength: 8)

                CopyMenu(exchange: exchange, inspection: inspection, model: model)

                // The command is written by the core, from the request the
                // core stored — headers, credentials, and body included,
                // because the point of it is to send the request again.
                Button {
                    Task { await model.copyAsCurl(exchange.id) }
                } label: {
                    Label("Copy as cURL", systemImage: "terminal")
                }
                .controlSize(.small)
                .help("Copy this request to the clipboard as a cURL command")

                Button {
                    Task { await model.editSelectedAsRequest() }
                } label: {
                    Label("Edit & Replay", systemImage: "paperplane")
                }
                .controlSize(.small)
                .help("Open this request in the replay editor")
            }

            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(exchange.request.url)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)

                CopyButton(what: "the URL") {
                    model.copy(exchange.request.url, as: "the URL")
                }
            }

            metadata

            if let error = exchange.error {
                Label(error.summary, systemImage: "exclamationmark.octagon")
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The status and its reason phrase, or why there is neither.
    @ViewBuilder
    private var outcome: some View {
        if let response = exchange.response {
            OutcomeBadge(outcome: .status(response.status))

            if let reason = response.reason, !reason.isEmpty {
                Text(reason)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        } else if exchange.error != nil {
            OutcomeBadge(outcome: .failed)
            Text("No response")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        } else {
            OutcomeBadge(outcome: .pending)
            Text("Waiting for a response")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        }
    }

    /// The row of small facts under the URL.
    private var metadata: some View {
        HStack(spacing: 10) {
            SchemeMark(scheme: exchange.connection.scheme)

            MetaChip(
                symbol: nil,
                text: ExchangeVersions.of(exchange).label,
                help: ExchangeVersions.of(exchange).help
            )

            if let total = ExchangeTiming.total(of: exchange.timing) {
                MetaChip(
                    symbol: "clock",
                    text: ExchangeTiming.duration(micros: total),
                    help: "How long the whole exchange took"
                )
            }

            if !inspection.request.isEmpty {
                MetaChip(
                    symbol: "arrow.up",
                    text: TrafficDisplay.size(exchange.request.body.size),
                    help: "Request body"
                )
            }
            if let response = inspection.response, !response.isEmpty {
                MetaChip(
                    symbol: "arrow.down",
                    text: TrafficDisplay.size(response.capture.size),
                    help: "Response body"
                )
            }

            Spacer(minLength: 0)
        }
    }
}

/// Everything about this exchange that can be put on the clipboard.
///
/// One menu rather than a row of buttons: they are all the same verb, and the
/// things worth copying differ between an exchange with a response and one
/// without. Nothing copied from here is written anywhere but the clipboard.
private struct CopyMenu: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel

    var body: some View {
        Menu {
            Button("URL") { model.copy(exchange.request.url, as: "the URL") }

            let query = QueryParameters.of(url: exchange.request.url)
            if !query.isEmpty {
                Button("Query Parameters") {
                    model.copy(ExchangeText.queryParameters(query), as: "the query parameters")
                }
            }

            Divider()

            Button("Request Headers") {
                model.copy(
                    ExchangeText.headers(exchange.request.headers),
                    as: "the request headers"
                )
            }
            if !inspection.request.isEmpty {
                Button(inspection.request.copyBodyLabel) {
                    model.copy(ExchangeText.body(inspection.request), as: "the request body")
                }
            }

            if let response = exchange.response, let body = inspection.response {
                Divider()

                Button("Response Headers") {
                    model.copy(ExchangeText.headers(response.headers), as: "the response headers")
                }
                if !body.isEmpty {
                    Button(body.copyBodyLabel) {
                        model.copy(ExchangeText.body(body), as: "the response body")
                    }
                }
            }
        } label: {
            Label("Copy", systemImage: "doc.on.doc")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .controlSize(.small)
        .help("Copy part of this exchange to the clipboard")
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

/// How an exchange's two protocol versions read together.
///
/// Kept apart from the request and the response because they are decided
/// separately: the proxy speaks to the client in the version the client
/// offered and to the origin in the version the origin offered, and an
/// exchange where those differ is an ordinary exchange, not a mistake.
enum ExchangeVersions {
    struct Pair: Equatable {
        let label: String
        let help: String
    }

    static func of(_ exchange: TrafficExchange) -> Pair {
        let request = exchange.request.version
        guard let response = exchange.response?.version else {
            return Pair(
                label: name(request),
                help: "The client framed its request as \(name(request))."
            )
        }
        guard response != request else {
            return Pair(
                label: name(request),
                help: "Both halves were framed as \(name(request))."
            )
        }

        return Pair(
            label: "\(name(request)) → \(name(response))",
            help: "The client's request was \(name(request)); the origin's response was "
                + "\(name(response))."
        )
    }

    /// A version's name, or an honest word for one this build cannot name.
    static func name(_ version: HTTPVersion) -> String {
        switch version {
        case .http09: "HTTP/0.9"
        case .http10: "HTTP/1.0"
        case .http11: "HTTP/1.1"
        // The core spells this `HTTP/2.0`; everything a developer reads about
        // it spells it `HTTP/2`.
        case .http2: "HTTP/2"
        case .http3: "HTTP/3"
        case .unknown: "Unknown version"
        }
    }
}
