//  What to do on the phone, in order, with the numbers already filled in.
//
//      ┌──────────────────────────────────────────┐
//      │ Set up an iPhone                         │
//      │ Server 192.168.1.42   Port 9090          │
//      │                                          │
//      │ 1. Join the iPhone to the same Wi-Fi…    │
//      │ 2. Settings › Wi-Fi › ⓘ › HTTP Proxy …   │
//      │ …                                        │
//      │ ⚠ While this is on, any device …         │
//      │                                  [ Done ]│
//      └──────────────────────────────────────────┘
//
//  A sheet rather than more of the sidebar panel: the steps are five sentences
//  and the sidebar is two hundred points wide. The address and the port are at
//  the top and selectable, because they are the two things that get typed into
//  another device by hand.
//
//  Nothing here does anything. There is no button that configures the phone,
//  because there is nothing on this Mac that could: the phone is a separate
//  device, and every step is one a person performs on it.

import SwiftUI

/// The iPhone setup instructions.
struct IPhoneSetupSheet: View {
    let model: IPhoneSetupModel

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            heading
            Divider()
            steps
            Divider()
            footer
        }
        .frame(width: 460)
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Set up an iPhone")
                .font(.system(size: 15, weight: .semibold))

            if let endpoint = model.status?.endpoint {
                HStack(spacing: 6) {
                    Text(endpoint)
                        .font(.system(size: 13, design: .monospaced))
                        .textSelection(.enabled)
                    Text("— the proxy's address and port")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Proxy address and port: \(endpoint)")
            } else {
                Text("Local network access is off, so there is no address to point a phone at yet.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(model.instructions.enumerated()), id: \.offset) { index, step in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(index + 1).")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .frame(width: 16, alignment: .trailing)
                    Text(step)
                        .font(.system(size: 12))
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        HStack(alignment: .top, spacing: 12) {
            Label {
                Text(
                    "While local network access is on, any device on this Wi-Fi can use this "
                        + "proxy, and a device that trusts the certificate has its HTTPS readable. "
                        + "Turn both off when you are done."
                )
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
            }

            Spacer(minLength: 8)

            Button("Done") { dismiss() }
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }
}
