//  One booted iOS Simulator.
//
//  A name, a UDID, and the runtime it is running — which is the whole of what
//  ProxyLab needs to know about a simulator. The UDID is what every `simctl`
//  command is addressed to; the name and the runtime are what a person picks
//  from a menu, and two simulators of the same name on two runtimes are a
//  normal thing to have booted at once.

import Foundation

/// A simulator that is booted right now.
public struct Simulator: Identifiable, Hashable, Sendable {
    /// What `simctl` is addressed by.
    public let udid: String
    /// What the developer named it, which is usually the device type.
    public let name: String
    /// The runtime it booted, written the way Xcode writes it: `iOS 26.1`.
    public let runtime: String

    public var id: String { udid }

    public init(udid: String, name: String, runtime: String) {
        self.udid = udid
        self.name = name
        self.runtime = runtime
    }

    /// What a picker shows.
    ///
    /// The runtime is part of it because the name alone is ambiguous: an
    /// `iPhone 17 Pro` on iOS 18.5 and one on iOS 26.1 are two devices with
    /// one name, and picking the wrong one means trusting a certificate in a
    /// simulator nobody is looking at.
    public var label: String { "\(name) — \(runtime)" }
}
