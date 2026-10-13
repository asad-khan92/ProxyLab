//  One exchange, as one row.
//
//  Two lines, and the same two lines at every width, so a burst of new
//  traffic does not change how tall the rows already on screen are. What
//  changes with the column is which trailing facts are present, and that
//  decision is `TrafficRowPresentation`'s.
//
//  Status is coloured text and nothing behind it. A tinted row would sit
//  under the list's selection and win, in both appearances.

import AppKit
import SwiftUI

/// One row of the traffic list.
struct TrafficRow: View {
    let summary: ExchangeSummary
    var width: TrafficRowWidth
    var domainSelected: Bool

    var body: some View {
        let row = TrafficRowPresentation(
            summary: summary,
            width: width,
            domainSelected: domainSelected
        )

        VStack(alignment: .leading, spacing: 1) {
            identityLine(row)
            pathLine(row)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(row.accessibilityLabel)
    }

    private func identityLine(_ row: TrafficRowPresentation) -> some View {
        HStack(spacing: Column.gap) {
            slot(row.method, width: Column.method, weight: .semibold, color: .primary)
            status(row.status)
            host(row)

            Spacer(minLength: Column.gap)

            if !row.indicators.isEmpty {
                indicators(row.indicators)
                    .layoutPriority(1)
            }
            if let time = row.time {
                metric(time, width: Column.time)
                    .layoutPriority(1)
            }
            if !row.durationFollowsPath {
                metric(row.duration ?? "", width: Column.duration)
                    .layoutPriority(1)
            }
        }
    }

    private func pathLine(_ row: TrafficRowPresentation) -> some View {
        HStack(spacing: Column.gap) {
            if row.pathAlignsWithHost {
                // A fixed indent, not a flexible colour. A colour offered the
                // row's height will take it, and the row grows with the list.
                Color.clear.frame(width: Column.pathIndent, height: 1)
            }

            Text(row.path)
                .font(
                    .system(
                        size: 11,
                        weight: row.hostIsQuiet ? .medium : .regular,
                        design: .monospaced
                    )
                )
                .foregroundStyle(row.hostIsQuiet ? .primary : .secondary)
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: Column.gap)

            if row.durationFollowsPath {
                metric(row.duration ?? "", width: Column.duration)
                    .layoutPriority(1)
            }
        }
    }

    private func status(_ status: TrafficRowPresentation.Status) -> some View {
        slot(status.text, width: Column.status, weight: .semibold, color: color(for: status.tone))
            .help(status.help)
    }

    private func host(_ row: TrafficRowPresentation) -> some View {
        Text(row.host)
            .font(.system(size: 12, weight: row.hostIsQuiet ? .regular : .medium))
            .foregroundStyle(row.hostIsQuiet ? .tertiary : .primary)
            .lineLimit(1)
            .truncationMode(.head)
            .frame(minWidth: 36, alignment: .leading)
    }

    private func indicators(_ indicators: [TrafficRowPresentation.Indicator]) -> some View {
        HStack(spacing: 8) {
            ForEach(indicators) { indicator in
                Text(indicator.label)
                    .font(.system(size: 10, weight: weight(for: indicator.role)))
                    .foregroundStyle(color(for: indicator.role))
                    .lineLimit(1)
                    .help(indicator.help)
            }
        }
        .fixedSize(horizontal: true, vertical: false)
    }

    private func slot(
        _ text: String,
        width: CGFloat,
        weight: Font.Weight,
        color: Color
    ) -> some View {
        Text(text)
            .font(.system(size: 11, weight: weight, design: .monospaced))
            .foregroundStyle(color)
            .lineLimit(1)
            .frame(width: width, alignment: .leading)
    }

    private func metric(_ text: String, width: CGFloat) -> some View {
        Text(text)
            .font(.system(size: 10, design: .monospaced))
            .foregroundStyle(.tertiary)
            .lineLimit(1)
            .frame(width: width, alignment: .trailing)
    }

    private func color(for tone: TrafficRowPresentation.Status.Tone) -> Color {
        switch tone {
        case .success: Color(nsColor: .systemGreen)
        case .redirect: Color(nsColor: .systemTeal)
        case .clientError: Color(nsColor: .systemOrange)
        case .serverError, .failed: Color(nsColor: .systemRed)
        case .informational, .pending, .other: Color.secondary
        }
    }

    private func color(for role: TrafficRowPresentation.Indicator.Role) -> Color {
        switch role {
        case .websocket, .sse, .failure: Color.secondary
        case .content, .version: Color.secondary.opacity(0.85)
        }
    }

    private func weight(for role: TrafficRowPresentation.Indicator.Role) -> Font.Weight {
        switch role {
        case .websocket, .sse, .failure: .semibold
        case .content, .version: .medium
        }
    }

    /// Fixed columns, so method, status, and the path under the host line up
    /// down the list. The path's indent is the method column, the gap, and
    /// the status column — the host starts one gap after that.
    private enum Column {
        static let method: CGFloat = 50
        static let status: CGFloat = 54
        static let gap: CGFloat = 6
        static let duration: CGFloat = 64
        static let time: CGFloat = 78
        static var pathIndent: CGFloat { method + gap + status }
    }
}
