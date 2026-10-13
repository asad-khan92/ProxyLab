//  The sections of one exchange: the request, the response, and a failure.
//
//  Everything here comes out of the reading built for this selection. Where
//  a field is absent — no response, no reason phrase, no content type — the
//  absence is shown as itself rather than filled in.

import SwiftUI

/// The request half: one section at a time, chosen from the rail.
struct RequestView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let reading: InspectorReading
    let model: TrafficInspectorModel

    @State private var section: RequestSection = .overview

    var body: some View {
        InspectorSectionSplit(selection: $section, sections: RequestSection.allCases) {
            switch section {
            case .overview:
                FactList(facts: reading.requestFacts)
            case .headers:
                HeaderListView(presentation: reading.requestHeaders) { text, what in
                    model.copy(text, as: what)
                }
            case .query:
                QueryParameterListView(parameters: reading.query) { text, what in
                    model.copy(text, as: what)
                }
            case .body:
                BodyView(
                    inspection: inspection.request,
                    contentEncoding: reading.requestEncoding,
                    copySubject: "the request body",
                    save: {
                        model.saveBody(inspection.request, url: exchange.request.url, half: .request)
                    },
                    copy: { text, what in model.copy(text, as: what) }
                )
            case .cookies:
                CookieListView(
                    presentation: reading.requestCookies,
                    emptyText: "No cookies"
                ) { text, what in
                    model.copy(text, as: what)
                }
            case .auth:
                AuthorizationSection(presentation: reading.authorization)
            case .raw:
                ReconstructedMessageView(message: reading.rawRequest)
            }
        }
    }
}

/// The response half.
struct ResponseView: View {
    let exchange: TrafficExchange
    let inspection: ExchangeInspection
    let reading: InspectorReading
    let model: TrafficInspectorModel

    @State private var section: ResponseSection = .overview

    var body: some View {
        InspectorSectionSplit(selection: $section, sections: ResponseSection.allCases) {
            switch section {
            case .overview:
                responseOverview
            case .headers:
                HeaderListView(presentation: reading.responseHeaders) { text, what in
                    model.copy(text, as: what)
                }
            case .body:
                if let body = inspection.response {
                    BodyView(
                        inspection: body,
                        contentEncoding: reading.responseEncoding,
                        status: exchange.response.map {
                            TrafficDisplay.status($0.status, reason: $0.reason)
                        },
                        copySubject: "the response body",
                        save: {
                            model.saveBody(body, url: exchange.request.url, half: .response)
                        },
                        copy: { text, what in model.copy(text, as: what) }
                    )
                } else {
                    InspectorEmptyNote(text: "No response body")
                }
            case .cookies:
                CookieListView(
                    presentation: reading.responseCookies,
                    emptyText: "No cookies"
                ) { text, what in
                    model.copy(text, as: what)
                }
            case .raw:
                if let message = reading.rawResponse {
                    ReconstructedMessageView(message: message)
                } else {
                    InspectorEmptyNote(text: "No response was captured.")
                }
            }
        }
    }

    @ViewBuilder
    private var responseOverview: some View {
        if let facts = reading.responseFacts {
            VStack(alignment: .leading, spacing: 12) {
                FactList(facts: facts)
                TimingBar(spans: ExchangeTiming.spans(of: exchange.timing))
            }
        } else if reading.failure != nil {
            InspectorEmptyNote(text: "No response was captured.")
        } else if let pending = reading.pendingDetail {
            Text(pending)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The authorization section. Schemes and masks, never the credential.
private struct AuthorizationSection: View {
    let presentation: AuthorizationPresentation

    var body: some View {
        if presentation.isEmpty {
            InspectorEmptyNote(text: "No authorization header")
        } else {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(presentation.entries) { entry in
                    FactList(facts: [
                        OverviewFact(name: entry.header, value: entry.summary),
                        OverviewFact(name: "Scheme", value: entry.scheme),
                    ])
                }
            }
        }
    }
}

/// A reconstructed message, labeled as one.
struct ReconstructedMessageView: View {
    let message: ReconstructedMessage

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(message.note)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)

            Text(message.text)
                .font(.system(size: 11, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(8)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 4))
        }
    }
}

/// The failure, in the summary and again on the response overview.
struct FailureSummary: View {
    let failure: FailurePresentation

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(failure.title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.red)
                .textSelection(.enabled)

            Text(failure.detail)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            FactList(facts: failure.facts.map {
                OverviewFact(name: $0.name, value: $0.value)
            })
            .padding(.top, 2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A section list beside the section that is open.
struct InspectorSectionSplit<Section: Hashable & Identifiable, Content: View>: View
where Section: RawRepresentable, Section.RawValue == String {
    @Binding var selection: Section
    let sections: [Section]
    @ViewBuilder var content: () -> Content

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 1) {
                    ForEach(sections) { section in
                        Button {
                            selection = section
                        } label: {
                            Text(section.rawValue)
                                .font(.system(size: 12, weight: selection == section ? .semibold : .regular))
                                .foregroundStyle(selection == section ? .primary : .secondary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 4)
                                .background(
                                    selection == section
                                        ? Color.accentColor.opacity(0.16)
                                        : Color.clear,
                                    in: RoundedRectangle(cornerRadius: 4)
                                )
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(selection == section ? .isSelected : [])
                    }
                }
                .padding(.vertical, 6)
                .padding(.horizontal, 6)
            }
            .scrollIndicators(.hidden)
            .frame(width: InspectorMetrics.railWidth)

            Divider()

            ScrollView {
                content()
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
