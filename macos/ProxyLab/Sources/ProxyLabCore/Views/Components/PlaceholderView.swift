//  What a pane shows when it has nothing to show.
//
//  Every empty pane in this app says which of the three things it is: waiting,
//  empty, or broken. A pane that is simply blank leaves a developer guessing
//  whether the proxy is working.

import SwiftUI

/// A centred symbol, a title, and an optional explanation.
struct PlaceholderView: View {
    let symbol: String
    let title: String
    var detail: String?
    var action: (label: String, perform: () -> Void)?

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: symbol)
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(.tertiary)
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
            if let detail {
                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
                    .textSelection(.enabled)
            }
            if let action {
                Button(action.label, action: action.perform)
                    .controlSize(.small)
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(24)
    }
}

/// A spinner with a word beside it.
struct LoadingView: View {
    let title: String

    var body: some View {
        VStack(spacing: 10) {
            ProgressView()
                .controlSize(.small)
            Text(title)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
