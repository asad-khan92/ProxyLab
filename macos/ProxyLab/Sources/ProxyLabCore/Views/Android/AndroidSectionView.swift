//  Pick a running emulator, connect it, send it the CA.
//
//      ┌ ANDROID EMULATOR           ↻ ┐
//      │ [ Pixel_10_Pro_XL — emu… ▾ ] │
//      │ ● Connected — via 10.0.2.2…  │
//      │ [ Connect ] [ Disconnect ]   │
//      │ [ Send CA to Emulator ]      │
//      │ Pixel_10_Pro_XL now proxies… │
//      └──────────────────────────────┘
//
//  Beneath the iOS Simulator section, in the foot of the sidebar, and built
//  the same way for the same reasons. The third button is the one difference:
//  an emulator needs the CA copied across before HTTPS can be read, and there
//  is no adb command that installs it, so ProxyLab delivers the file and the
//  note says what to tap.

import SwiftUI

/// The Android Emulator section.
struct AndroidSectionView: View {
    let model: AndroidSessionModel

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
            Text("ANDROID EMULATOR")
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
            .help("Look again for running emulators")
            .accessibilityLabel("Refresh running emulators")
        }
    }

    @ViewBuilder
    private var picker: some View {
        if model.emulators.isEmpty {
            Text("No emulator running")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        } else {
            Picker("Emulator", selection: selection) {
                ForEach(model.emulators) { emulator in
                    Text(emulator.isUsable ? emulator.label : "\(emulator.label) (\(emulator.state))")
                        .tag(String?.some(emulator.serial))
                }
            }
            .labelsHidden()
            .controlSize(.small)
            .disabled(model.isWorking)
            .help("The running emulators ProxyLab can connect")
        }
    }

    /// Which emulator the picker has, which is the model's and is set through
    /// it.
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
        .accessibilityLabel("Emulator status: \(model.statusText)")
    }

    private var buttons: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Button("Connect") {
                    Task { await model.connect(coreIsRunning: coreConnected) }
                }
                .controlSize(.small)
                .disabled(!model.canConnect)
                .help("Point the emulator's HTTP proxy at \(model.proxyAddress)")

                Button("Disconnect") {
                    Task { await model.disconnect() }
                }
                .controlSize(.small)
                .disabled(!model.canDisconnect)
                .help("Put back the proxy setting the emulator had before")
            }

            Button("Send CA to Emulator") {
                Task { await model.deliverCertificate() }
            }
            .controlSize(.small)
            .disabled(!model.canDeliverCertificate)
            .help("Copy ProxyLab's CA to the emulator's Downloads, ready to install by hand")
        }
    }

    private var tint: Color {
        switch model.state {
        case .connected: .green
        case .connecting: .orange
        case .notConnected: .secondary
        }
    }
}
