//  Finding `adb`.
//
//      $ANDROID_HOME/platform-tools/adb        if the variable is set
//      $ANDROID_SDK_ROOT/platform-tools/adb    the older spelling of it
//      ~/Library/Android/sdk/platform-tools/adb    where Android Studio puts it
//      <each $PATH entry>/adb                  for a Homebrew or hand-placed one
//
//  In that order, and the first one that is actually there wins.
//
//  # Why $PATH is last and not first
//
//  An application launched from the Finder or the Dock does not inherit the
//  `$PATH` a login shell builds: it gets a short one that almost never has the
//  Android SDK on it. So an `adb` found only by asking a shell is an `adb` the
//  app would find when run from a terminal and lose when double-clicked, which
//  is the worst of both. The SDK's own location is looked for first because it
//  is the one that does not depend on how ProxyLab was started.
//
//  Nothing here reads or writes a shell configuration file.

import Foundation

/// Resolves the `adb` this Mac has.
public enum AdbLocation {
    /// The variables that name an SDK, in the order they are consulted.
    public static let sdkVariables = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]

    /// Where Android Studio installs the SDK, under the home directory.
    static let defaultSDK = "Library/Android/sdk"

    /// Where `adb` sits inside an SDK.
    static let platformTools = "platform-tools"

    /// The executable's name.
    static let executable = "adb"

    /// The path `adb` resolves to, or `nil` when there is none.
    ///
    /// - Parameters:
    ///   - environment: the process environment, which a test may supply.
    ///   - exists: whether a path is there, which a test may answer for.
    public static func resolve(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        exists: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> String? {
        candidates(environment: environment).first(where: exists)
    }

    /// Every place that is worth looking, in order.
    ///
    /// Separated from the looking so the order is testable without a
    /// filesystem that happens to have an SDK on it.
    static func candidates(environment: [String: String]) -> [String] {
        var paths: [String] = []

        for variable in sdkVariables {
            guard let sdk = environment[variable], !sdk.isEmpty else { continue }

            paths.append(
                URL(fileURLWithPath: sdk, isDirectory: true)
                    .appendingPathComponent(platformTools, isDirectory: true)
                    .appendingPathComponent(executable)
                    .path
            )
        }

        if let home = environment["HOME"], !home.isEmpty {
            paths.append(
                URL(fileURLWithPath: home, isDirectory: true)
                    .appendingPathComponent(defaultSDK, isDirectory: true)
                    .appendingPathComponent(platformTools, isDirectory: true)
                    .appendingPathComponent(executable)
                    .path
            )
        }

        for directory in (environment["PATH"] ?? "").split(separator: ":") where !directory.isEmpty {
            paths.append(
                URL(fileURLWithPath: String(directory), isDirectory: true)
                    .appendingPathComponent(executable)
                    .path
            )
        }

        return paths
    }
}
