//  One exchange, as one row.
//
//      GET   🔒 api.example.com                          200
//            /v1/users?active=true              84 ms  12:04:31.882
//
//  Two lines, and a deliberate split between them. The first is what was
//  asked and of whom — the part that is scanned down a column. The second is
//  which one and how it went, in a lighter weight, because a developer looking
//  for a request finds it by host and path and only then reads the numbers.
//
//  The status is the one thing that carries colour, so a page of rows shows
//  what failed without being read.

import SwiftUI

/// One row of the traffic list.
struct TrafficRow: View {
    let summary: ExchangeSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                MethodLabel(method: summary.method)
                SchemeMark(scheme: summary.scheme)

                Text(TrafficDisplay.host(summary))
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.head)

                Spacer(minLength: 8)

                OutcomeBadge(outcome: TrafficDisplay.outcome(summary))
            }

            HStack(spacing: 8) {
                Text(TrafficDisplay.path(summary.url))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)

                Spacer(minLength: 8)

                if let duration = TrafficDisplay.duration(
                    milliseconds: summary.durationMilliseconds
                ) {
                    Text(duration)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                }

                Text(TrafficDisplay.time(summary.startedAt))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
            .padding(.leading, 54)
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityDescription)
    }

    /// The row read out as one sentence, ending in how it went.
    private var accessibilityDescription: String {
        var description =
            "\(summary.method) \(TrafficDisplay.host(summary))\(TrafficDisplay.path(summary.url))"

        switch TrafficDisplay.outcome(summary) {
        case let .status(status): description += ", status \(status)"
        case .failed: description += ", failed"
        case .pending: description += ", no response yet"
        }

        return description
    }
}
