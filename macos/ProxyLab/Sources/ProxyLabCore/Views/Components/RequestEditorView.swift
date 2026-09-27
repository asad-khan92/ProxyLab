//  One request, and everything that can be changed about it.
//
//      ┌──────────────────────────────────────────────┐
//      │ POST ▾ [ https://example.com/api          ]  │
//      │ ─ Headers ──────────────────────────── + ──  │
//      │ [ Content-Type ] [ application/json    ] ×   │
//      │ ─ Body ───────────────────────────────────   │
//      │ {"hello":"world"}                            │
//      └──────────────────────────────────────────────┘
//
//  The middle of two sheets: the cURL import sheet, where the request came
//  from a pasted command, and the saved request sheet, where it came out of
//  the core's store. It is one view because it is one thing — a request a
//  person is about to send — and two copies of it would be two places for the
//  header rows to drift apart.
//
//  # Nothing is decided here
//
//  The method is free text with a menu beside it rather than a picker: the
//  core sends any method that is an HTTP token, and a list that made `PURGE`
//  unreachable would be the interface deciding something the core does not.
//  Whether the URL is a URL is the core's to say, and it says so under the
//  field.
//
//  # A body that is not text
//
//  Shown as a size and not editable. The only edit that field could offer is
//  one that corrupts the bytes; sending them unchanged is what a developer
//  actually wants, and there is a button that throws them away for whoever
//  does not.

import SwiftUI

/// The method, the URL, the headers, and the body.
struct RequestEditorView: View {
    @Bindable var request: CurlImportModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            line
            headerRows
            bodyEditor
        }
    }

    /// The method and the URL, which are what a request is.
    private var line: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                TextField("Method", text: $request.method)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 11, design: .monospaced))
                    .frame(width: 96)
                    .accessibilityLabel("Request method")

                Menu {
                    ForEach(CurlImportModel.commonMethods, id: \.self) { method in
                        Button(method) { request.method = method }
                    }
                } label: {
                    Image(systemName: "chevron.up.chevron.down")
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Common methods")
                .accessibilityLabel("Choose a common method")

                TextField("https://example.com/api", text: $request.url)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 11, design: .monospaced))
                    .accessibilityLabel("Request URL")
            }

            if let problem = request.problem {
                Text(problem)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var headerRows: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Headers")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    request.addHeader()
                } label: {
                    Image(systemName: "plus")
                }
                .buttonStyle(.borderless)
                .help("Add a header")
                .accessibilityLabel("Add a header")
            }

            if request.headers.isEmpty {
                Text("No headers. The request is sent with only the Host this URL implies.")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }

            ForEach($request.headers) { $field in
                HStack(spacing: 6) {
                    TextField("Name", text: $field.name)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(width: 170)
                        .accessibilityLabel("Header name")

                    if field.isBinary {
                        Text("\(field.bytes?.count ?? 0) bytes that are not text")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        TextField("Value", text: $field.value)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 11, design: .monospaced))
                            .accessibilityLabel("Header value")
                    }

                    Button {
                        request.removeHeader(field.id)
                    } label: {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.borderless)
                    .help("Remove this header")
                    .accessibilityLabel("Remove this header")
                }
            }
        }
    }

    @ViewBuilder
    private var bodyEditor: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Body")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)

            if let description = request.binaryBodyDescription {
                HStack(spacing: 8) {
                    Label(description, systemImage: "doc.badge.ellipsis")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    Spacer()

                    Button("Replace with Text") { request.makeBodyEditable() }
                        .controlSize(.small)
                        .help("Throw the bytes away and type a body instead")
                }
                .padding(10)
                .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 5))
            } else {
                TextEditor(text: $request.bodyText)
                    .font(.system(size: 11, design: .monospaced))
                    .frame(minHeight: 110)
                    .overlay {
                        RoundedRectangle(cornerRadius: 5)
                            .stroke(.separator)
                    }
                    .accessibilityLabel("Request body")
            }
        }
    }
}
