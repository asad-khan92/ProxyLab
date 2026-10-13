//  The one exchange being held, and the fields that change it.
//
//      POST  https://api.example.com/orders          Request held
//      ┌ Method ─┐ ┌ URL ─────────────────────────────────────┐
//      │ POST    │ │ https://api.example.com/orders           │
//      └─────────┘ └──────────────────────────────────────────┘
//      Headers                       Body
//      Accept: application/json      {"id":1}
//      Content-Type: application/json
//                                  [ Cancel ]  [ Continue ]
//
//  Editable text, not a form of switches: the headers are the header section
//  as it will be sent, one per line, and the body is the bytes as they will be
//  sent. That is the whole of what a breakpoint offers, and showing it as the
//  message keeps what is on screen and what goes on the wire the same thing.
//
//  Continue sends what is on screen. Cancel sends nothing at all: the request
//  never reaches the origin, or the response never reaches the client, and the
//  exchange appears in the traffic list saying so.

import SwiftUI

/// The exchange a person is being asked about.
struct PauseEditorView: View {
    let editor: PauseEditorModel
    let model: BreakpointsModel

    @Bindable private var bindable: PauseEditorModel

    init(editor: PauseEditorModel, model: BreakpointsModel) {
        self.editor = editor
        self.model = model
        bindable = editor
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if editor.direction != .websocket {
                        label("Overview")
                    }
                    line
                    if editor.direction == .request, showsQuery {
                        querySection
                    }
                    if editor.direction != .websocket {
                        headerSection
                    }
                    bodySection
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            Divider()
            actions
        }
    }

    // MARK: - What is held

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Label(
                PausedHoldLabel.phase(editor.direction),
                systemImage: "pause.fill"
            )
            .font(.system(size: 12, weight: .medium))

            Text(editor.paused.url)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)

            Spacer()

            if model.paused.count > 1 {
                holdPicker
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    /// The other exchanges still held, so this one can be chosen without
    /// losing them.
    private var holdPicker: some View {
        Menu {
            ForEach(model.paused) { pause in
                Button {
                    model.selectPause(pause.pauseID)
                } label: {
                    if pause.pauseID == editor.paused.pauseID {
                        Label(holdTitle(pause), systemImage: "checkmark")
                    } else {
                        Text(holdTitle(pause))
                    }
                }
            }
        } label: {
            Text(PausedHoldLabel.count(model.paused.count))
                .font(.system(size: 11))
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Choose which held exchange to edit")
    }

    private func holdTitle(_ pause: PausedExchange) -> String {
        let row = PausedTrafficRowContent(pause)

        return "\(row.title) · \(row.detail)"
    }

    private var showsQuery: Bool {
        URLComponents(string: editor.url)?.host != nil
    }

    /// The query, edited as part of the URL the core already accepts.
    private var querySection: some View {
        VStack(alignment: .leading, spacing: 4) {
            label("Query")
            TextField("Query", text: query, prompt: Text("none"))
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12, design: .monospaced))
            Text("Part of the URL. Leave it blank to send the request without a query.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
        }
    }

    private var query: Binding<String> {
        Binding(
            get: { PauseEditorModel.query(in: bindable.url) },
            set: { bindable.url = PauseEditorModel.url(bindable.url, replacingQuery: $0) }
        )
    }

    /// The request line, or the status.
    @ViewBuilder
    private var line: some View {
        switch editor.direction {
        case .request:
            HStack(alignment: .top, spacing: 10) {
                field("Method", width: 110) {
                    TextField("Method", text: $bindable.method)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 12, design: .monospaced))
                }
                field("URL", width: nil) {
                    TextField("URL", text: $bindable.url)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 12, design: .monospaced))
                }
            }
        case .response:
            HStack(alignment: .top, spacing: 10) {
                field("Status", width: 110) {
                    TextField("Status", text: $bindable.status)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 12, design: .monospaced))
                }
                VStack(alignment: .leading, spacing: 4) {
                    label("Request")
                    Text("\(editor.paused.method) \(editor.paused.url)")
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        case .websocket:
            VStack(alignment: .leading, spacing: 4) {
                label("Direction")
                Text(editor.paused.websocket?.direction == .clientToServer
                    ? "Client → server"
                    : "Server → client")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            label("Headers")
            TextEditor(text: $bindable.headerText)
                .font(.system(size: 11, design: .monospaced))
                .frame(minHeight: 110)
                .overlay(
                    RoundedRectangle(cornerRadius: 5)
                        .stroke(Color.secondary.opacity(0.3))
                )
            Text("One per line, written as “Name: value”. These replace the whole section.")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
        }
    }

    @ViewBuilder
    private var bodySection: some View {
        VStack(alignment: .leading, spacing: 4) {
            label(editor.direction == .websocket ? "Payload" : "Body")

            if let refusal = editor.bodyRefusal {
                Text(refusal)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 5))
            } else {
                TextEditor(text: $bindable.bodyText)
                    .font(.system(size: 11, design: .monospaced))
                    .frame(minHeight: 160)
                    .overlay(
                        RoundedRectangle(cornerRadius: 5)
                            .stroke(Color.secondary.opacity(0.3))
                    )
                Text(
                    editor.direction == .websocket
                        ? "Sent as written. The connection stays open."
                        : "Sent as written. Content-Length is set to match."
                )
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    // MARK: - Deciding

    private var actions: some View {
        HStack(spacing: 10) {
            if let refusal = editor.refusal {
                Text(refusal)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .lineLimit(2)
            }

            Spacer()

            Button(editor.direction == .websocket ? "Drop Message" : "Drop", role: .destructive) {
                Task { await model.cancelPause() }
            }
            .help(dropHelp)
            .disabled(isBusy)

            if editor.direction != .websocket {
                Button("Continue Without Changes") {
                    Task { await model.continueUnchanged() }
                }
                .help(
                    "Send the captured message. Edits on screen are left unsent, "
                        + "and a body too long to show is forwarded in full."
                )
                .disabled(isBusy)
            }

            Button("Continue") {
                Task { await model.continuePause() }
            }
            .keyboardShortcut(.defaultAction)
            .help(continueHelp)
            .disabled(isBusy || editor.refusal != nil)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.bar)
    }

    private var isBusy: Bool {
        model.busyIDs.contains(editor.paused.pauseID)
    }

    private var dropHelp: String {
        switch editor.direction {
        case .websocket:
            "Drop this text message. The connection stays open."
        case .request:
            "Stop this request. It is not sent, and the exchange is still recorded."
        case .response:
            "Stop this response. The client is not given it, and the exchange is still recorded."
        }
    }

    private var continueHelp: String {
        switch editor.direction {
        case .websocket:
            "Send the payload on screen."
        case .request:
            "Send the edited request."
        case .response:
            "Send the edited response."
        }
    }

    // MARK: - Bits

    private func label(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(.secondary)
            .textCase(.uppercase)
    }

    private func field<Content: View>(
        _ title: String,
        width: CGFloat?,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            label(title)
            content()
        }
        .frame(width: width)
    }
}
