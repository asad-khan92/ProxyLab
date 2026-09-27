//  Finding adb, in an order that does not depend on how ProxyLab was started.

import Foundation
import Testing

@testable import AndroidTools

@Suite("Finding adb")
struct AdbLocationTests {
    @Test("the SDK variables are consulted before anything else")
    func sdkVariablesFirst() {
        let candidates = AdbLocation.candidates(environment: [
            "ANDROID_HOME": "/sdk-home",
            "ANDROID_SDK_ROOT": "/sdk-root",
            "HOME": "/home/dev",
            "PATH": "/usr/bin",
        ])

        #expect(candidates[0] == "/sdk-home/platform-tools/adb")
        #expect(candidates[1] == "/sdk-root/platform-tools/adb")
    }

    @Test("Android Studio's own location is looked for before the PATH")
    func defaultSDKBeforePath() {
        // An app launched from the Dock has almost no PATH, so the location
        // that does not depend on how it was started comes first.
        let candidates = AdbLocation.candidates(environment: [
            "HOME": "/home/dev",
            "PATH": "/opt/homebrew/bin:/usr/bin",
        ])

        #expect(candidates == [
            "/home/dev/Library/Android/sdk/platform-tools/adb",
            "/opt/homebrew/bin/adb",
            "/usr/bin/adb",
        ])
    }

    @Test("an empty variable is not a location")
    func emptyVariables() {
        let candidates = AdbLocation.candidates(environment: [
            "ANDROID_HOME": "",
            "HOME": "",
            "PATH": "",
        ])

        #expect(candidates.isEmpty)
    }

    @Test("the first one that is actually there is the one that is used")
    func firstThatExists() {
        let resolved = AdbLocation.resolve(
            environment: ["ANDROID_HOME": "/sdk", "HOME": "/home/dev"],
            exists: { $0 == "/home/dev/Library/Android/sdk/platform-tools/adb" }
        )

        #expect(resolved == "/home/dev/Library/Android/sdk/platform-tools/adb")
    }

    @Test("no adb anywhere is nothing, and the message says where to put one")
    func noAdb() {
        #expect(AdbLocation.resolve(environment: ["HOME": "/home/dev"], exists: { _ in false }) == nil)
        #expect(AndroidToolsError.adbNotFound.message.contains("ANDROID_HOME"))
        #expect(AndroidToolsError.adbNotFound.message.contains("platform-tools"))
    }
}
