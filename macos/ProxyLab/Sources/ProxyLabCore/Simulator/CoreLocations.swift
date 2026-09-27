//  Where the core listens, and where it keeps its CA certificate.
//
//      proxy      127.0.0.1:9090                DEFAULT_LISTEN_ADDR
//      bridge     127.0.0.1:9091                DEFAULT_TRANSPARENT_ADDR
//      CA         $PROXYLAB_CA_DIR/ca-cert.pem  if the variable is set
//                 ~/.proxylab/ca/ca-cert.pem    otherwise
//
//  The same thing `IPCSocketLocation` is for the socket, for the two other
//  places the core puts itself. Every rule here is the Rust side's rule,
//  copied deliberately and in one place:
//
//      `DEFAULT_LISTEN_ADDR`                   core/proxy-engine/src/listener.rs
//      `DEFAULT_TRANSPARENT_ADDR`              core/proxy-engine/src/transparent/mod.rs
//      `CertificateAuthority::default_directory`   core/tls/src/authority.rs
//      `store::CERTIFICATE_FILE`               core/tls/src/store.rs
//
//  A second configuration source would be a way for the app and the core to
//  disagree about where the proxy is, so there is not one: nothing else in the
//  app spells a port or a certificate path, and there is no preference for
//  either. When the core learns to listen somewhere else, this is the file
//  that follows it.
//
//  # Why the port is mirrored rather than asked for
//
//  `lan_status` now exists and its `port` is the port the proxy is actually
//  listening on — that is the better source, and `IPhoneSetupModel` uses it.
//  The simulator section has not moved to it, and the number is the same
//  either way: `DEFAULT_LISTEN_ADDR` is the only thing that sets the port, so
//  what the core would report is the constant below. Moving this to the core's
//  answer means giving the simulator section a core connection and a state for
//  "the core has not said yet", which is a change to that feature rather than
//  to this constant, and is the change to make when the port stops being
//  fixed.
//
//  Nothing here is written. The CA certificate is read by `simctl`, which is
//  handed the path; the port is a number put into a comparison and into a
//  sentence.

import Foundation
import SimulatorTools

/// The core's own locations, as the core resolves them.
public enum CoreLocations {
    /// Where the proxy engine listens.
    ///
    /// Fixed, because `DEFAULT_LISTEN_ADDR` is fixed: the core reads no
    /// variable and takes no argument for it, so neither does this.
    public static let proxyEndpoint = ProxyEndpoint(host: "127.0.0.1", port: proxyPort)

    /// The port of that endpoint, on its own.
    ///
    /// The same number, reachable without the `SimulatorTools` type that
    /// carries it — an Android emulator needs the port and has no business
    /// importing the iOS tooling to read it.
    public static let proxyPort = 9_090

    /// Where the core accepts flows from the transparent proxy provider.
    ///
    /// The app never connects to it. It is handed to the provider in the
    /// configuration the app saves, because the provider runs in a sandbox
    /// of its own and has no other way to learn it.
    public static let transparentBridgePort = 9_091

    /// The variable that names a CA directory other than the default.
    public static let caDirectoryVariable = "PROXYLAB_CA_DIR"

    /// ProxyLab's directory under the home directory.
    static let stateDirectory = ".proxylab"

    /// The CA's directory, under that.
    static let caDirectory = "ca"

    /// The certificate's name inside it. The private key beside it is never
    /// named here, because nothing in this application has any business with
    /// it.
    static let certificateFile = "ca-cert.pem"

    /// The CA certificate the core generated, or `nil` when there is no home
    /// directory to look under and no variable naming one.
    ///
    /// - Parameter environment: the process environment, which a test may
    ///   supply instead.
    public static func caCertificate(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String? {
        let directory: String

        if let configured = environment[caDirectoryVariable], !configured.isEmpty {
            directory = configured
        } else {
            let home = [environment["HOME"], environment["USERPROFILE"]]
                .compactMap { $0 }
                .first { !$0.isEmpty }

            guard let home else { return nil }

            directory = URL(fileURLWithPath: home, isDirectory: true)
                .appendingPathComponent(stateDirectory, isDirectory: true)
                .appendingPathComponent(caDirectory, isDirectory: true)
                .path
        }

        return URL(fileURLWithPath: directory, isDirectory: true)
            .appendingPathComponent(certificateFile)
            .path
    }
}
