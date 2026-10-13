//  A running emulator's setup, opened from its row.
//
//  Connect points it at ProxyLab. Sending the CA is the next step, and only
//  that step, once the proxy is set. The address the emulator uses stays in
//  the session model.

import SwiftUI

/// The Android Emulator detail.
struct AndroidSectionView: View {
    let model: AndroidSessionModel
    let coreConnected: Bool

    private var presented: CaptureSourcePresentation {
        CaptureSourcePresentation.android(model: model, coreConnected: coreConnected)
    }

    var body: some View {
        CaptureSourceDetailView(
            presentation: presented,
            refreshDisabled: model.isWorking,
            onRefresh: { Task { await model.refresh() } },
            perform: perform
        ) {
            if presented.showsAndroidPicker {
                picker
            }
        }
        .task { await model.refresh() }
    }

    private var picker: some View {
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

    private var selection: Binding<String?> {
        Binding(
            get: { model.selected },
            set: { model.selected = $0 }
        )
    }

    private func perform(_ control: CaptureSourceControl) {
        switch control {
        case .connect:
            Task { await model.connect(coreIsRunning: coreConnected) }
        case .disconnect:
            Task { await model.disconnect() }
        case .sendCertificate:
            Task { await model.deliverCertificate() }
        case .enableLocalNetwork, .disableLocalNetwork, .certificateInstructions:
            break
        }
    }
}
