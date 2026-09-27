//  How slow the proxy is pretending its link is.
//
//      Throttling                            ⏻
//      Latency          [  200 ] ms
//      Download         [  128 ] KB/s
//      Upload           [   32 ] KB/s
//                                      [ Apply ]
//
//  Three numbers and a switch, which is all the core has. Nothing here is a
//  preset, a graph, or a schedule: a developer who wants a different shape
//  types different numbers.
//
//  A blank field, or `0`, is *no limit* rather than *no bytes* — the same
//  thing zero means on the wire. Applying sends all three together, because
//  the core takes them together.

import SwiftUI

/// The throttling controls, under the breakpoints list.
struct ThrottleControls: View {
    let model: BreakpointsModel

    @State private var latency = ""
    @State private var download = ""
    @State private var upload = ""
    /// The settings these fields were filled in from, so a reload that
    /// changes nothing does not wipe what is being typed.
    @State private var loaded: ThrottleSettings?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Throttling")
                    .font(.system(size: 12, weight: .medium))

                Spacer()

                Toggle(
                    "Throttling",
                    isOn: Binding(
                        get: { model.throttle.enabled },
                        set: { enabled in Task { await model.setThrottleEnabled(enabled) } }
                    )
                )
                .toggleStyle(.switch)
                .controlSize(.mini)
                .labelsHidden()
                .help("Apply the numbers below to every exchange from the next one on")
            }

            row("Latency", unit: "ms", text: $latency)
            row("Download", unit: "KB/s", text: $download)
            row("Upload", unit: "KB/s", text: $upload)

            HStack {
                Text("Blank or 0 is no limit")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)

                Spacer()

                Button("Apply") {
                    Task { await model.applyThrottle(edited) }
                }
                .controlSize(.small)
                .disabled(edited == model.throttle)
            }
        }
        .padding(12)
        .onAppear { fill(from: model.throttle) }
        .onChange(of: model.throttle) { _, settings in fill(from: settings) }
    }

    private func row(_ title: String, unit: String, text: Binding<String>) -> some View {
        HStack(spacing: 8) {
            Text(title)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .frame(width: 70, alignment: .leading)

            TextField("0", text: text)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 11, design: .monospaced))
                .multilineTextAlignment(.trailing)
                .frame(width: 80)

            Text(unit)
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)

            Spacer()
        }
    }

    /// The settings the fields describe.
    ///
    /// The bandwidths are typed in kilobytes per second, because that is the
    /// unit a person thinks about a link in, and sent in bytes, because that
    /// is the unit the core keeps.
    private var edited: ThrottleSettings {
        ThrottleSettings(
            enabled: model.throttle.enabled,
            latencyMs: UInt64(latency.trimmingCharacters(in: .whitespaces)) ?? 0,
            downloadBytesPerSecond: bytes(download),
            uploadBytesPerSecond: bytes(upload)
        )
    }

    private func bytes(_ text: String) -> UInt64 {
        (UInt64(text.trimmingCharacters(in: .whitespaces)) ?? 0) * 1024
    }

    /// Fills the fields in from what the core holds.
    ///
    /// Only when they came from different settings, so a redraw does not take
    /// a half-typed number away.
    private func fill(from settings: ThrottleSettings) {
        guard loaded != settings else { return }

        loaded = settings
        latency = settings.latencyMs == 0 ? "" : String(settings.latencyMs)
        download = kilobytes(settings.downloadBytesPerSecond)
        upload = kilobytes(settings.uploadBytesPerSecond)
    }

    private func kilobytes(_ bytesPerSecond: UInt64) -> String {
        bytesPerSecond == 0 ? "" : String(bytesPerSecond / 1024)
    }
}
