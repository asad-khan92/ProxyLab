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
//  The captured URL is not changed by any of this. It is above, in the
//  headline, exactly as the client sent it.

import SwiftUI

/// One request's query parameters, in the order the URL carried them.
struct QueryParameterListView: View {
    let parameters: [QueryParameter]
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    var body: some View {
        if parameters.isEmpty {
            Text("No query parameters")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(parameters) { parameter in
                    QueryParameterRow(parameter: parameter, copy: copy)
                        .background(
                            parameter.position.isMultiple(of: 2)
                                ? Color.clear
                                : Color.primary.opacity(0.03)
                        )
                }
            }
            .textSelection(.enabled)
        }
    }
}

/// One parameter: its name, its decoded value, and the wire behind it.
private struct QueryParameterRow: View {
    let parameter: QueryParameter
    let copy: ((String, String) -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(parameter.name)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 150, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 1) {
                value

                // Only when the two differ, which is the only time the wire
                // tells a reader anything the decoded value does not.
                if parameter.wasEncoded, let raw = parameter.rawValue {
                    Text(raw)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .help("As the URL spelled it")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .contextMenu {
            if let copy {
                Button("Copy Value") { copy(parameter.value ?? "", "the value") }
                Button("Copy Name") { copy(parameter.name, "the parameter name") }
            }
        }
    }

    @ViewBuilder
    private var value: some View {
        if let value = parameter.value {
            if value.isEmpty {
                // An `=` with nothing after it. Not the same thing as a
                // parameter that had no `=`, and not the same thing as a
                // parameter that was not there.
                Text("(empty)")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            } else {
                Text(value)
                    .font(.system(size: 11, design: .monospaced))
            }
        } else {
            Text("(no value)")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        }
    }
}
