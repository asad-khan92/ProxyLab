//  Headers, as names and values.
//
//      Content-Type    application/json; charset=utf-8
//      Set-Cookie      session=abc; Path=/; HttpOnly
//      Set-Cookie      tracking=xyz              ← the same name twice
//
//  A list of pairs and not a table of unique keys: a message may carry the
//  same header twice, and collapsing the two would hide the thing being
//  debugged. Order is the order the message carried them in. A search
//  narrows that list and does not sort it.
//
//  Values are the stored values, `Authorization` included. The authorization
//  section is the masked summary. What a value is never put in is a log.
//
//  A very long value is folded to a few lines with a control to open it.

import SwiftUI

/// One message's headers, in the order they arrived.
struct HeaderListView: View {
    let presentation: HeaderPresentation
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    @State private var query = ""

    var body: some View {
        if presentation.rows.isEmpty {
            InspectorEmptyNote(text: "No headers")
        } else {
            VStack(alignment: .leading, spacing: 8) {
                toolbar
                if rows.isEmpty {
                    InspectorEmptyNote(text: "No matching headers")
                } else {
                    AdaptiveKeyColumn {
                        list(stacked: false)
                    } narrow: {
                        list(stacked: true)
                    }
                }
            }
        }
    }

    private var rows: [HeaderPresentation.Row] {
        presentation.matching(query)
    }

    private var toolbar: some View {
        HStack(spacing: 8) {
            TextField("Search", text: $query)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 11))
                .controlSize(.small)
                .accessibilityLabel("Search headers")

            if !query.isEmpty {
                Text("\(rows.count) of \(presentation.rows.count)")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .fixedSize()
            }

            Button("Copy All") {
                copy?(presentation.copyAll, "the headers")
            }
            .buttonStyle(.borderless)
            .font(.system(size: 11))
            .disabled(copy == nil)
            .help("Copy every header")
        }
    }

    private func list(stacked: Bool) -> some View {
        VStack(alignment: .leading, spacing: stacked ? 8 : 2) {
            ForEach(rows) { row in
                HeaderRow(row: row, stacked: stacked, copy: copy)
            }
        }
    }
}

/// One header: its name, and its value beside it.
private struct HeaderRow: View {
    let row: HeaderPresentation.Row
    let stacked: Bool
    let copy: ((String, String) -> Void)?

    /// How long a value has to be before it is folded.
    private static let foldingThreshold = 240

    @State private var expanded = false

    var body: some View {
        KeyValueRow(name: row.name, stacked: stacked) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.value)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(isLong && !expanded ? 4 : nil)

                if isLong {
                    Button(expanded ? "Show less" : "Show all") {
                        expanded.toggle()
                    }
                    .buttonStyle(.borderless)
                    .font(.system(size: 10))
                }
            }
        }
        .contextMenu {
            if let copy {
                Button("Copy Name") { copy(row.copyName, "the header name") }
                Button("Copy Value") { copy(row.copyValue, "the header value") }
                Button("Copy Header") { copy(row.copyHeader, "the header") }
            }
        }
    }

    private var isLong: Bool { row.value.count > Self.foldingThreshold }
}
