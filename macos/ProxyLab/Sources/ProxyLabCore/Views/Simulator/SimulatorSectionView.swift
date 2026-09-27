//  The smallest useful thing: pick a booted simulator, connect it, see where
//  it stands.
//
//      ┌ iOS SIMULATOR              ↻ ┐
//      │ [ iPhone 17 Pro — iOS 26.1 ▾]│
//      │ ● Connected — via 127.0.0.1… │
//      │ [ Connect ]   [ Disconnect ] │
//      │ iPhone 17 Pro trusts ProxyL… │
//      └──────────────────────────────┘
//
//  At the foot of the sidebar rather than in the list above it: the list is a
//  selection of hosts and this is not a host, and a panel pinned under it
//  stays put while that list scrolls.
//
//  The note is the part that matters. Connecting a simulator can half-succeed
//  — the certificate is trusted, the route is not ProxyLab's — and that is a
//  sentence, not an icon, so there is room for one under the buttons.

import SwiftUI

/// The iOS Simulator section.
struct SimulatorSectionView: View {
    let model: SimulatorSessionModel

    /// Whether the window's connection to the core is up, which is what
    /// "ProxyLab is running" means here.
    let coreConnected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            picker
            status
            buttons

            if let note = model.note {
                Text(note.text)
                    .font(.system(size: 10))
                    .foregroundStyle(note.isFailure ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .accessibilityLabel(note.text)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .task { await model.refresh() }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Text("iOS SIMULATOR")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)

            Spacer(minLength: 4)

            Button {
                Task { await model.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 9))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .disabled(model.isWorking)
            .help("Look again for booted simulators")
            .accessibilityLabel("Refresh booted simulators")
        }
    }

    @ViewBuilder
    private var picker: some View {
        if model.simulators.isEmpty {
            Text("No simulator booted")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        } else {
            Picker("Simulator", selection: selection) {
                ForEach(model.simulators) { simulator in
                    Text(simulator.label).tag(String?.some(simulator.udid))
                }
            }
            .labelsHidden()
            .controlSize(.small)
            .disabled(model.isWorking)
            .help("The booted simulators ProxyLab can connect")
        }
    }

    /// Which simulator the picker has, which is the model's and is set
    /// through it — the same shape as the traffic list's selection, and for
    /// the same reason: the model is the one that holds it.
    private var selection: Binding<String?> {
        Binding(
            get: { model.selected },
            set: { model.selected = $0 }
        )
    }

    private var status: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
            Text(model.statusText)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Simulator status: \(model.statusText)")
    }

    private var buttons: some View {
        HStack(spacing: 6) {
            Button("Connect") {
                Task { await model.connect(coreIsRunning: coreConnected) }
            }
            .controlSize(.small)
            .disabled(!model.canConnect)
            .help("Trust ProxyLab's CA in the selected simulator and check what it routes through")

            Button("Disconnect") {
                model.disconnect()
            }
            .controlSize(.small)
            .disabled(!model.canDisconnect)
            .help("End this simulator's ProxyLab session")
        }
    }

    private var tint: Color {
        switch model.state {
        case .connected: .green
        case .certificateTrusted, .connecting: .orange
        case .notConnected: .secondary
        }
    }
}
