// swift-tools-version: 6.0

// ProxyLab's Android Emulator tooling.
//
// A package of its own, beside the iOS one and the macOS client, for the
// reason `ARCHITECTURE.md` gives device tools their own directory: pointing a
// client at the proxy and handing it the CA is not the proxy, and it is not
// the interface either.
//
// It deliberately does **not** depend on `SimulatorTools`, although the two
// have a process runner of much the same shape. Android tooling that imported
// iOS tooling would make one platform's device support a reason to rebuild the
// other's, and the shared part is ninety lines of `Process` plumbing. One
// directional rule — neither device package depends on the other — is worth
// more than that.
//
// No third-party dependencies. Foundation runs `adb`, and the tests are the
// toolchain's own Swift Testing.

import PackageDescription

let package = Package(
    name: "AndroidTools",
    // The same floor the macOS client has, for the same reasons.
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AndroidTools", targets: ["AndroidTools"]),
    ],
    targets: [
        .target(name: "AndroidTools"),
        .testTarget(
            name: "AndroidToolsTests",
            dependencies: ["AndroidTools"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
