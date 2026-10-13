//  One set of status colors.
//
//  Listening, connected, pending, and failed use the same hues wherever they
//  appear. A word sits next to the color, so the state is not color alone.

import SwiftUI

/// How loud a status marker is.
enum StatusTone: Equatable, Sendable {
    case ready
    case pending
    case fault
    case idle

    var color: Color {
        switch self {
        case .ready: .green
        case .pending: .orange
        case .fault: .red
        case .idle: .secondary
        }
    }
}

extension CaptureTint {
    var tone: StatusTone {
        switch self {
        case .idle: .idle
        case .active: .ready
        case .pending: .pending
        case .fault: .fault
        }
    }

    var color: Color { tone.color }
}

extension CaptureSourceTone {
    var tone: StatusTone {
        switch self {
        case .ready: .ready
        case .idle: .idle
        case .attention: .pending
        case .fault: .fault
        }
    }

    var color: Color { tone.color }
}
