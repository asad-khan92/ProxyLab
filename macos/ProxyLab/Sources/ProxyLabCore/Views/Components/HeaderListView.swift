//  Headers, as names and values.
//
//      Content-Type    application/json; charset=utf-8
//      Set-Cookie      session=abc; Path=/; HttpOnly
//      Set-Cookie      tracking=xyz              ← the same name twice
//      Authorization   Bearer eyJhbGciOi…        ← shown, never logged
//
//  A list of pairs and not a table of unique keys: a message may carry the
//  same header twice, and collapsing the two would hide the thing being
//  debugged. Order is the order the message carried them in.
//
//  Values are shown in full, `Authorization` and `Cookie` included. That is
//  what a debugging proxy is for. What they are never put in is a log.
//
//  A very long value — a token, a `Set-Cookie` with a dozen attributes — is
//  folded to a few lines with a control to open it, rather than being cut or
//  being allowed to push everything under it off the pane.

import SwiftUI

/// One message's headers, in the order they arrived.
struct HeaderListView: View {
    let headers: Headers
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    var body: some View {
        if headers.isEmpty {
            Text("No headers")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(headers.entries.enumerated()), id: \.offset) { position, header in
                    HeaderRow(header: header, copy: copy)
                        .background(
                            position.isMultiple(of: 2)
                                ? Color.clear
                                : Color.primary.opacity(0.03)
                        )
                }
            }
            .textSelection(.enabled)
        }
    }
}

/// One header: its name, and its value beside it.
private struct HeaderRow: View {
    let header: Header
    let copy: ((String, String) -> Void)?

    /// How long a value has to be before it is folded.
    ///
    /// About four lines at the pane's usual width. Shorter than this and a
    /// control to expand it would be in the way of reading it.
    private static let foldingThreshold = 240

    @State private var expanded = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(header.name)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 150, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 2) {
                value

                if isLong {
                    Button(expanded ? "Show less" : "Show all \(header.value.byteCount) bytes") {
                        expanded.toggle()
                    }
                    .buttonStyle(.borderless)
                    .font(.system(size: 10))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .contextMenu {
            if let copy {
                Button("Copy Value") { copy(header.value.text ?? "", "the header value") }
                Button("Copy Header") {
                    copy(ExchangeText.headers(Headers([header])), "the header")
                }
            }
        }
    }

    private var isLong: Bool {
        guard case let .text(text) = header.value else { return false }

        return text.count > Self.foldingThreshold
    }

    @ViewBuilder
    private var value: some View {
        switch header.value {
        case let .text(text):
            Text(text)
                .font(.system(size: 11, design: .monospaced))
                .lineLimit(isLong && !expanded ? 4 : nil)
        case let .bytes(bytes):
            // Not valid UTF-8, so there is nothing honest to print. The size
            // is what can be said about it.
            Text("\(bytes.count) bytes, not text")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.tertiary)
        }
    }
}
