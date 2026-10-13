//  Whether the core is there, said plainly.
//
//  The one thing a developer needs to know before anything else on screen
//  means anything: an empty traffic list is a very different thing when the
//  core is running than when it is not.

import SwiftUI

/// A dot and a word for the connection to the core.
struct ConnectionIndicator: View {
    let state: ConnectionState

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
        }
        .help(help)
        .accessibilityElement(children: .combine)
    }

    private var label: String {
        switch state {
        case .idle: "Starting"
        case .connecting: "Connecting"
        case .connected: "Connected"
        case .disconnected: "Disconnected"
        }
    }

    private var help: String {
        switch state {
        case let .disconnected(reason?): reason
        case .disconnected: "Not connected to the ProxyLab core."
        case .connected: "Connected to the ProxyLab core."
        case .connecting: "Opening a connection to the ProxyLab core."
        case .idle: "Not started yet."
        }
    }

    private var tint: Color {
        switch state {
        case .connected: StatusTone.ready.color
        case .connecting, .idle: StatusTone.pending.color
        case .disconnected: StatusTone.fault.color
        }
    }
}
