//  An error in a status bar.
//
//  The line is the short sentence. Details… holds the technical text, and
//  only when there is some.

import SwiftUI

/// A short error, with Details… when the full message is not that sentence.
struct StatusNotice: View {
    let message: String

    @State private var showingDetail = false

    var body: some View {
        let notice = UserFacingNotice(message)

        HStack(spacing: 4) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 10))
                .foregroundStyle(StatusTone.fault.color)
                .accessibilityHidden(true)

            Text(notice.summary)
                .font(.system(size: 11))
                .foregroundStyle(StatusTone.fault.color)
                .lineLimit(1)
                .truncationMode(.tail)
                .help(notice.detail ?? notice.summary)

            if let detail = notice.detail {
                Button("Details…") { showingDetail = true }
                    .buttonStyle(.borderless)
                    .controlSize(.mini)
                    .font(.system(size: 11))
                    .popover(isPresented: $showingDetail, arrowEdge: .top) {
                        Text(detail)
                            .font(.system(size: 11))
                            .textSelection(.enabled)
                            .frame(maxWidth: 360, alignment: .leading)
                            .padding(12)
                    }
                    .accessibilityHint("Show the technical detail")
            }
        }
        .accessibilityElement(children: .combine)
    }
}
