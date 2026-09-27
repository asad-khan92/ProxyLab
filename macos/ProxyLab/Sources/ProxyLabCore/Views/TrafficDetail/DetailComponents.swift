//  The pieces the detail pane is built from.
//
//      ┌ HEADERS (12)                              [⧉] ┐
//      │ name      value                               │
//      └───────────────────────────────────────────────┘
//
//  A section, a two-column grid, and the small copy button that sits in a
//  section's title row. They are here rather than beside the views that use
//  them because the request half, the response half, and the overview are all
//  the same shapes with different contents in them, and a pane where a heading
//  is a different size in one tab than in another reads as two panes.
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
        VStack(alignment: .leading, spacing: 4) {
            ForEach(fields) { field in
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(field.name)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .frame(width: 122, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)

                    VStack(alignment: .leading, spacing: 1) {
                        Text(field.value)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)

                        if let note = field.note {
                            Text(note)
                                .font(.system(size: 10))
                                .foregroundStyle(.tertiary)
                                .textSelection(.enabled)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

/// The small button that copies one thing.
///
/// A symbol and a tooltip rather than a word, because these sit in title rows
/// beside each other and four buttons reading "Copy" say less than one.
struct CopyButton: View {
    let what: String
    let perform: () -> Void

    var body: some View {
        Button(action: perform) {
            Image(systemName: "doc.on.doc")
                .font(.system(size: 10))
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
        .help("Copy \(what)")
        .accessibilityLabel("Copy \(what)")
    }
}

/// One fact about an exchange, in the row under its URL.
struct MetaChip: View {
    let symbol: String?
    let text: String
    var help: String?

    var body: some View {
        HStack(spacing: 3) {
            if let symbol {
                Image(systemName: symbol)
                    .font(.system(size: 9))
            }
            Text(text)
                .font(.system(size: 10, design: .monospaced))
        }
        .foregroundStyle(.secondary)
        .help(help ?? text)
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
