//  What a claimed flow does with a connection that is not ready yet.
//
//  The provider has already told macOS it will handle the flow. From there
//  the flow is the provider's: leaving it claimed and then ending it is how
//  enabling transparent capture takes the Mac's web traffic away.
//
//  A connection that is `.waiting` has not failed. It is how Network
//  framework reports both "nothing is listening" and "the route is not up
//  yet", and the second of those is what a connection looks like in the
//  moment the provider's own network settings have just been installed.
//  Only a refusal — nothing is listening, and waiting will not make it so —
//  means the bridge is down and the flow has to be carried directly. The
//  direct connection is the one that actually leaves the Mac, so a wait
//  there is the route coming up, and ending the flow on it drops traffic
//  that was about to work.

import Foundation

/// Where an outbound connection has got to, as much of it as the decision needs.
public enum RelaySocketCondition: Equatable, Sendable {
    case ready
    /// The connection is waiting. `refused` means nothing is listening.
    case waiting(RelaySocketFailure)
    /// The connection has failed and will not become ready.
    case failed(RelaySocketFailure)
    case other
}

/// Why a connection is waiting or has failed, as the decision needs it.
public enum RelaySocketFailure: Equatable, Sendable {
    /// `ECONNREFUSED`: the port is closed.
    case refused
    case other
}

/// What the provider should do about that connection.
public enum OutboundConnectionPlan: Equatable, Sendable {
    /// Go on: send the handshake, or start copying bytes.
    case proceed
    /// This connection will not succeed.
    case giveUp
    /// It may still become ready. Leave it alone.
    case wait
}

/// The two connections a claimed flow opens, and what each state means.
public enum RelayConnectionPolicy {
    /// The connection to the core's bridge on `127.0.0.1:9091`.
    ///
    /// A refusal or a failure means the core is not there. The flow is then
    /// carried directly, and for a few seconds later flows are not claimed.
    /// Any other wait is the path settling after the provider was enabled;
    /// cancelling the bridge there, and then failing the direct connection
    /// for the same wait, is what drops every claimed flow.
    public static func bridge(_ condition: RelaySocketCondition) -> OutboundConnectionPlan {
        switch condition {
        case .ready:
            return .proceed
        case .waiting(.refused), .failed:
            return .giveUp
        case .waiting, .other:
            return .wait
        }
    }

    /// The connection straight to where the application was going.
    ///
    /// Used when the bridge is down, and when the core hands its own flow
    /// back. Waiting is not a failure: the flow stays claimed until the
    /// connection is ready or has actually failed.
    public static func direct(_ condition: RelaySocketCondition) -> OutboundConnectionPlan {
        switch condition {
        case .ready:
            return .proceed
        case .failed:
            return .giveUp
        case .waiting, .other:
            return .wait
        }
    }
}
