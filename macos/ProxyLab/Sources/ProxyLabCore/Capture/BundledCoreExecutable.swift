//  Where this app's own proxy engine lives.
//
//      ProxyLab.app/Contents/MacOS/proxy-engine
//
//  The same directory as the app executable. macOS treats that as code, which
//  is what a helper the app launches has to be — a resource would be data,
//  and Gatekeeper does not run data. The path is the bundle's, worked out
//  from the bundle URL, so a Dock launch and a `swift run` do not go looking
//  through a repository, Cargo's output, or `PATH`.

import Foundation

/// The proxy engine executable embedded in an application bundle.
public enum BundledCoreExecutable {
    /// The file name inside `Contents/MacOS`.
    public static let executableName = "proxy-engine"

    /// `Contents/MacOS/proxy-engine` inside `bundleURL`.
    public static func url(bundleURL: URL) -> URL {
        bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent(executableName)
    }

    /// The engine in `bundle`, which is the running app unless a test says
    /// otherwise.
    public static func url(in bundle: Bundle = .main) -> URL {
        url(bundleURL: bundle.bundleURL)
    }
}
