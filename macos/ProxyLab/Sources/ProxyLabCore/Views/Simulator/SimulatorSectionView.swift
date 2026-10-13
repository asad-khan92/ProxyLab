//  A booted simulator's setup, opened from its row.
//
//  Connect trusts the CA. The route is this Mac's proxy, which Start and Stop
//  own, so this view only says which path is in use: Explicit Proxy,
//  Transparent Capture, or Not Routed.

import SwiftUI

/// The iOS Simulator detail.
struct SimulatorSectionView: View {
    let model: SimulatorSessionModel
    let coreConnected: Bool
    var transparentCaptureListening: Bool = false

    private var presented: CaptureSourcePresentation {
        CaptureSourcePresentation.simulator(
            model: model,
            transparentCaptureListening: transparentCaptureListening,
            coreConnected: coreConnected
        )
    }

    var body: some View {
        CaptureSourceDetailView(
            presentation: presented,
            refreshDisabled: model.isWorking,
            onRefresh: { Task { await model.refresh() } },
            perform: perform
        ) {
            if presented.showsSimulatorPicker {
                picker
            }
        }
        .task { await model.refresh() }
    }

    private var picker: some View {
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

    private var selection: Binding<String?> {
        Binding(
            get: { model.selected },
            set: { model.selected = $0 }
        )
    }

    private func perform(_ control: CaptureSourceControl) {
        switch control {
        case .connect:
            Task {
                await model.connect(
                    coreIsRunning: coreConnected,
                    transparentCaptureListening: transparentCaptureListening
                )
            }
        case .disconnect:
            model.disconnect()
        case .sendCertificate, .enableLocalNetwork, .disableLocalNetwork, .certificateInstructions:
            break
        }
    }
}
