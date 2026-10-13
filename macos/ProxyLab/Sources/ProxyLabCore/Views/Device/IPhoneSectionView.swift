//  A physical iPhone's setup, opened from its row.
//
//  The row names the LAN address the phone has to be given. This view is how
//  that listener is turned on, and where the certificate steps are. It is not
//  the simulator's route.

import SwiftUI

/// The physical iPhone detail.
struct IPhoneSectionView: View {
    let model: IPhoneSetupModel
    let coreConnected: Bool

    private var presented: CaptureSourcePresentation {
        CaptureSourcePresentation.iPhone(model: model, coreConnected: coreConnected)
    }

    var body: some View {
        CaptureSourceDetailView(
            presentation: presented,
            refreshDisabled: model.isWorking || !coreConnected,
            onRefresh: { Task { await model.refresh() } },
            perform: perform
        ) {
            if presented.showsAddressPicker {
                picker
            }
        }
        .task { await model.refresh() }
    }

    private var picker: some View {
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

    private var selection: Binding<String?> {
        Binding(
            get: { model.selected },
            set: { model.selected = $0 }
        )
    }

    private func perform(_ control: CaptureSourceControl) {
        switch control {
        case .enableLocalNetwork:
            Task { await model.enable() }
        case .disableLocalNetwork:
            Task { await model.disable() }
        case .certificateInstructions:
            model.showingInstructions = true
        case .connect, .disconnect, .sendCertificate:
            break
        }
    }
}
