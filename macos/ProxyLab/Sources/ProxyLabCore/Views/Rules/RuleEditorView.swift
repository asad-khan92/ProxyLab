//  Writing a rule, or changing one.
//
//      ┌─ New Rule ───────────────────────────────────┐
//      │ Name     [ Block analytics            ]      │
//      │ Priority [ 10 ]   Enabled  (•)               │
//      │ ─ Matches ─────────────────────────────────  │
//      │ Host     [ Any host                   ]      │
//      │ Path     [ Any path      ] ( ) prefix        │
//      │ Method   [ Any method   ▾ ]                  │
//      │ ─ Block ───────────────────────────────────  │
//      │ Answers  [ 403 ]                             │
//      │                          [ Cancel ] [ Save ] │
//      └──────────────────────────────────────────────┘
//
//  One editor for every kind of rule: the name, the order, and the three
//  matching criteria are the same whatever the rule does, and only the last
//  section changes. A separate editor per action would be several copies of
//  the matcher, and several places for them to drift apart.
//
//  The action is a menu rather than a segmented control, because there are
//  seven of them and they are two kinds of thing: the five that decide what
//  happens to a request, and the two Modify rules, which change it and let
//  the rules below them decide.
//
//  Every field says what leaving it empty means. "Any host" is not a
//  placeholder for a host someone forgot to type — it is what the rule does.

import SwiftUI

/// The rule editor, as a sheet.
struct RuleEditorView: View {
    @Bindable var editor: RuleEditorModel
    let model: RulesModel

    @FocusState private var focus: RuleEditorField?

    var body: some View {
        VStack(spacing: 0) {
            header

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    basics
                    matching
                    action
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(minHeight: 300)

            Divider()

            footer
        }
        .frame(width: 520)
        .frame(maxHeight: 640)
        .disabled(editor.isGone)
        .overlay { if editor.isGone { goneOverlay } }
    }

    // MARK: - Chrome

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: editor.actionKind.symbol)
                .foregroundStyle(.secondary)
            Text(editor.title)
                .font(.system(size: 13, weight: .semibold))
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if case let .failed(reason) = editor.saveState {
                Label(reason, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .lineLimit(2)
                    .help(reason)
            }

            Spacer()

            Button("Cancel") { model.cancelEditing() }
                .keyboardShortcut(.cancelAction)

            Button(editor.saveTitle) {
                Task { await model.saveEditor() }
            }
            .keyboardShortcut(.defaultAction)
            .disabled(editor.isSaving || !editor.hasChanges)

            if editor.isSaving {
                ProgressView().controlSize(.small)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    /// What is shown when the rule was removed while it was being edited.
    ///
    /// Nothing is saved over it and nothing is recreated: the rule is gone, and
    /// the only honest thing to offer is to close the editor and look again.
    private var goneOverlay: some View {
        ZStack {
            Rectangle().fill(.regularMaterial)
            PlaceholderView(
                symbol: "questionmark.circle",
                title: "That rule no longer exists",
                detail: "It was removed while it was open here. Nothing was saved over it.",
                action: (
                    "Close",
                    {
                        model.cancelEditing()
                        Task { await model.load() }
                    }
                )
            )
        }
    }

    // MARK: - Sections

    private var basics: some View {
        Section {
            Field("Name", error: editor.fieldErrors[.name]) {
                TextField("What this rule is called", text: $editor.name)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .name)
            }

            HStack(alignment: .top, spacing: 16) {
                Field(
                    "Priority",
                    error: editor.fieldErrors[.priority],
                    note: "Lower is consulted first."
                ) {
                    TextField("0", text: $editor.priority)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                        .focused($focus, equals: .priority)
                }

                Field("Enabled", error: nil, note: "A disabled rule is not consulted.") {
                    Toggle("", isOn: $editor.enabled)
                        .toggleStyle(.switch)
                        .controlSize(.small)
                        .labelsHidden()
                }

                Spacer()
            }
        } heading: {
            Heading("Rule", note: nil)
        }
    }

    private var matching: some View {
        Section {
            Field(
                "Host",
                error: editor.fieldErrors[.host],
                note: "Exact, case-insensitive. The port is never compared."
            ) {
                TextField("Any host", text: $editor.host)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .host)
            }

            Field(
                "Path",
                error: editor.fieldErrors[.path],
                note: editor.pathIsPrefix
                    ? "Every path that starts with this. A string prefix, not a path "
                        + "segment: write the trailing slash to mean the folder."
                    : "Exact, case-sensitive. The query is never compared."
            ) {
                HStack(spacing: 10) {
                    TextField("Any path", text: $editor.path)
                        .textFieldStyle(.roundedBorder)
                        .focused($focus, equals: .path)
                    Toggle("Starts with", isOn: $editor.pathIsPrefix)
                        .controlSize(.small)
                        .fixedSize()
                }
            }

            Field(
                "Method",
                error: editor.fieldErrors[.method],
                note: "Exact, case-insensitive."
            ) {
                Picker("", selection: $editor.method) {
                    Text("Any method").tag("")
                    Divider()
                    ForEach(editor.methodChoices, id: \.self) { method in
                        Text(method).tag(method)
                    }
                }
                .labelsHidden()
                .frame(width: 160)
            }
        } heading: {
            Heading(
                "Matches",
                note: "Every criterion that is set must match. One left empty matches "
                    + "anything — a rule with none set matches every request."
            )
        }
    }

    @ViewBuilder
    private var action: some View {
        Section {
            Picker("", selection: $editor.actionKind) {
                ForEach(RuleActionKind.allCases.filter { !$0.isTransformation }) { kind in
                    Label(kind.title, systemImage: kind.symbol).tag(kind)
                }
                Divider()
                ForEach(RuleActionKind.allCases.filter(\.isTransformation)) { kind in
                    Label(kind.title, systemImage: kind.symbol).tag(kind)
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .frame(width: 220)

            Text(editor.actionKind.explanation)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            switch editor.actionKind {
            case .allow:
                EmptyView()
            case .block:
                blockFields
            case .mapLocal:
                mapLocalFields
            case .mapRemote:
                mapRemoteFields
            case .redirect:
                redirectFields
            case .modifyRequest:
                modifyRequestFields
            case .modifyResponse:
                modifyResponseFields
            }
        } heading: {
            Heading("Action", note: nil)
        }
    }

    private var blockFields: some View {
        Field(
            "Answers",
            error: editor.fieldErrors[.blockStatus],
            note: "The body is one fixed sentence the core writes. The origin is never "
                + "contacted."
        ) {
            TextField("403", text: $editor.blockStatus)
                .textFieldStyle(.roundedBorder)
                .frame(width: 90)
                .focused($focus, equals: .blockStatus)
        }
    }

    private var mapLocalFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            Field(
                "File",
                error: editor.fieldErrors[.localPath],
                note: "The core reads this file each time the rule answers. Nothing from "
                    + "the request is joined to the path."
            ) {
                HStack(spacing: 10) {
                    Text(
                        editor.localPath.isEmpty ? "No file chosen" : editor.localPathForDisplay
                    )
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(editor.localPath.isEmpty ? .tertiary : .primary)
                    .lineLimit(1)
                    .truncationMode(.head)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)

                    Button("Choose File…") { editor.chooseLocalFile() }
                        .controlSize(.small)
                }
            }

            Field(
                "Content type",
                error: editor.fieldErrors[.localContentType],
                note: "Worked out from the file's extension when left empty."
            ) {
                TextField("From the file's extension", text: $editor.localContentType)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .localContentType)
            }

            Field("Answers", error: editor.fieldErrors[.localStatus], note: nil) {
                TextField("200", text: $editor.localStatus)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 90)
                    .focused($focus, equals: .localStatus)
            }
        }
    }

    private var mapRemoteFields: some View {
        Field(
            "Destination",
            error: editor.fieldErrors[.destination],
            note: "http or https, with an optional port, path, and query. A path here "
                + "replaces the request's; leave it off to keep the request's own."
        ) {
            TextField("https://staging.example.com", text: $editor.destination)
                .textFieldStyle(.roundedBorder)
                .focused($focus, equals: .destination)
        }
    }
}

extension RuleEditorView {
    fileprivate var redirectFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            Field(
                "Location",
                error: editor.fieldErrors[.redirectURL],
                note: "http or https. Sent to the client exactly as written: nothing from the "
                    + "request is carried over, and the origin is never contacted."
            ) {
                TextField("https://new.example.com/path", text: $editor.redirectURL)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .redirectURL)
            }

            Field(
                "Answers",
                error: nil,
                note: "307 and 308 keep the method and body; 301 and 308 are cached by browsers."
            ) {
                Picker("", selection: $editor.redirectStatus) {
                    ForEach(RedirectAction.statuses, id: \.self) { status in
                        Text(Self.redirectTitle(status)).tag(status)
                    }
                }
                .labelsHidden()
                .frame(width: 220)
            }
        }
    }

    fileprivate var modifyRequestFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            transformationProblem

            Field(
                "Method",
                error: editor.fieldErrors[.modifyMethod],
                note: "Empty keeps the request's own."
            ) {
                HStack(spacing: 6) {
                    TextField("Unchanged", text: $editor.modifyRequest.method)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .frame(width: 120)
                        .focused($focus, equals: .modifyMethod)
                    Menu {
                        ForEach(commonHTTPMethods, id: \.self) { method in
                            Button(method) { editor.modifyRequest.method = method }
                        }
                    } label: {
                        Image(systemName: "chevron.up.chevron.down")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                    .help("Common methods")
                }
            }

            Field(
                "Path",
                error: editor.fieldErrors[.modifyPath],
                note: editor.modifyRequest.pathMode == .replacePrefix
                    ? "A string prefix, like the matcher's: write the trailing slash on both to "
                        + "mean the folder."
                    : "The host never changes here; sending a request somewhere else is Map "
                        + "Remote's."
            ) {
                VStack(alignment: .leading, spacing: 6) {
                    Picker("", selection: $editor.modifyRequest.pathMode) {
                        ForEach(PathEditMode.allCases) { mode in
                            Text(mode.title).tag(mode)
                        }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 320)

                    switch editor.modifyRequest.pathMode {
                    case .unchanged:
                        EmptyView()
                    case .set:
                        TextField("/v2/users", text: $editor.modifyRequest.path)
                            .textFieldStyle(.roundedBorder)
                            .font(.system(size: 11, design: .monospaced))
                    case .replacePrefix:
                        HStack(spacing: 6) {
                            TextField("/api/v1/", text: $editor.modifyRequest.pathFrom)
                                .textFieldStyle(.roundedBorder)
                                .font(.system(size: 11, design: .monospaced))
                            Image(systemName: "arrow.right")
                                .foregroundStyle(.secondary)
                            TextField("/api/v2/", text: $editor.modifyRequest.pathTo)
                                .textFieldStyle(.roundedBorder)
                                .font(.system(size: 11, design: .monospaced))
                        }
                    }
                }
            }

            Field(
                "Query parameters",
                error: editor.fieldErrors[.modifyQuery],
                note: "Written as they appear in the URL: nothing is percent-encoded for you."
            ) {
                QueryEditRows(rows: $editor.modifyRequest.query)
            }

            Field(
                "Headers",
                error: editor.fieldErrors[.requestHeaders],
                note: "Applied in order. Host and the framing headers are the proxy's own."
            ) {
                HeaderEditRows(rows: $editor.modifyRequest.headers)
            }

            Field(
                "Body",
                error: editor.fieldErrors[.requestBody],
                note: bodyNote(editor.modifyRequest.body)
            ) {
                BodyEditFields(body: $editor.modifyRequest.body)
            }
        }
    }

    fileprivate var modifyResponseFields: some View {
        VStack(alignment: .leading, spacing: 12) {
            transformationProblem

            Field(
                "Status",
                error: editor.fieldErrors[.responseStatus],
                note: "Empty keeps the origin's. Between 200 and 599."
            ) {
                TextField("Unchanged", text: $editor.modifyResponse.status)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 90)
                    .focused($focus, equals: .responseStatus)
            }

            Field(
                "Headers",
                error: editor.fieldErrors[.responseHeaders],
                note: "Applied in order. The framing headers are the proxy's own."
            ) {
                HeaderEditRows(rows: $editor.modifyResponse.headers)
            }

            Field(
                "Body",
                error: editor.fieldErrors[.responseBody],
                note: bodyNote(editor.modifyResponse.body)
            ) {
                BodyEditFields(body: $editor.modifyResponse.body)
            }
        }
    }

    /// A transformation that changes nothing, said at the top of its section.
    @ViewBuilder
    fileprivate var transformationProblem: some View {
        if let problem = editor.fieldErrors[.transformation] {
            Label(problem, systemImage: "exclamationmark.circle.fill")
                .font(.system(size: 10))
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    fileprivate func bodyNote(_ body: BodyDraft) -> String? {
        switch body.mode {
        case .unchanged:
            nil
        case .replace:
            "Sent exactly as typed, in UTF-8, with its own Content-Length. At most 32 KiB."
        case .setJSON:
            "For a JSON object body. The body is rewritten compactly with its keys sorted; "
                + "one that is compressed, not JSON, or still arriving after 5 seconds is left alone."
        }
    }

    fileprivate static func redirectTitle(_ status: UInt16) -> String {
        switch status {
        case 301: "301 Moved Permanently"
        case 302: "302 Found"
        case 307: "307 Temporary Redirect"
        case 308: "308 Permanent Redirect"
        default: "\(status)"
        }
    }
}

// MARK: - The shapes the editor is built from

/// A heading and the group of fields under it.
private struct Section<Content: View, Heading: View>: View {
    @ViewBuilder let content: Content
    @ViewBuilder let heading: Heading

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            heading
            content
        }
    }
}

/// The heading above one group.
private struct Heading: View {
    let title: String
    let note: String?

    init(_ title: String, note: String?) {
        self.title = title
        self.note = note
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
            if let note {
                Text(note)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// One labelled control, what it means underneath, and what is wrong with it.
private struct Field<Content: View>: View {
    let label: String
    let error: String?
    let note: String?
    @ViewBuilder let content: Content

    init(
        _ label: String,
        error: String?,
        note: String? = nil,
        @ViewBuilder content: () -> Content
    ) {
        self.label = label
        self.error = error
        self.note = note
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)

            content

            // The refusal, where it happened. The core's own words when it is
            // the core that refused.
            if let error {
                Label(error, systemImage: "exclamationmark.circle.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let note {
                Text(note)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
