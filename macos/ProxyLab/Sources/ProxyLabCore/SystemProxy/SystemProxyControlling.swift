//  The capture lifecycle's reach into this Mac's HTTP and HTTPS proxy.
//
//  One protocol, so Start, Stop, and a network change are testable without
//  changing the machine the tests run on. The real implementation talks to
//  System Configuration. Nothing else does.

import Foundation

/// A service ProxyLab configured, or refused to overwrite, after a network change.
public struct SystemProxyAdoption: Equatable, Sendable {
    public var claimed: [String]
    public var ownershipLost: [String]

    public static let none = SystemProxyAdoption(claimed: [], ownershipLost: [])
}

/// What Stop did with the services in the ownership record.
public struct SystemProxyRestoreReport: Equatable, Sendable {
    public var restored: [String]
    public var leftInPlace: [String]
    public var vanished: [String]
    /// The proxy still points at ProxyLab, so the core must keep running.
    public var stillClaimed: Bool
    public var message: String?

    public static let none = SystemProxyRestoreReport(
        restored: [],
        leftInPlace: [],
        vanished: [],
        stillClaimed: false,
        message: nil
    )
}

/// Why a change to the system proxy did not happen.
public enum SystemProxyError: Error, Equatable, LocalizedError {
    case unreadable
    case noActiveService
    case authorizationCancelled
    case authorizationDenied
    case notApplied(String)
    /// HTTP and HTTPS already point at ProxyLab, and there is no saved baseline.
    case ownedWithoutBaseline

    public var errorDescription: String? {
        switch self {
        case .unreadable:
            "ProxyLab could not read this Mac's network services."
        case .noActiveService:
            "ProxyLab could not find an active network service, so it did not change the HTTP or HTTPS proxy."
        case .authorizationCancelled:
            "ProxyLab needs an administrator's approval to change this Mac's HTTP and HTTPS proxy. The approval was cancelled, so the proxy was not changed."
        case .authorizationDenied:
            "ProxyLab could not get approval to change this Mac's HTTP and HTTPS proxy."
        case let .notApplied(detail):
            "ProxyLab could not apply the HTTP and HTTPS proxy. \(detail)"
        case .ownedWithoutBaseline:
            "HTTP and HTTPS are already set to 127.0.0.1:9090, and ProxyLab has no saved proxy to put back. "
                + "The settings were left as they are, and the proxy engine was left running."
        }
    }
}

/// Saves, sets, and restores this Mac's HTTP and HTTPS proxy for the services
/// ProxyLab has taken.
@MainActor
public protocol SystemProxyControlling: AnyObject {
    var hasClaim: Bool { get }

    /// After the explicit listener is ready. Does nothing to the system
    /// before that call.
    func claim(port: Int) throws

    /// The active service may have changed. Configures a service that is
    /// newly primary. Does not overwrite one whose settings are no longer
    /// ProxyLab's.
    func adoptActiveService() throws -> SystemProxyAdoption

    /// Puts back every service that still has ProxyLab's proxy.
    func restore() throws -> SystemProxyRestoreReport

    /// Whether the next claim or restore will show the administrator dialog.
    ///
    /// The default is no. The System Configuration implementation asks the
    /// rights it already holds, without showing the dialog.
    func authorizationNeedsPrompt() -> Bool
}

extension SystemProxyControlling {
    public func authorizationNeedsPrompt() -> Bool { false }
}
