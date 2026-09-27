//  The small coloured pieces a traffic row is built from.
//
//  Colour carries one meaning here and it is always the same one: how the
//  exchange ended. A method is never coloured by what it is, because a `POST`
//  is not a warning, and a row where everything is coloured is a row where
//  nothing stands out.

import SwiftUI

/// The HTTP method, in a fixed-width slot so the rows line up.
struct MethodLabel: View {
    let method: String

    var body: some View {
        Text(method)
            .font(.system(size: 10, weight: .semibold, design: .monospaced))
            .foregroundStyle(.secondary)
            .frame(width: 46, alignment: .leading)
            .accessibilityLabel("Method \(method)")
    }
}

/// How an exchange ended, as a status code or as a reason there is none.
struct OutcomeBadge: View {
    let outcome: TrafficDisplay.Outcome

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold, design: .monospaced))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(tint.opacity(0.16), in: RoundedRectangle(cornerRadius: 4))
            .foregroundStyle(tint)
            .accessibilityLabel(accessibilityText)
    }

    private var text: String {
        switch outcome {
        case let .status(status): String(status)
        case .failed: "ERR"
        case .pending: "···"
        }
    }

    private var accessibilityText: String {
        switch outcome {
        case let .status(status): "Status \(status)"
        case .failed: "Failed"
        case .pending: "No response yet"
        }
    }

    private var tint: Color {
        switch outcome {
        case let .status(status):
            switch status {
            case 200..<300: .green
            case 300..<400: .teal
            case 400..<500: .orange
            case 500..<600: .red
            default: .secondary
            }
        case .failed: .red
        case .pending: .secondary
        }
    }
}

/// A lock for an intercepted TLS session, nothing for plaintext.
struct SchemeMark: View {
    let scheme: Scheme

    var body: some View {
        Image(systemName: scheme == .https ? "lock.fill" : "lock.open")
            .font(.system(size: 9))
            .foregroundStyle(scheme == .https ? Color.secondary : Color.secondary.opacity(0.45))
            .accessibilityLabel(scheme == .https ? "HTTPS" : "HTTP")
    }
}
