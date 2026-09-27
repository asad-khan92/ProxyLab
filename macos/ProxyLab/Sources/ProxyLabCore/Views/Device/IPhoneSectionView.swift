//  The smallest useful thing: put the proxy on the Wi-Fi, read the address
//  off the screen, type it into the phone.
//
//      ┌ IPHONE                     ↻ ┐
//      │ ● Off — only this Mac can …  │
//      │ [ en0 — 192.168.1.42       ▾]│
//      │ [ Turn On ]      [ Setup… ]  │
//      └──────────────────────────────┘
//
//  Under the iOS Simulator section, in the sidebar's foot, because it is the
//  same kind of thing: a device this window can be pointed at, which is not a
//  host to filter the list by.
//
//  What is deliberately small is the panel; what is deliberately not small is
//  the warning. A LAN listener is an open proxy for everything on the Wi-Fi,
//  and the moment it is on, the panel says so in a sentence rather than with a
//  coloured dot.

import SwiftUI

/// The physical iPhone section.
struct IPhoneSectionView: View {
    let model: IPhoneSetupModel

    /// Whether the window's connection to the core is up. With no core there
    /// is nothing to ask and nothing to switch.
    let coreConnected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            status
            picker
            buttons

            if let warning = model.exposureWarning {
                Text(warning)
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityLabel(warning)
            } else if let error = model.lastError {
                Text(error)
                    .font(.system(size: 10))
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .accessibilityLabel(error)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .task { await model.refresh() }
        .sheet(isPresented: instructionsBinding) {
            IPhoneSetupSheet(model: model)
        }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Text("IPHONE")
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
            .disabled(model.isWorking || !coreConnected)
            .help("Look again for this Mac's local network addresses")
            .accessibilityLabel("Refresh local network addresses")
        }
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
                .textSelection(.enabled)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Local network: \(model.statusText)")
    }

    @ViewBuilder
    private var picker: some View {
        if model.addresses.isEmpty {
            Text("No local network address")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        } else {
            // Shown even while it is on, so what the proxy is bound to is
            // readable without turning it off to find out.
            Picker("Address", selection: selection) {
                ForEach(model.addresses) { address in
                    Text(address.label).tag(String?.some(address.address))
                }
            }
            .labelsHidden()
            .controlSize(.small)
            .disabled(model.isWorking || model.isOn)
            .help("Which of this Mac's addresses the proxy listens on")
        }
    }

    /// Which address the picker has, which is the model's and is set through
    /// it — the same shape as the simulator picker and the traffic list's
    /// selection, and for the same reason.
    private var selection: Binding<String?> {
        Binding(
            get: { model.selected },
            set: { model.selected = $0 }
        )
    }

    /// Whether the instructions sheet is up.
    private var instructionsBinding: Binding<Bool> {
        Binding(
            get: { model.showingInstructions },
            set: { model.showingInstructions = $0 }
        )
    }

    private var buttons: some View {
        HStack(spacing: 6) {
            if model.isOn {
                Button("Turn Off") {
                    Task { await model.disable() }
                }
                .controlSize(.small)
                .disabled(!model.canDisable || !coreConnected)
                .help("Stop listening on the local network")
            } else {
                Button("Turn On") {
                    Task { await model.enable() }
                }
                .controlSize(.small)
                .disabled(!model.canEnable || !coreConnected)
                .help("Let devices on this Wi-Fi reach the proxy at the selected address")
            }

            Button("Setup…") {
                model.showingInstructions = true
            }
            .controlSize(.small)
            .help("How to point an iPhone at this proxy and trust its certificate")
        }
    }

    private var tint: Color {
        switch model.state {
        case .on: .orange
        case .failed: .red
        case .loading, .working, .off: .secondary
        }
    }
}
