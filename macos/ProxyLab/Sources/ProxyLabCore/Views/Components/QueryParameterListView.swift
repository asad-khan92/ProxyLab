//  The query string, read as parameters.
//
//      page         2
//      tag          a
//      tag          b          ← the same name twice, kept twice
//      flag         (no value) ← no `=` at all
//      empty        (empty)    ← an `=` with nothing after it
//      q            iPhone 17
//                   iPhone+17  ← what was on the wire, when it differs
//
//  A list of pairs and not a dictionary, for the same reason the headers are:
//  a query may carry the same name more than once, and which value came first
//  is occasionally the thing being debugged.
//
//  The captured URL is not changed by any of this. Decoding is
//  `QueryParameters`, which is also what the overview's URL still shows raw.

import SwiftUI

/// One request's query parameters, in the order the URL carried them.
struct QueryParameterListView: View {
    let parameters: [QueryParameter]
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    var body: some View {
        if parameters.isEmpty {
            InspectorEmptyNote(text: "No query parameters")
        } else {
            AdaptiveKeyColumn {
                list(stacked: false)
            } narrow: {
                list(stacked: true)
            }
        }
    }

    private func list(stacked: Bool) -> some View {
        VStack(alignment: .leading, spacing: stacked ? 8 : 2) {
            ForEach(parameters) { parameter in
                QueryParameterRow(parameter: parameter, stacked: stacked, copy: copy)
            }
        }
    }
}

/// One parameter: its name, its decoded value, and the wire behind it.
private struct QueryParameterRow: View {
    let parameter: QueryParameter
    let stacked: Bool
    let copy: ((String, String) -> Void)?

    var body: some View {
        KeyValueRow(name: parameter.name, stacked: stacked, note: wireNote) {
            value
        }
        .contextMenu {
            if let copy {
                Button("Copy Name") { copy(parameter.name, "the parameter name") }
                Button("Copy Value") { copy(parameter.value ?? "", "the value") }
                Button("Copy Parameter") { copy(parameter.parameterText, "the parameter") }
            }
        }
    }

    /// The wire spelling, and only when it differs from the decoded value.
    private var wireNote: String? {
        guard parameter.wasEncoded, let raw = parameter.rawValue else { return nil }

        return raw
    }

    @ViewBuilder
    private var value: some View {
        if let value = parameter.value {
            if value.isEmpty {
                Text("(empty)")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            } else {
                Text(value)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
            }
        } else {
            Text("(no value)")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        }
    }
}
