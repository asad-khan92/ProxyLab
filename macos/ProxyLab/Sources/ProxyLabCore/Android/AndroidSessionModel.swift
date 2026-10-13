//  Pointing an Android Emulator at ProxyLab, and putting it back exactly as it
//  was.
//
//      ┌ ANDROID EMULATOR ──────────────┐
//      │ [ Pixel_10_Pro_XL — emulator-…]│
//      │ ● Connected — via 10.0.2.2:9090│
//      │ [ Connect ] [ Disconnect ] [CA]│
//      └────────────────────────────────┘
//
//  # This one really does connect
//
//  Unlike the iOS Simulator — which has no proxy settings of its own and reads
//  this Mac's — an emulator has one setting, `global http_proxy`, that adb can
//  read and write. So Connect here is a change and not a check, and everything
//  below is about making that change reversible.
//
//  `10.0.2.2` is the standard emulator's alias for the Mac's loopback
//  interface, so the proxy the core is already running on `127.0.0.1` is
//  reachable without a second listener, without LAN exposure, and without
//  touching this Mac's own proxy settings. None of those are done.
//
//  # The previous setting is saved once, not on every connect
//
//  Connecting an emulator that is already connected must not record ProxyLab's
//  own proxy as "what was there before" — a disconnect would then restore
//  ProxyLab, which is not restoring anything. So the saved value is written
//  the first time a serial is connected and left alone afterwards, and a
//  connect that finds ProxyLab's own address already set with nothing saved
//  records *no proxy* instead: an emulator pointing at ProxyLab is ProxyLab's
//  own leftover, not a configuration a developer chose.
//
//  # An emulator that goes away keeps its saved setting
//
//  Shutting an emulator down does not clear `global http_proxy` — it is in the
//  settings database and survives a reboot. So a vanished emulator is reported
//  and its saved value is **kept**, and a disconnect after it comes back still
//  puts the developer's own setting back.

import Foundation
import Observation

import AndroidTools

/// Where an emulator stands with ProxyLab.
public enum AndroidConnection: Equatable, Sendable {
    /// Nothing is connected.
    case notConnected
    /// Connect is part-way through.
    case connecting
    /// The emulator's proxy is ProxyLab's.
    case connected(serial: String)

    /// Which emulator this is about, when it is about one.
    public var serial: String? {
        switch self {
        case .notConnected, .connecting: nil
        case let .connected(serial): serial
        }
    }
}

/// Something worth saying about the last thing that happened.
public struct AndroidNote: Equatable, Sendable {
    /// The sentence to show.
    public let text: String
    /// Whether it is something that went wrong, which is how it is coloured.
    public let isFailure: Bool

    public init(text: String, isFailure: Bool) {
        self.text = text
        self.isFailure = isFailure
    }
}

/// The running emulators, which one is selected, and what connecting it did.
@MainActor
@Observable
public final class AndroidSessionModel {
    /// The emulators that were running when this was last refreshed.
    public private(set) var emulators: [AndroidEmulator] = []

    /// Which one the picker has. Settable, because a picker binds to it.
    public var selected: String?

    /// Where the selected emulator stands.
    public private(set) var state: AndroidConnection = .notConnected

    /// What the last action had to say, if anything.
    public private(set) var note: AndroidNote?

    /// Whether a command is running, which is what disables the buttons.
    public private(set) var isWorking = false

    /// Whether discovery has been asked at least once.
    public private(set) var hasRefreshed = false

    /// The serial whose CA file was copied during this session.
    ///
    /// Delivery is a file copy, not a trust check. The emulator does not
    /// report whether that file was installed.
    private var deliveredCertificateSerial: String?

    /// Whether `adb` was found when this model was created.
    public var isAdbAvailable: Bool { runner != nil }

    /// What each connected emulator's proxy was before ProxyLab touched it.
    ///
    /// Kept by serial and kept across a disappearance, because the setting it
    /// describes is kept by the emulator across a reboot.
    private var savedProxy: [String: EmulatorProxy] = [:]

    private let runner: (any AdbCommandRunner)?
    private let address: EmulatorProxyAddress
    private let certificatePath: String?
    private let certificateExists: @Sendable (String) -> Bool

    /// - Parameters:
    ///   - runner: the adb to run, or `nil` when this Mac has none.
    ///   - port: the port the core's proxy listens on.
    ///   - certificatePath: the CA certificate the core generated.
    ///   - certificateExists: whether it is there, which a test may answer for.
    public init(
        runner: (any AdbCommandRunner)? = AdbLocation.resolve().map { AdbRunner(adb: $0) },
        port: Int = CoreLocations.proxyPort,
        certificatePath: String? = CoreLocations.caCertificate(),
        certificateExists: @escaping @Sendable (String) -> Bool = { path in
            FileManager.default.fileExists(atPath: path)
        }
    ) {
        self.runner = runner
        address = .onHostLoopback(port: port)
        self.certificatePath = certificatePath
        self.certificateExists = certificateExists
    }

    /// The emulator the picker has selected, when it is still running.
    public var selectedEmulator: AndroidEmulator? {
        guard let selected else { return nil }

        return emulators.first { $0.serial == selected }
    }

    /// Where the emulator is asked to send its traffic.
    public var proxyAddress: String { address.description }

    /// What to tap to install the CA, once it has been delivered.
    public var certificateSteps: [String] { CertificateInstructions.steps }

    /// The part about user CAs that has to be said out loud.
    public var certificateLimitation: String { CertificateInstructions.limitation }

    /// Whether Connect can be pressed.
    public var canConnect: Bool {
        !isWorking && (selectedEmulator?.isUsable ?? false)
    }

    /// Whether Disconnect can be pressed.
    public var canDisconnect: Bool { !isWorking && state.serial != nil }

    /// Whether the CA can be sent across.
    public var canDeliverCertificate: Bool {
        !isWorking && (selectedEmulator?.isUsable ?? false)
    }

    /// Whether the connected emulator was given the CA file this session.
    var certificateDelivered: Bool {
        guard let serial = state.serial else { return false }

        return deliveredCertificateSerial == serial
    }

    /// What the status line says.
    public var statusText: String {
        switch state {
        case .notConnected:
            emulators.isEmpty ? "No emulator running" : "Not connected"
        case .connecting:
            "Connecting…"
        case let .connected(serial):
            "Connected — \(name(of: serial)) via \(address)"
        }
    }

    /// Asks adb which emulators are running.
    ///
    /// Keeps the selection when the emulator it names is still running, and
    /// drops the session — but not the saved proxy — when the connected one
    /// has gone.
    public func refresh() async {
        guard !isWorking else { return }
        guard let runner else {
            note = failure(.adbNotFound)
            hasRefreshed = true

            return
        }

        isWorking = true
        defer {
            isWorking = false
            hasRefreshed = true
        }

        let running: [AndroidEmulator]
        do {
            running = try await EmulatorCatalog(runner: runner).runningEmulators()
        } catch {
            note = failure(error)

            return
        }

        emulators = running

        if let selected, !running.contains(where: { $0.serial == selected }) {
            self.selected = nil
        }
        if selected == nil {
            selected = running.first(where: \.isUsable)?.serial ?? running.first?.serial
        }

        if let connected = state.serial, !running.contains(where: { $0.serial == connected }) {
            state = .notConnected
            deliveredCertificateSerial = nil
            note = AndroidNote(
                text: "\(connected) is no longer running. Its proxy setting is still "
                    + "ProxyLab's — the emulator keeps it across a restart. Start it again and "
                    + "press Disconnect to put back what was there before.",
                isFailure: true
            )

            return
        }

        note = running.isEmpty ? failure(.noRunningEmulator) : nil
    }

    /// Saves the emulator's proxy setting, then points it at ProxyLab.
    ///
    /// - Parameter coreIsRunning: whether the window's IPC connection is up,
    ///   which is this application's one answer to whether ProxyLab is
    ///   running.
    public func connect(coreIsRunning: Bool) async {
        guard !isWorking else { return }
        guard let runner else {
            note = failure(.adbNotFound)

            return
        }
        guard let emulator = selectedEmulator else {
            note = failure(.noRunningEmulator)

            return
        }
        guard emulator.isUsable else {
            note = failure(.deviceUnavailable(serial: emulator.serial, state: emulator.state))

            return
        }
        guard coreIsRunning else {
            state = .notConnected
            note = AndroidNote(
                text: "ProxyLab is not running. Start the core — the connection indicator at "
                    + "the top of this window turns green — and connect again.",
                isFailure: true
            )

            return
        }

        isWorking = true
        state = .connecting
        defer { isWorking = false }

        let control = EmulatorProxyControl(runner: runner)
        do {
            let current = try await control.currentProxy(on: emulator.serial)

            // Written once per serial. A second Connect must not record
            // ProxyLab's own proxy as what was there before.
            if savedProxy[emulator.serial] == nil {
                savedProxy[emulator.serial] = current == .configured(address.description)
                    ? .disabled
                    : current
            }

            try await control.connect(emulator.serial, to: address)
        } catch {
            state = .notConnected
            note = failure(error)

            return
        }

        state = .connected(serial: emulator.serial)
        note = AndroidNote(
            text: "\(emulator.label) now proxies through \(address), and its HTTP traffic "
                + "appears in the list. For HTTPS, send it the CA and install it. "
                + "\(CertificateInstructions.limitation)",
            isFailure: false
        )
    }

    /// Puts back the proxy setting that was there before ProxyLab connected.
    public func disconnect() async {
        guard !isWorking else { return }
        guard let serial = state.serial else { return }
        guard let runner else {
            note = failure(.adbNotFound)

            return
        }

        isWorking = true
        defer { isWorking = false }

        // Nothing saved would mean connecting never got as far as saving, so
        // the emulator is left with no proxy rather than with ProxyLab's.
        let previous = savedProxy[serial] ?? .disabled
        let name = name(of: serial)

        do {
            try await EmulatorProxyControl(runner: runner).restore(previous, on: serial)
        } catch {
            state = .notConnected
            note = AndroidNote(
                text: "\(name) was disconnected, and its previous proxy setting could not be "
                    + "put back: \(message(for: error)) The saved setting is kept — press "
                    + "Disconnect again once the emulator is answering.",
                isFailure: true
            )

            return
        }

        savedProxy[serial] = nil
        deliveredCertificateSerial = nil
        state = .notConnected
        note = AndroidNote(
            text: previous.isProxied
                ? "\(name) disconnected, and its proxy is back to \(previous.description)."
                : "\(name) disconnected, and its proxy is back to none, as it was.",
            isFailure: false
        )
    }

    /// Copies ProxyLab's CA onto the emulator, where the installer can find it.
    public func deliverCertificate() async {
        guard !isWorking else { return }
        guard let runner else {
            note = failure(.adbNotFound)

            return
        }
        guard let emulator = selectedEmulator, emulator.isUsable else {
            note = failure(.noRunningEmulator)

            return
        }
        guard let certificatePath else {
            note = AndroidNote(
                text: "ProxyLab's CA certificate cannot be located: there is no home directory "
                    + "to look under. Set \(CoreLocations.caDirectoryVariable) to the core's "
                    + "CA directory.",
                isFailure: true
            )

            return
        }

        isWorking = true
        defer { isWorking = false }

        do {
            let landed = try await CertificateDelivery(
                runner: runner,
                certificateExists: certificateExists
            )
            .deliver(certificateAt: certificatePath, to: emulator.serial)
            deliveredCertificateSerial = emulator.serial
            note = AndroidNote(
                text: "ProxyLab's CA is on the emulator at \(landed). Install it: "
                    + CertificateInstructions.steps.joined(separator: " ")
                    + " " + CertificateInstructions.limitation,
                isFailure: false
            )
        } catch {
            note = failure(error)
        }
    }

    /// What to call an emulator that may since have been shut down.
    private func name(of serial: String) -> String {
        emulators.first { $0.serial == serial }?.label ?? serial
    }

    private func failure(_ error: AndroidToolsError) -> AndroidNote {
        AndroidNote(text: error.message, isFailure: true)
    }

    private func failure(_ error: any Error) -> AndroidNote {
        AndroidNote(text: message(for: error), isFailure: true)
    }

    private func message(for error: any Error) -> String {
        (error as? AndroidToolsError)?.message
            ?? "The emulator could not be reached over adb."
    }
}
