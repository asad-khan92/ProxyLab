//  Importing a cURL command and sending the request it describes.
//
//      ┌─ Import cURL ────────────────────────────────┐
//      │ curl 'https://example.com/api' \             │
//      │   -H 'Accept: application/json'              │
//      │                                              │
//      │                        [ Cancel ] [ Import ] │
//      └──────────────────────────────────────────────┘
//
//      ┌─ Replay Request ─────────────────────────────┐
//      │ POST ▾ [ https://example.com/api          ]  │
//      │ ─ Headers ──────────────────────────── + ──  │
//      │ [ Content-Type ] [ application/json    ] ×   │
//      │ ─ Body ───────────────────────────────────   │
//      │ {"hello":"world"}                            │
//      │                       [ Cancel ] [ Replay ]  │
//      └──────────────────────────────────────────────┘
//
//  One sheet with two faces, because it is one task: a command goes in and a
//  request comes out the other side. It is deliberately not a request
//  workspace — no tabs, no collections, no environments, no saved history —
//  and nothing in it survives the sheet being closed.
//
//  The design language is the rule editor's, for the same reasons: a header
//  above, a scrolling form in the middle, and the two buttons at the bottom
//  with the reason anything was refused beside them.

import SwiftUI

/// The cURL import sheet.
struct CurlImportView: View {
    @Bindable var importer: CurlImportModel
    let model: TrafficInspectorModel

    var body: some View {
        VStack(spacing: 0) {
            header

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    switch importer.stage {
                    case .pasting:
                        paste
                    case .editing:
                        editor
                    }
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(minHeight: 320)

            Divider()

            footer
        }
        .frame(width: 560)
        .frame(maxHeight: 680)
    }

    // MARK: - Chrome

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: importer.stage == .pasting ? "doc.on.clipboard" : "paperplane")
                .foregroundStyle(.secondary)
            Text(importer.stage == .pasting ? "Import cURL" : "Replay Request")
                .font(.system(size: 13, weight: .semibold))
            Spacer()

            if importer.stage == .editing {
                Button("Back to Command") { importer.backToCommand() }
                    .controlSize(.small)
                    .buttonStyle(.link)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if case let .failed(reason) = importer.replayState {
                Label(reason, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .lineLimit(2)
                    .help(reason)
            } else if let reason = tooLarge {
                // Said here rather than left as a button that does nothing:
                // a disabled button with no explanation is the worst of both.
                Label(reason, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .help(reason)
            }

            Spacer()

            Button("Cancel") { model.cancelImport() }
                .keyboardShortcut(.cancelAction)

            switch importer.stage {
            case .pasting:
                Button("Import") {
                    Task { await importer.parse() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!importer.canParse)
            case .editing:
                Button("Replay") {
                    Task { await model.replayImported() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!importer.canReplay)
            }

            if importer.isParsing || importer.replayState == .sending {
                ProgressView().controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    /// Why the button is disabled, when it is disabled for size.
    private var tooLarge: String? {
        switch importer.stage {
        case .pasting: importer.isCommandTooLarge ? CurlImportModel.tooLargeMessage : nil
        case .editing: importer.isTooLargeToSend ? CurlImportModel.tooLargeMessage : nil
        }
    }

    // MARK: - Pasting

    private var paste: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Paste a cURL command")
                .font(.system(size: 12, weight: .medium))
            Text(
                "ProxyLab reads the command as text. It is never run, and a file or a shell "
                    + "command written into it is refused rather than carried out."
            )
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)

            TextEditor(text: $importer.curlText)
                .font(.system(size: 11, design: .monospaced))
                .frame(minHeight: 180)
                .overlay {
                    RoundedRectangle(cornerRadius: 5)
                        .stroke(.separator)
                }

            if let problem = importer.problem {
                Label(problem, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    // MARK: - Editing

    private var editor: some View {
        VStack(alignment: .leading, spacing: 18) {
            notes
            // The same editor the saved request sheet shows, because it is
            // the same thing: a request somebody is about to send.
            RequestEditorView(request: importer)
        }
    }

    @ViewBuilder
    private var notes: some View {
        let messages = importer.notes.compactMap(\.message)

        if !messages.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(messages, id: \.self) { message in
                    Label(message, systemImage: "info.circle")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}
