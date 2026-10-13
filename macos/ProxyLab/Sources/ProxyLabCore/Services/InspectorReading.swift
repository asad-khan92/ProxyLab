//  Everything the detail pane draws for one exchange, built once per selection.
//
//  The bodies were read when the exchange arrived. This joins that reading
//  with the headers, the query, and the cookies. A redraw of the pane uses
//  the value it already built. Selecting another exchange builds another.

import Foundation

/// The prepared text of one selected exchange.
struct InspectorReading: Equatable, Sendable {
    /// The exchange this reading was built from.
    let sourceID: String
    let headline: InspectorHeadline
    let requestFacts: [OverviewFact]
    let responseFacts: [OverviewFact]?
    /// Set when there is no response and no failure.
    let pendingDetail: String?
    let failure: FailurePresentation?
    let requestHeaders: HeaderPresentation
    let responseHeaders: HeaderPresentation
    let query: [QueryParameter]
    let requestCookies: CookiePresentation
    let responseCookies: CookiePresentation
    let authorization: AuthorizationPresentation
    let rawRequest: ReconstructedMessage
    let rawResponse: ReconstructedMessage?
    let requestEncoding: String?
    let responseEncoding: String?

    static func make(exchange: TrafficExchange, inspection: ExchangeInspection) -> InspectorReading {
        let failure = exchange.error.map { _ in FailurePresentation(exchange: exchange) }
        let pending = exchange.response == nil && failure == nil
            ? "The exchange has not finished, so there is no response yet."
            : nil

        return InspectorReading(
            sourceID: exchange.id,
            headline: ExchangeOverviewPresentation.headline(
                exchange: exchange,
                inspection: inspection
            ),
            requestFacts: ExchangeOverviewPresentation.requestFacts(exchange: exchange),
            responseFacts: ExchangeOverviewPresentation.responseFacts(
                exchange: exchange,
                inspection: inspection
            ),
            pendingDetail: pending,
            failure: failure,
            requestHeaders: HeaderPresentation(headers: exchange.request.headers),
            responseHeaders: HeaderPresentation(
                headers: exchange.response?.headers ?? Headers()
            ),
            query: QueryParameters.of(url: exchange.request.url),
            requestCookies: CookiePresentation.request(headers: exchange.request.headers),
            responseCookies: CookiePresentation.response(
                headers: exchange.response?.headers ?? Headers()
            ),
            authorization: AuthorizationPresentation(headers: exchange.request.headers),
            rawRequest: ReconstructedMessage.request(exchange, inspection: inspection.request),
            rawResponse: exchange.response.map {
                ReconstructedMessage.response($0, inspection: inspection.response)
            },
            requestEncoding: ExchangeOverviewPresentation.contentEncodings(exchange.request.headers),
            responseEncoding: exchange.response.flatMap {
                ExchangeOverviewPresentation.contentEncodings($0.headers)
            }
        )
    }
}
