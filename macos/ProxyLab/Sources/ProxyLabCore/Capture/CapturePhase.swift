//  What ProxyLab's capture is doing, as one value.
//
//  The window toolbar and This Mac both read this. A second account of
//  "are we capturing?" would be a way for the two to disagree, so there is
//  not one: both ask the capture lifecycle, and the lifecycle's phase is
//  this.

import Foundation

/// Where app-owned capture stands.
public enum CapturePhase: Equatable, Sendable {
    /// Capture is off. The owned core is not running.
    case stopped
    /// The core is launching, or transparent capture is connecting.
    case starting
    /// The core is ready and transparent capture is connected.
    case listening
    /// Transparent capture is being turned off, then the owned core.
    case stopping
    /// macOS is waiting for the user to allow the network extension.
    case needsApproval
    /// macOS is waiting for the user to trust this session's CA.
    ///
    /// This is a person deciding, not capture still starting. Stop cancels it.
    case needsTrust
    /// Capture is not listening. `message` says what to do about it.
    case failed(String)

    /// The words and control both surfaces show for this phase.
    public var presentation: CaptureControlPresentation {
        switch self {
        case .stopped:
            CaptureControlPresentation(
                title: "Stopped",
                action: .start,
                symbol: "circle",
                tint: .idle,
                help: "Start capture. ProxyLab launches its proxy and then captures this Mac's HTTP and HTTPS."
            )
        case .starting:
            CaptureControlPresentation(
                title: "Starting…",
                action: nil,
                symbol: "hourglass",
                tint: .pending,
                help: "Capture is starting. The system proxy is set after the proxy is ready, and then transparent capture turns on."
            )
        case .listening:
            CaptureControlPresentation(
                title: "Listening",
                action: .stop,
                symbol: "antenna.radiowaves.left.and.right",
                tint: .active,
                help: "ProxyLab is capturing this Mac's HTTP and HTTPS. Stop restores the previous HTTP and HTTPS proxy and then stops the proxy."
            )
        case .stopping:
            CaptureControlPresentation(
                title: "Stopping…",
                action: nil,
                symbol: "hourglass",
                tint: .pending,
                help: "Capture is stopping. The previous HTTP and HTTPS proxy is restored before the proxy engine stops."
            )
        case .needsApproval:
            CaptureControlPresentation(
                title: "Needs Approval",
                action: .stop,
                symbol: "exclamationmark.shield",
                tint: .pending,
                help: "Allow ProxyLab in System Settings › General › Login Items & Extensions › Network Extensions. Capture continues once it is allowed. Stop cancels this startup."
            )
        case .needsTrust:
            CaptureControlPresentation(
                title: "Needs CA Trust",
                action: .stop,
                symbol: "exclamationmark.shield",
                tint: .pending,
                help: "Trust ProxyLab's certificate to capture HTTPS. The system proxy is not changed until that trust is confirmed. Stop cancels this startup."
            )
        case let .failed(message):
            CaptureControlPresentation(
                title: "Error",
                action: .start,
                symbol: "exclamationmark.triangle",
                tint: .fault,
                help: message
            )
        }
    }

    /// The sentence under an error, when this phase has one.
    public var detail: String? {
        if case let .failed(message) = self { return message }
        return nil
    }
}

/// Start or Stop, the only two capture actions.
public enum CaptureControlAction: Equatable, Sendable {
    case start
    case stop

    public var title: String {
        switch self {
        case .start: "Start"
        case .stop: "Stop"
        }
    }

    /// The symbol on the button. The status symbol is the phase's own.
    public var symbol: String {
        switch self {
        case .start: "play.fill"
        case .stop: "stop.fill"
        }
    }
}

/// How loud the status marker is.
public enum CaptureTint: Equatable, Sendable {
    case idle
    case active
    case pending
    case fault
}

/// What the toolbar draws for one phase: the word, the button, and the marker.
public struct CaptureControlPresentation: Equatable, Sendable {
    public var title: String
    public var action: CaptureControlAction?
    public var symbol: String
    public var tint: CaptureTint
    public var help: String

    public init(
        title: String,
        action: CaptureControlAction?,
        symbol: String,
        tint: CaptureTint,
        help: String
    ) {
        self.title = title
        self.action = action
        self.symbol = symbol
        self.tint = tint
        self.help = help
    }
}
