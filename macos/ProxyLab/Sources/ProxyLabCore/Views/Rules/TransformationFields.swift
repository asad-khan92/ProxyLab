//  The rows a Modify rule is written in.
//
//      [ Set    ▾] [ Authorization ] [ Bearer …          ] ×
//      [ Remove ▾] [ Cookie        ]                       ×
//                                                     + Add
//
//  The same shape as the request editor's header rows — a name field, a value
//  field, a button that takes the row away — with an operation in front,
//  because an edit is a thing done to a header rather than a header. Only the
//  rows are here: the label, the note, and the refusal under them are the rule
//  editor's own `Field`, so every part of the editor says what is wrong in one
//  way.

import SwiftUI

/// Header edits, one row each.
struct HeaderEditRows: View {
    @Binding var rows: [EditableHeaderEdit]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach($rows) { $row in
                HStack(spacing: 6) {
                    Picker("", selection: $row.operation) {
                        ForEach(HeaderEdit.Operation.allCases) { operation in
                            Text(operation.title).tag(operation)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 92)

                    TextField("Name", text: $row.name)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(width: 150)
                        .accessibilityLabel("Header name")

                    if row.operation == .remove {
                        Spacer()
                    } else {
                        TextField("Value", text: $row.value)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 11, design: .monospaced))
                            .accessibilityLabel("Header value")
                    }

                    RemoveRowButton { rows.removeAll { $0.id == row.id } }
                }
            }

            AddRowButton(title: "Add Header") { rows.append(EditableHeaderEdit()) }
        }
    }
}

/// Query parameter edits, one row each.
struct QueryEditRows: View {
    @Binding var rows: [EditableQueryEdit]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach($rows) { $row in
                HStack(spacing: 6) {
                    Picker("", selection: $row.operation) {
                        ForEach(QueryEdit.Operation.allCases) { operation in
                            Text(operation.title).tag(operation)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 92)

                    TextField("name", text: $row.name)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(width: 150)
                        .accessibilityLabel("Query parameter name")

                    if row.operation == .remove {
                        Spacer()
                    } else {
                        TextField("value", text: $row.value)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 11, design: .monospaced))
                            .accessibilityLabel("Query parameter value")
                    }

                    RemoveRowButton { rows.removeAll { $0.id == row.id } }
                }
            }

            AddRowButton(title: "Add Parameter") { rows.append(EditableQueryEdit()) }
        }
    }
}

/// What happens to a body: nothing, a replacement, or one JSON field.
struct BodyEditFields: View {
    @Binding var body_: BodyDraft

    init(body: Binding<BodyDraft>) {
        _body_ = body
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Picker("", selection: $body_.mode) {
                ForEach(BodyEditMode.allCases) { mode in
                    Text(mode.title).tag(mode)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 320)

            switch body_.mode {
            case .unchanged:
                EmptyView()
            case .replace:
                TextEditor(text: $body_.text)
                    .font(.system(size: 11, design: .monospaced))
                    .frame(minHeight: 90)
                    .overlay {
                        RoundedRectangle(cornerRadius: 5).stroke(.separator)
                    }
                    .accessibilityLabel("Replacement body")
            case .setJSON:
                HStack(spacing: 6) {
                    TextField("user.address.city", text: $body_.jsonPath)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(width: 180)
                        .accessibilityLabel("JSON field")
                    Text("=")
                        .foregroundStyle(.secondary)
                    TextField("\"staging\"", text: $body_.jsonValue)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .accessibilityLabel("JSON value")
                }
            }
        }
    }
}

private struct RemoveRowButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
        }
        .buttonStyle(.borderless)
        .help("Remove this edit")
        .accessibilityLabel("Remove this edit")
    }
}

private struct AddRowButton: View {
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: "plus")
                .font(.system(size: 11))
        }
        .buttonStyle(.borderless)
    }
}
