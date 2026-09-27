//  This Mac's own traffic, captured with no proxy setting.
//
//      ┌ THIS MAC                    ↻ ┐
//      │ ● Off — only apps pointed …   │
//      │   Extension installed         │
//      │ [ Turn On ]                   │
//      └───────────────────────────────┘
//
//  At the foot of the sidebar, under the iPhone, because it is the same kind
//  of thing: somewhere traffic comes from, not a host to filter the list by.
//
//  Small on purpose, and loud when it matters. Capturing every app's traffic
//  breaks the apps that pin their certificates, so while it is on the panel
//  says so in a sentence — and when the core is not there to receive what is
//  captured, it says that instead, because then nothing is being captured at
//  all.

import SwiftUI

/// The Network Extension section.
struct TransparentProxySectionView: View {
    let model: TransparentProxyModel

    /// Whether the window's connection to the core is up. Capture can be
    /// switched without it — the extension is macOS's, not the core's — but
    /// what happens to the traffic then is different, and is said.
    let coreConnected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            status

            if let text = model.extensionText {
                Text(text)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .padding(.leading, 13)
            }

            if model.readiness == .ready {
                button
            }

            footnote
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .task { await model.refresh() }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Text("THIS MAC")
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
            .disabled(model.isWorking || model.readiness != .ready)
            .help("Ask macOS again where transparent capture stands")
            .accessibilityLabel("Refresh transparent capture status")
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
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Transparent capture: \(model.statusText)")
    }

    @ViewBuilder
    private var button: some View {
        if model.isOn {
            Button("Turn Off") {
                Task { await model.turnOff() }
            }
            .controlSize(.small)
            .disabled(!model.canTurnOff)
            .help("Stop capturing this Mac's traffic; apps connect directly again")
        } else {
            Button("Turn On") {
                Task { await model.turnOn() }
            }
            .controlSize(.small)
            .disabled(!model.canTurnOn)
            .help("Capture this Mac's HTTP and HTTPS through ProxyLab, with no proxy setting")
        }
    }

    /// One sentence, the most important one there is: why it cannot be used,
    /// what went wrong, what to do next, or what being on means.
    @ViewBuilder
    private var footnote: some View {
        if let reason = model.readiness.message {
            sentence(reason, color: .secondary)
        } else if let error = model.lastError {
            sentence(error, color: .red)
        } else if let notice = model.notice {
            sentence(notice, color: .primary)
        } else if let warning = model.warning(coreConnected: coreConnected) {
            sentence(warning, color: .orange)
        }
    }

    private func sentence(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.system(size: 10))
            .foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .accessibilityLabel(text)
    }

    private var tint: Color {
        if model.lastError != nil { return .red }
        guard model.isOn else { return .secondary }

        return model.isPassingThrough(coreConnected: coreConnected) ? .red : .orange
    }
}
