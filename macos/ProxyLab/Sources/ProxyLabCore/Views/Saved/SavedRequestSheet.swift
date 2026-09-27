//  One saved request: what it is called, what it sends, and the two things
//  that can be done with it.
//
//      ┌─ Saved Request ──────────────────────────────┐
//      │ Name [ Create a user                      ]  │
//      │ POST ▾ [ https://example.com/users        ]  │
//      │ ─ Headers ──────────────────────────── + ──  │
//      │ [ Content-Type ] [ application/json    ] ×   │
//      │ ─ Body ───────────────────────────────────   │
//      │ {"name":"ada"}                               │
//      │                [ Close ] [ Save ] [ Replay ] │
//      └──────────────────────────────────────────────┘
//
//  The cURL import sheet with a name field on top and one more button at the
//  bottom. The editor in the middle is literally the same view — see
//  ``RequestEditorView`` — because it is the same thing: a request somebody is
//  about to send.
//
//  # Save and Replay are separate on purpose
//
//  Editing a saved request before sending it is the common case, and most of
//  those edits are not worth keeping: a different id in the path, a token
//  pasted in for one call. So Replay sends what is on screen and writes
//  nothing down, and Save is the deliberate act of changing what is kept.
//  Neither closes the sheet — a saved request is a thing to send more than
//  once.
//
//  # A body the capture cut short
//
//  Said at the top, once, rather than discovered when the server answers
//  differently: a request saved out of a large upload sends the bytes the
//  core kept, and that is less than the request that was captured.

import SwiftUI

/// The saved request sheet.
struct SavedRequestSheet: View {
    @Bindable var editor: SavedRequestEditor
    let model: SavedRequestsModel

    var body: some View {
        VStack(spacing: 0) {
            header

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    nameField
                    truncationNote
                    RequestEditorView(request: editor.request)
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

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "bookmark")
                .foregroundStyle(.secondary)
            Text("Saved Request")
                .font(.system(size: 13, weight: .semibold))
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var nameField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Name")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)

            TextField("What this request is for", text: $editor.name)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12))
                .accessibilityLabel("Saved request name")
        }
    }

    @ViewBuilder
    private var truncationNote: some View {
        if editor.sendsLessThanItWas {
            Label(
                "The captured body was truncated, so replaying this sends less than the "
                    + "original request did.",
                systemImage: "scissors"
            )
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var footer: some View {
        HStack(spacing: 10) {
            status

            Spacer()

            Button("Close") { model.closeEditor() }
                .keyboardShortcut(.cancelAction)

            Button("Save") {
                Task { await model.saveEdits() }
            }
            .disabled(!editor.hasName || editor.saveState == .saving)

            Button("Replay") {
                Task { await model.replayEdits() }
            }
            .keyboardShortcut(.defaultAction)
            .disabled(!editor.request.canReplay)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    @ViewBuilder
    private var status: some View {
        if case let .failed(reason) = editor.request.replayState {
            problem(reason)
        } else if case let .failed(reason) = editor.saveState {
            problem(reason)
        } else if editor.saveState == .saved {
            Label("Saved.", systemImage: "checkmark.circle")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        } else if editor.request.isTooLargeToSend {
            // Said here rather than left as a button that does nothing: a
            // disabled button with no explanation is the worst of both.
            Label(CurlImportModel.tooLargeMessage, systemImage: "exclamationmark.triangle")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .help(CurlImportModel.tooLargeMessage)
        }
    }

    private func problem(_ reason: String) -> some View {
        Label(reason, systemImage: "exclamationmark.triangle.fill")
            .font(.system(size: 11))
            .foregroundStyle(.red)
            .lineLimit(2)
            .help(reason)
    }
}

/// What to call the request being kept.
///
/// One field, so it is a small sheet rather than a full editor: a request
/// saved out of the traffic list is saved as it was captured, and changing it
/// is what the saved request sheet is for.
struct SaveRequestSheet: View {
    let model: SavedRequestsModel

    @Binding var prompt: SaveRequestPrompt

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 8) {
                Image(systemName: "bookmark")
                    .foregroundStyle(.secondary)
                Text("Save Request")
                    .font(.system(size: 13, weight: .semibold))
            }

            VStack(alignment: .leading, spacing: 6) {
                TextField("What this request is for", text: $prompt.name)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12))
                    .onSubmit { Task { await model.confirmSave() } }
                    .accessibilityLabel("Saved request name")

                Text("The request is kept as the core captured it — method, URL, headers, and "
                    + "body. The response is not.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()

                Button("Cancel") { model.cancelSave() }
                    .keyboardShortcut(.cancelAction)

                Button("Save") {
                    Task { await model.confirmSave() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(prompt.name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}
