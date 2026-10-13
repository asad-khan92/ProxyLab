//  The pieces the detail pane is built from.
//
//  A section title, a key and its value, and the timing bar. The key stays a
//  fixed width when there is room and sits above the value when there is not.
//
//  Nothing here chooses a colour. The pane is drawn in the system's own
//  hierarchy of label styles, so it follows the appearance the person set
//  rather than a palette written into this file.

import SwiftUI

/// A titled block of the detail pane, with room for an action beside the title.
struct DetailSection<Content: View, Accessory: View>: View {
    let title: String
    /// What sits at the end of the title row: a copy button, usually.
    @ViewBuilder var accessory: Accessory
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title.uppercased())
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .kerning(0.6)

                Spacer(minLength: 8)

                accessory
            }

            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

extension DetailSection where Accessory == EmptyView {
    init(title: String, @ViewBuilder content: () -> Content) {
        self.init(title: title, accessory: { EmptyView() }, content: content)
    }
}

/// Name/value pairs, aligned in a column.
struct FieldGrid: View {
    let fields: [Field]

    /// One row of the grid.
    struct Field: Identifiable {
        let name: String
        let value: String
        /// Shown under the value, in the way a caption is.
        var note: String?

        var id: String { name }
    }

    init(_ fields: [Field]) {
        self.fields = fields
    }

    var body: some View {
        AdaptiveKeyColumn {
            rows(stacked: false)
        } narrow: {
            rows(stacked: true)
        }
    }

    private func rows(stacked: Bool) -> some View {
        VStack(alignment: .leading, spacing: stacked ? 8 : 4) {
            ForEach(fields) { field in
                KeyValueRow(
                    name: field.name,
                    stacked: stacked,
                    note: field.note
                ) {
                    Text(field.value)
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
        }
    }
}

/// A list of overview facts. The key column stays a fixed width.
struct FactList: View {
    let facts: [OverviewFact]

    var body: some View {
        AdaptiveKeyColumn {
            rows(stacked: false)
        } narrow: {
            rows(stacked: true)
        }
    }

    private func rows(stacked: Bool) -> some View {
        VStack(alignment: .leading, spacing: stacked ? 8 : 4) {
            ForEach(facts) { fact in
                KeyValueRow(name: fact.name, stacked: stacked, note: fact.note) {
                    Text(fact.value)
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
        }
    }
}

/// Columns when the content is wide enough. A key above its value when it is not.
struct AdaptiveKeyColumn<Wide: View, Narrow: View>: View {
    @ViewBuilder var wide: () -> Wide
    @ViewBuilder var narrow: () -> Narrow

    var body: some View {
        ViewThatFits(in: .horizontal) {
            wide()
                .frame(minWidth: InspectorMetrics.stackBreakpoint, alignment: .leading)
            narrow()
        }
    }
}

/// One name and the value beside it, or under it.
struct KeyValueRow<Value: View>: View {
    let name: String
    var stacked: Bool
    var note: String?
    @ViewBuilder var value: () -> Value

    var body: some View {
        if stacked {
            VStack(alignment: .leading, spacing: 1) {
                nameLabel
                valueBlock
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                nameLabel
                    .frame(width: InspectorMetrics.keyWidth, alignment: .leading)
                valueBlock
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private var nameLabel: some View {
        Text(name)
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var valueBlock: some View {
        VStack(alignment: .leading, spacing: 1) {
            value()
                .fixedSize(horizontal: false, vertical: true)
            if let note {
                Text(note)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// A single quiet line for a section that has nothing to show.
struct InspectorEmptyNote: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11))
            .foregroundStyle(.tertiary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The measured stretches of an exchange, drawn to scale.
///
/// Two segments at most, and only ones the core actually timed: everything up
/// to the response head, and the body after it. An exchange with nothing
/// measured draws nothing rather than a bar that implies a measurement.
struct TimingBar: View {
    let spans: [ExchangeTiming.Span]

    private var total: Double {
        max(1, Double(spans.reduce(UInt64(0)) { $0 + $1.micros }))
    }

    var body: some View {
        if !spans.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                GeometryReader { geometry in
                    HStack(spacing: 1) {
                        ForEach(Array(spans.enumerated()), id: \.element.id) { index, span in
                            RoundedRectangle(cornerRadius: 2)
                                .fill(index == 0 ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                                .frame(
                                    width: max(
                                        2,
                                        geometry.size.width * Double(span.micros) / total
                                    )
                                )
                        }
                    }
                }
                .frame(height: 6)

                HStack(spacing: 12) {
                    ForEach(Array(spans.enumerated()), id: \.element.id) { index, span in
                        HStack(spacing: 4) {
                            Circle()
                                .fill(index == 0 ? AnyShapeStyle(.tint) : AnyShapeStyle(.tertiary))
                                .frame(width: 5, height: 5)
                            Text("\(span.name) \(ExchangeTiming.duration(micros: span.micros))")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            .accessibilityLabel(
                spans
                    .map { "\($0.name) \(ExchangeTiming.duration(micros: $0.micros))" }
                    .joined(separator: ", ")
            )
        }
    }
}
