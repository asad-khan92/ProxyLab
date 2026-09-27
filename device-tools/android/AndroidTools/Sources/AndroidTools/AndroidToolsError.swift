//  What can go wrong reaching an emulator, said in a sentence a developer can
//  act on.
//
//  `adb`'s own standard error is carried through where there is one: it is
//  Google's text about Google's tool, and it is the most useful thing there is
//  when something fails for a reason this package did not anticipate.

import Foundation

/// A failure reaching, reading, or configuring an Android Emulator.
public enum AndroidToolsError: Error, Equatable, Sendable {
    /// `adb` could not be found on this Mac.
    case adbNotFound
    /// It was found, and could not be started.
    case adbNotRunnable(String)
    /// An `adb` command ran and failed.
    case commandFailed(command: String, status: Int32, reason: String)
    /// Nothing is running, so there is nothing to connect.
    case noRunningEmulator
    /// The emulator is there but not ready to be talked to.
    case deviceUnavailable(serial: String, state: String)
    /// It was there when the list was made, and is not there now.
    case emulatorGone(serial: String)
    /// The proxy was set, and reading it back said something else.
    case proxyNotApplied(expected: String, actual: String)
    /// A saved proxy value that cannot safely be sent to the device's shell.
    case proxyNotRestorable(String)
    /// The CA certificate is not where it was expected.
    case certificateMissing(path: String)

    /// The sentence the interface shows.
    public var message: String {
        switch self {
        case .adbNotFound:
            "adb could not be found. Install the Android SDK platform-tools, or set "
                + "ANDROID_HOME to the SDK — ProxyLab also looks in "
                + "~/Library/Android/sdk/platform-tools."
        case let .adbNotRunnable(reason):
            "adb could not be run: \(reason)."
        case let .commandFailed(command, status, reason) where reason.isEmpty:
            "adb \(command) failed with status \(status)."
        case let .commandFailed(command, _, reason):
            "adb \(command) failed: \(reason)"
        case .noRunningEmulator:
            "No Android Emulator is running. Start one from Android Studio's Device Manager, "
                + "then refresh."
        case let .deviceUnavailable(serial, state):
            "\(serial) is \(state) rather than ready. Wait for it to finish booting, or "
                + "reconnect it in Android Studio, then refresh."
        case let .emulatorGone(serial):
            "\(serial) is no longer running."
        case let .proxyNotApplied(expected, actual):
            "The emulator's proxy was set to \(expected) but reads back as \(actual). "
                + "Nothing has been left connected."
        case let .proxyNotRestorable(value):
            "The proxy setting that was there before is not a value ProxyLab will send back "
                + "to the device (\(value)). It has been left as it is; set it by hand if it "
                + "is wanted."
        case let .certificateMissing(path):
            "ProxyLab's CA certificate is not at \(path). "
                + "Run the ProxyLab core once — it creates the CA the first time it starts."
        }
    }
}
