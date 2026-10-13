//  This Mac's detail: the capture lifecycle, and nothing that starts it.
//
//  The toolbar's Start and Stop are the only capture switch. This view reads
//  that same lifecycle and says whether the proxy, transparent capture, and
//  the certificate are ready.

import SwiftUI

/// This Mac, as a detail rather than a second capture control.
struct TransparentProxySectionView: View {
    let model: TransparentProxyModel

    /// Whether the window's connection to the core is up.
    let coreConnected: Bool

    /// The app-owned capture lifecycle, when the window has one.
    ///
    /// Nil only for a test that draws the extension on its own. The window
    /// always has one, and the row then says what that lifecycle says.
    var lifecycle: CaptureLifecycle? = nil

    /// LAN, when the detail should mention it. Nil in a test that draws this
    /// panel on its own.
    var lan: IPhoneSetupModel? = nil

    private var presented: CaptureSourcePresentation {
        if let lifecycle {
            return CaptureSourcePresentation.thisMac(
                lifecycle: lifecycle,
                lan: lan,
                coreConnected: coreConnected
            )
        }
        return Self.withoutLifecycle(model: model, coreConnected: coreConnected)
    }

    var body: some View {
        CaptureSourceDetailView(
            presentation: presented,
            refreshDisabled: !model.canRefresh,
            onRefresh: { Task { await model.refresh() } },
            perform: { _ in }
        ) {
            EmptyView()
        }
        .task { await model.refresh() }
    }

    /// The status the row shows. The toolbar's word for Listening and Stopped
    /// is this same lifecycle; a trust step uses the shorter row wording.
    static func displayedStatus(lifecycle: CaptureLifecycle?, model: TransparentProxyModel) -> String {
        if let lifecycle {
            return CaptureSourcePresentation.thisMac(
                lifecycle: lifecycle,
                lan: nil,
                coreConnected: true
            ).state.subtitle
        }
        return model.statusText
    }

    /// A drawing of the extension with no capture lifecycle beside it.
    private static func withoutLifecycle(
        model: TransparentProxyModel,
        coreConnected: Bool
    ) -> CaptureSourcePresentation {
        let awaiting = model.snapshot?.extensionState == .awaitingApproval
        let input = CaptureSourcePresentation.ThisMacInput(
            phase: model.isOn ? .listening : .stopped,
            trustActivity: nil,
            statusDetail: model.lastError,
            proxyNotice: nil,
            coreRunning: coreConnected,
            transparentOn: model.isOn,
            transparentAvailable: model.readiness == .ready,
            awaitingApproval: awaiting,
            passingThrough: model.isPassingThrough(coreConnected: coreConnected),
            lanEndpoint: nil,
            lanWaiting: false
        )
        var presentation = CaptureSourcePresentation.thisMac(input)
        if let notice = model.notice, presentation.message == nil {
            presentation.message = notice
        } else if model.readiness != .ready, let reason = model.readiness.message {
            presentation.message = reason
            presentation.technicalDetail = nil
        }
        return presentation
    }
}
