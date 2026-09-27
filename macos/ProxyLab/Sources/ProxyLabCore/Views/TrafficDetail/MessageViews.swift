//  The three faces of one exchange: the overview, the request, the response.
//
//  Everything here comes out of the stored exchange. Where a field is absent —
//  no response, no reason phrase, no completion, no content type — the absence
//  is shown as itself rather than filled in: an exchange the origin never
//  answered is not an exchange with a `502`, and a body nobody typed a media
//  type for is not `application/octet-stream`.

import SwiftUI

/// Both halves side by side, and the clock that ran over them.
struct OverviewView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection

    var body: some View {
        DetailSection(title: "Request") {
            FieldGrid(requestFields)
        }

        DetailSection(title: "Response") {
            if let response = exchange.response {
                FieldGrid(responseFields(response, body: inspection.response))
            } else {
                Text(
                    exchange.error?.summary
                        ?? "The exchange has not finished, so there is no response yet."
                )
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            }
        }

        DetailSection(title: "Timing") {
            VStack(alignment: .leading, spacing: 10) {
                TimingBar(spans: ExchangeTiming.spans(of: exchange.timing))
                FieldGrid(timingFields)
            }
        }

        DetailSection(title: "Connection") {
            FieldGrid(connectionFields)
        }
    }

    private var requestFields: [FieldGrid.Field] {
        [
            FieldGrid.Field(name: "Method", value: exchange.request.method),
            FieldGrid.Field(name: "URL", value: exchange.request.url),
            FieldGrid.Field(
                name: "HTTP version",
                value: ExchangeVersions.name(exchange.request.version)
            ),
            FieldGrid.Field(name: "Headers", value: String(exchange.request.headers.count)),
            contentTypeField(inspection.request),
            bodyField(inspection.request),
        ]
    }

    private func responseFields(_ response: CapturedResponse, body: BodyInspection?)
        -> [FieldGrid.Field]
    {
        var fields = [
            FieldGrid.Field(
                name: "Status",
                value: TrafficDisplay.status(response.status, reason: response.reason),
                note: TrafficDisplay.statusClass(response.status)
            ),
            FieldGrid.Field(
                name: "HTTP version",
                value: ExchangeVersions.name(response.version)
            ),
            FieldGrid.Field(name: "Headers", value: String(response.headers.count)),
        ]

        if let body {
            fields.append(contentTypeField(body))
            fields.append(bodyField(body))
        }

        return fields
    }

    private func contentTypeField(_ body: BodyInspection) -> FieldGrid.Field {
        FieldGrid.Field(
            name: "Content type",
            // The header as it was spelled, parameters and all. "None" is not
            // a media type; it is the absence of one, and saying so is the
            // difference between a message that declared nothing and one that
            // declared bytes.
            value: body.contentType?.raw ?? "None declared"
        )
    }

    private func bodyField(_ body: BodyInspection) -> FieldGrid.Field {
        FieldGrid.Field(
            name: "Body",
            value: body.isEmpty ? "None" : TrafficDisplay.size(body.capture.size),
            note: body.capture.truncated
                ? "Captured "
                    + TrafficDisplay.size(UInt64(body.capture.capturedByteCount))
                    + " — truncated"
                : nil
        )
    }

    private var timingFields: [FieldGrid.Field] {
        var fields = [FieldGrid.Field(name: "Started", value: started)]

        for mark in ExchangeTiming.marks(of: exchange.timing) {
            fields.append(
                FieldGrid.Field(
                    name: mark.name,
                    value: ExchangeTiming.duration(micros: mark.micros),
                    note: "after the request arrived"
                )
            )
        }

        fields.append(
            FieldGrid.Field(
                name: "Total",
                value: ExchangeTiming.total(of: exchange.timing)
                    .map(ExchangeTiming.duration) ?? "did not finish"
            )
        )

        return fields
    }

    private var connectionFields: [FieldGrid.Field] {
        var fields = [
            FieldGrid.Field(
                name: "Scheme",
                value: exchange.connection.scheme.rawValue,
                note: exchange.connection.scheme == .https
                    ? "read out of an intercepted TLS session"
                    : nil
            ),
            FieldGrid.Field(name: "Client", value: exchange.connection.client),
        ]

        if let destination = exchange.connection.destination {
            fields.append(
                FieldGrid.Field(
                    name: "Destination",
                    value: "\(destination.host):\(destination.port)"
                )
            )
        }
        fields.append(FieldGrid.Field(name: "Exchange ID", value: exchange.id))

        return fields
    }

    private var started: String {
        let date = exchange.timing.startedAt.date

        return date.formatted(date: .abbreviated, time: .standard)
            + " ("
            + TrafficDisplay.time(exchange.timing.startedAt)
            + ")"
    }
}

/// What the client asked for.
struct RequestView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel

    @State private var rawHeaders = false

    private var query: [QueryParameter] { QueryParameters.of(url: exchange.request.url) }

    var body: some View {
        DetailSection(title: "Request") {
            FieldGrid([
                FieldGrid.Field(
                    name: "HTTP version",
                    value: ExchangeVersions.name(exchange.request.version)
                ),
                FieldGrid.Field(
                    name: "Content type",
                    value: inspection.request.contentType?.raw ?? "None declared"
                ),
                FieldGrid.Field(
                    name: "Body",
                    value: inspection.request.isEmpty
                        ? "None"
                        : TrafficDisplay.size(inspection.request.capture.size)
                ),
            ])
        }

        DetailSection(title: "Query Parameters (\(query.count))") {
            if !query.isEmpty {
                CopyButton(what: "the query parameters") {
                    model.copy(ExchangeText.queryParameters(query), as: "the query parameters")
                }
            }
        } content: {
            QueryParameterListView(parameters: query) { text, what in
                model.copy(text, as: what)
            }
        }

        DetailSection(title: "Headers (\(exchange.request.headers.count))") {
            if !exchange.request.headers.isEmpty {
                HStack(spacing: 10) {
                    CopyButton(what: "the request headers") {
                        model.copy(
                            ExchangeText.headers(exchange.request.headers),
                            as: "the request headers"
                        )
                    }
                    HeaderModePicker(raw: $rawHeaders)
                }
            }
        } content: {
            if rawHeaders {
                RawHeadersText(text: ExchangeText.headers(exchange.request.headers))
            } else {
                HeaderListView(headers: exchange.request.headers) { text, what in
                    model.copy(text, as: what)
                }
            }
        }

        BodySection(
            title: "Request Body",
            inspection: inspection.request,
            url: exchange.request.url,
            half: .request,
            model: model
        )
    }
}

/// What came back, or why nothing did.
struct ResponseView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let model: TrafficInspectorModel

    @State private var rawHeaders = false

    var body: some View {
        if let response = exchange.response {
            DetailSection(title: "Status") {
                FieldGrid([
                    FieldGrid.Field(
                        name: "Status",
                        value: TrafficDisplay.status(response.status, reason: response.reason),
                        note: TrafficDisplay.statusClass(response.status)
                    ),
                    FieldGrid.Field(
                        name: "HTTP version",
                        value: ExchangeVersions.name(response.version)
                    ),
                    FieldGrid.Field(
                        name: "Content type",
                        value: inspection.response?.contentType?.raw ?? "None declared"
                    ),
                    FieldGrid.Field(
                        name: "Body",
                        value: (inspection.response?.isEmpty ?? true)
                            ? "None"
                            : TrafficDisplay.size(inspection.response?.capture.size ?? 0)
                    ),
                ])
            }

            DetailSection(title: "Headers (\(response.headers.count))") {
                if !response.headers.isEmpty {
                    HStack(spacing: 10) {
                        CopyButton(what: "the response headers") {
                            model.copy(
                                ExchangeText.headers(response.headers),
                                as: "the response headers"
                            )
                        }
                        HeaderModePicker(raw: $rawHeaders)
                    }
                }
            } content: {
                if rawHeaders {
                    RawHeadersText(text: ExchangeText.headers(response.headers))
                } else {
                    HeaderListView(headers: response.headers) { text, what in
                        model.copy(text, as: what)
                    }
                }
            }

            if let body = inspection.response {
                BodySection(
                    title: "Response Body",
                    inspection: body,
                    url: exchange.request.url,
                    half: .response,
                    model: model
                )
            }
        } else {
            DetailSection(title: "No response") {
                NoResponseView(error: exchange.error)
            }
        }
    }
}

/// Why an exchange has no response half to show.
private struct NoResponseView: View {
    let error: TrafficError?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let error {
                Label(error.summary, systemImage: "exclamationmark.octagon")
                    .font(.system(size: 12))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)

                Text(error.kind.rawValue)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)

                Text("The request is on the Request tab, exactly as the client sent it.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            } else {
                Text("The exchange has not finished, so there is no response yet.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A body, with what can be done with it beside its title.
private struct BodySection: View {
    let title: String
    let inspection: BodyInspection
    /// The request's URL, which is what a saved file is named after.
    let url: String
    let half: ExchangeText.Half
    let model: TrafficInspectorModel

    var body: some View {
        DetailSection(title: title) {
            if !inspection.isEmpty {
                HStack(spacing: 10) {
                    CopyButton(what: copyDescription) {
                        model.copy(ExchangeText.body(inspection), as: copyDescription)
                    }

                    Button("Save…") {
                        model.saveBody(inspection, url: url, half: half)
                    }
                    .buttonStyle(.borderless)
                    .font(.system(size: 10))
                    .help("Write the captured bytes to a file you choose")
                }
            }
        } content: {
            BodyView(inspection: inspection) { text, what in
                model.copy(text, as: what)
            }
        }
    }

    private var copyDescription: String {
        let what = half == .request ? "the request body" : "the response body"

        switch inspection.content {
        case .image, .binary, .undecodable: return "\(what) as Base64"
        case .empty, .text: return what
        }
    }
}

/// Formatted vs the raw header block as it would be copied.
private struct HeaderModePicker: View {
    @Binding var raw: Bool

    var body: some View {
        Picker("", selection: $raw) {
            Text("Formatted").tag(false)
            Text("Raw").tag(true)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
        .fixedSize()
        .help("How the headers are shown")
    }
}

/// Headers as `Name: value` lines, selectable.
private struct RawHeadersText: View {
    let text: String

    var body: some View {
        Text(text.isEmpty ? "No headers" : text)
            .font(.system(size: 11, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(8)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 5))
    }
}
