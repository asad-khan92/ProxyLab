//  Where ProxyLab stands, and what recently went wrong.
//
//  A sheet, not a fourth screen. Refresh asks the models the window already
//  has. Copy puts the same lines on the clipboard, with secrets and personal
//  paths already removed.

import SwiftUI

/// The diagnostics sheet.
public struct DiagnosticsView: View {
    private let model: DiagnosticsModel

    public init(model: DiagnosticsModel) {
        self.model = model
    }

    public var body: some View {
        let snapshot = model.snapshot

        VStack(spacing: 0) {
            List {
                ForEach(snapshot.sections) { section in
                    Section(section.title) {
                        ForEach(section.lines) { line in
                            LabeledContent(line.title) {
                                Text(line.value)
                                    .foregroundStyle(line.isProblem ? Color.red : Color.primary)
                                    .textSelection(.enabled)
                                    .multilineTextAlignment(.trailing)
                            }
                        }
                    }
                }
            }

            HStack {
                Button("Refresh") {
                    Task { await model.refresh(includingDevices: true) }
                }
                .disabled(model.isRefreshing)
                .help("Ask the core, the device tools, and the Network Extension again")

                Spacer()

                Button("Copy Report") {
                    model.copyReport()
                }
                .help("Copy a diagnostics report with secrets and personal paths removed")
            }
            .padding(12)
        }
        .frame(minWidth: 520, minHeight: 480)
        .task { await model.refresh(includingDevices: false) }
    }
}
