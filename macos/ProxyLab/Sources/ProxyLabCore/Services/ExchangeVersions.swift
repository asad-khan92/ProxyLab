//  How an exchange's two protocol versions read together.
//
//  Kept apart from the request and the response because they are decided
//  separately: the proxy speaks to the client in the version the client
//  offered and to the origin in the version the origin offered, and an
//  exchange where those differ is an ordinary exchange, not a mistake.

import Foundation

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
