//  Writing one breakpoint: which half, and which exchanges.
//
//      New Request Breakpoint
//      Host    [ api.example.com            ]
//      Path    [ /orders                    ] ( ) exact  (•) prefix
//      Method  [ POST                       ]
//                              [ Cancel ]  [ Add ]
//
//  The same three criteria a rule matches on, because they are the same
//  matcher — see ``RuleMatcher``. A field left blank is not a criterion: it
//  matches anything, so a breakpoint with nothing filled in stops every
//  exchange, which is a real thing to want and is said plainly under the
//  fields rather than guarded against.

import SwiftUI

/// The breakpoint being written, if one is.
struct BreakpointDraft {
    var isOpen = false
    var direction: BreakpointDirection = .request
    var host = ""
    var path = ""
    var pathIsPrefix = false
    var method = ""

    /// Opens the sheet on a new breakpoint for `direction`.
    mutating func open(_ direction: BreakpointDirection) {
        self = BreakpointDraft(isOpen: true, direction: direction)
    }

    /// The criteria the fields describe.
    var matcher: RuleMatcher {
        var matcher = RuleMatcher.any
        let host = host.trimmingCharacters(in: .whitespaces)
        let path = path.trimmingCharacters(in: .whitespaces)
        let method = method.trimmingCharacters(in: .whitespaces)

        if !host.isEmpty { matcher.host = .exact(host) }
        if !path.isEmpty { matcher.path = pathIsPrefix ? .prefix(path) : .exact(path) }
        if !method.isEmpty { matcher.method = .exact(method) }

        return matcher
    }

    /// Why the core would refuse this, or `nil` when it would not.
    ///
    /// The same checks `RuleMatcher::is_valid` makes, so the sheet can say so
    /// while it is being typed. The core checks again.
    var refusal: String? {
        let host = host.trimmingCharacters(in: .whitespaces)
        let path = path.trimmingCharacters(in: .whitespaces)

        if !host.isEmpty, host.contains("/") {
            return "A host is a host name, not a URL."
        }
        if !path.isEmpty, !path.hasPrefix("/") {
            return "A path begins with “/”."
        }

        return nil
    }
}

/// The sheet a new breakpoint is written in.
struct BreakpointEditorSheet: View {
    @Binding var draft: BreakpointDraft
    let model: BreakpointsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New \(draft.direction.title) Breakpoint")
                .font(.system(size: 13, weight: .semibold))
            Text(draft.direction.explanation)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            Form {
                TextField("Host", text: $draft.host, prompt: Text("api.example.com"))
                HStack(spacing: 8) {
                    TextField("Path", text: $draft.path, prompt: Text("/orders"))
                    Picker("", selection: $draft.pathIsPrefix) {
                        Text("Exact").tag(false)
                        Text("Prefix").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 130)
                }
                TextField("Method", text: $draft.method, prompt: Text("POST"))
            }
            .formStyle(.grouped)

            Text(
                draft.matcher.isAny
                    ? "Every \(draft.direction.title.lowercased()) will be held."
                    : "Holds: \(draft.matcher.summary)"
            )
            .font(.system(size: 11))
            .foregroundStyle(draft.matcher.isAny ? .orange : .secondary)

            if let refusal = draft.refusal {
                Text(refusal)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { draft.isOpen = false }
                Button("Add") {
                    let direction = draft.direction
                    let matcher = draft.matcher
                    draft.isOpen = false

                    Task { await model.create(direction: direction, matcher: matcher) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(draft.refusal != nil)
            }
        }
        .padding(18)
        .frame(width: 420)
    }
}
