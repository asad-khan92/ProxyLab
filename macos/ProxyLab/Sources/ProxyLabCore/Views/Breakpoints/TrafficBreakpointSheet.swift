//  The sheet a breakpoint is written in, starting from a captured exchange.
//
//      GET staging-api.example.com/api/orders
//      The selected exchange stays as it was. The next match will pause.
//      Host    [ staging-api.example.com ]
//      Path    [ /api/orders             ] (•) exact  ( ) prefix
//      Method  [ GET                     ]
//      [✓] Pause request
//      [✓] Pause response
//                              [ Cancel ]  [ Create ]
//
//  The same matcher the Breakpoints screen writes, filled in from the row.
//  Creating it calls the same create that screen uses, once per direction.

import SwiftUI

/// The sheet opened by Breakpoint… on a traffic row.
struct TrafficBreakpointSheet: View {
    @Bindable var model: BreakpointsModel

    var body: some View {
        if let draft = model.trafficDraft {
            content(
                Binding(
                    get: { model.trafficDraft ?? draft },
                    set: { model.trafficDraft = $0 }
                )
            )
        }
    }

    private func content(_ draft: Binding<TrafficBreakpointDraft>) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(draft.wrappedValue.name)
                .font(.system(size: 13, weight: .semibold))
                .textSelection(.enabled)
            Text(TrafficBreakpointDraft.scopeNote)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Form {
                TextField("Host", text: draft.host, prompt: Text("api.example.com"))
                HStack(spacing: 8) {
                    TextField("Path", text: draft.path, prompt: Text("/orders"))
                    Picker("Path match", selection: draft.pathIsPrefix) {
                        Text("Exact").tag(false)
                        Text("Prefix").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: 130)
                }
                TextField("Method", text: draft.method, prompt: Text("GET"))
                Toggle("Pause request", isOn: draft.pauseRequest)
                Toggle("Pause response", isOn: draft.pauseResponse)
            }
            .formStyle(.grouped)

            Text(draft.wrappedValue.matcher.isAny ? everyMatch : "Holds: \(draft.wrappedValue.matcher.summary)")
                .font(.system(size: 11))
                .foregroundStyle(draft.wrappedValue.matcher.isAny ? .orange : .secondary)
            Text(draft.wrappedValue.pauseNote)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if let refusal = draft.wrappedValue.refusal {
                Text(refusal)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { model.cancelTrafficDraft() }
                Button("Create") {
                    Task { await model.createTrafficDraft() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(draft.wrappedValue.refusal != nil)
            }
        }
        .padding(18)
        .frame(width: 440)
    }

    private var everyMatch: String {
        "Every request this pause covers will be held."
    }
}
