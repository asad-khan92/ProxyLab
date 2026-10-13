//  The window's Start / Stop control.
//
//      Starting…          Listening  [ Stop ]
//      Stopped  [ Start ]  Needs Approval
//      Error
//
//  The word is the capture lifecycle's phase. This Mac reads that same word.

import SwiftUI

/// Start and Stop for app-owned capture, in the window toolbar.
struct CaptureLifecycleControl: View {
    let lifecycle: CaptureLifecycle

    var body: some View {
        let presentation = lifecycle.presentation

        HStack(spacing: 8) {
            Image(systemName: presentation.symbol)
                .foregroundStyle(presentation.tint.color)
                .font(.system(size: 12, weight: .semibold))
            Text(Self.displayedStatus(lifecycle: lifecycle))
                .font(.system(size: 12, weight: .medium))

            if let action = presentation.action {
                Button {
                    Task { await lifecycle.perform(action) }
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: action.symbol)
                        Text(action.title)
                    }
                    .font(.system(size: 12, weight: .medium))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .accessibilityLabel(action.title)
            }
        }
        .help(presentation.help)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Capture, \(Self.displayedStatus(lifecycle: lifecycle))")
    }

    /// The status the toolbar shows. This Mac uses the same function.
    static func displayedStatus(lifecycle: CaptureLifecycle) -> String {
        CaptureSurfaces.title(lifecycle)
    }
}
