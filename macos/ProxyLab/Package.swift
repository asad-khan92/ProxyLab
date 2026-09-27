// swift-tools-version: 6.0

// ProxyLab's macOS client.
//
// A Swift package rather than an Xcode project: `swift build` and `swift test`
// are the whole of the build, they run in a terminal and in CI with no
// application harness, and Xcode opens `Package.swift` directly for anyone who
// wants an editor. Nothing here is generated, so there is no project file to
// keep in step with the sources.
//
// Two targets, for one reason: a test target cannot reliably `@testable
// import` an executable that carries `@main`, so everything except the entry
// point lives in `ProxyLabCore`, which the tests import. `ProxyLab` is the
// entry point and nothing else.
//
// No third-party dependencies. The IPC client is Foundation and Dispatch, the
// interface is SwiftUI, and the tests are the toolchain's own Swift Testing.
//
// Two local dependencies, `device-tools/ios/SimulatorTools` and
// `device-tools/android/AndroidTools`, which is where `ARCHITECTURE.md` puts
// tooling that configures a client to route through the proxy and trust its
// CA. They are packages rather than directories of sources because SwiftPM
// will not let a target's sources live outside its package root, and a package
// is what lets that tooling be built and tested with no window and no core.
// Neither of them depends on the other.
//
// Transparent capture adds two more, and one of them is not part of the app:
//
// - `ProxyLabTransparentProxy` is the Network Extension — a transparent
//   proxy provider that macOS runs as a system extension, in a process of its
//   own. SwiftPM builds it as an executable; `Scripts/make-app-bundle.sh`
//   wraps it in the `.systemextension` bundle macOS expects and embeds that in
//   `ProxyLab.app`. It depends on nothing of the app's.
// - `TransparentProxyKit` is what the provider and the app have to agree on:
//   the handshake with the core's bridge, which flows are handed over, the
//   configuration the app gives the provider, and the status it reports back.
//   Foundation only, so it is tested without an extension, a signature, or a
//   system that would run one.

import PackageDescription

let package = Package(
    name: "ProxyLab",
    // Observation, `NavigationSplitView`, and Swift Concurrency's
    // `AsyncStream` are all macOS 14 or older.
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "ProxyLab", targets: ["ProxyLab"]),
        .executable(name: "ProxyLabTransparentProxy", targets: ["ProxyLabTransparentProxy"]),
        .library(name: "ProxyLabCore", targets: ["ProxyLabCore"]),
    ],
    dependencies: [
        .package(path: "../../device-tools/ios/SimulatorTools"),
        .package(path: "../../device-tools/android/AndroidTools"),
    ],
    targets: [
        .executableTarget(
            name: "ProxyLab",
            dependencies: ["ProxyLabCore"]
        ),
        .target(
            name: "ProxyLabCore",
            dependencies: [
                "TransparentProxyKit",
                .product(name: "SimulatorTools", package: "SimulatorTools"),
                .product(name: "AndroidTools", package: "AndroidTools"),
            ]
        ),
        .target(name: "TransparentProxyKit"),
        .executableTarget(
            name: "ProxyLabTransparentProxy",
            dependencies: ["TransparentProxyKit"]
        ),
        .testTarget(
            name: "ProxyLabCoreTests",
            dependencies: ["ProxyLabCore"]
        ),
        .testTarget(
            name: "TransparentProxyKitTests",
            dependencies: ["TransparentProxyKit"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
