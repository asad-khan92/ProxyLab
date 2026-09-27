// swift-tools-version: 6.0

// ProxyLab's iOS Simulator tooling.
//
// A package of its own, beside the macOS client rather than inside it, for the
// reason `ARCHITECTURE.md` gives device tools their own directory: configuring
// a client to route through the proxy and to trust its CA is not the proxy,
// and it is not the interface either. Nothing here imports ProxyLabCore, and
// nothing here knows the Rust core exists — it is told a port and a
// certificate path by whoever calls it.
//
// SwiftPM will not let a target's sources live outside its package root, so
// `macos/ProxyLab` depends on this by path rather than reaching across into
// it. That is also what keeps the tooling testable on its own: `swift test`
// here runs without a window, without the core, and — apart from the one
// suite that asks the real `simctl` for the booted devices — without a
// simulator.
//
// No third-party dependencies. Foundation runs `simctl`, SystemConfiguration
// reads the proxy settings, and the tests are the toolchain's own Swift
// Testing.

import PackageDescription

let package = Package(
    name: "SimulatorTools",
    // The same floor the macOS client has, for the same reasons.
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "SimulatorTools", targets: ["SimulatorTools"]),
    ],
    targets: [
        .target(name: "SimulatorTools"),
        .testTarget(
            name: "SimulatorToolsTests",
            dependencies: ["SimulatorTools"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
