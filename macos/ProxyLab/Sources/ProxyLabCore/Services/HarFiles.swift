//  Choosing where a HAR file goes, and which one to read.
//
//      [ Export… ] ──▶ NSSavePanel ──▶ a path ──▶ the core writes it
//      [ Import… ] ──▶ NSOpenPanel ──▶ a path ──▶ the core reads it
//
//  The app never opens the file. It asks macOS for a location and sends the
//  path; the core writes or reads it, and is the only thing that ever does.
//  So this is a picker and not a file manager: nothing here creates a
//  directory, overwrites a file, or reads a byte — the panel is what shows the
//  file system, and it is the only thing that sees any of it.
//
//  It is the division ``LocalFileChooser`` already has for a Map Local rule,
//  and it is here for the same two reasons: the core is authoritative for the
//  captured traffic, so the core is what writes it down; and a protocol means
//  a test can say what path would have been chosen without a panel that needs
//  a person to click it, and without a test run leaving files on a developer's
//  disk.
//
//  # The warning on the way out
//
//  A HAR file contains what the traffic contained: credentials, cookies,
//  tokens, bodies. The panel says so, because the file is about to leave
//  ProxyLab's own directory and go wherever the person puts it.

import AppKit
import Foundation
import UniformTypeIdentifiers

/// Somewhere a HAR file can be written, and one that can be read.
///
/// Both give back `nil` when the person cancelled, which is an answer rather
/// than a failure and is not reported as one.
@MainActor
public protocol HarFileChooser: AnyObject {
    /// Asks where to write a HAR file, suggesting `name`.
    func chooseExportPath(suggestedName: String) -> String?
    /// Asks which HAR file to read.
    func chooseImportPath() -> String?
}

/// macOS's own panels, and the paths that come back from them.
@MainActor
public final class PanelHarFileChooser: HarFileChooser {
    public init() {}

    public func chooseExportPath(suggestedName: String) -> String? {
        let panel = NSSavePanel()
        panel.title = "Export Traffic as HAR"
        panel.message =
            "A HAR file contains what the traffic contained, including headers, cookies, and "
            + "credentials."
        panel.prompt = "Export"
        panel.nameFieldStringValue = suggestedName
        panel.allowedContentTypes = Self.harTypes
        panel.allowsOtherFileTypes = true
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false

        guard panel.runModal() == .OK, let url = panel.url else { return nil }

        return url.path
    }

    public func chooseImportPath() -> String? {
        let panel = NSOpenPanel()
        panel.title = "Import a HAR File"
        panel.message =
            "The entries are added to the traffic list to be inspected. Nothing in the file is "
            + "sent."
        panel.prompt = "Import"
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.allowedContentTypes = Self.harTypes
        panel.allowsOtherFileTypes = true

        guard panel.runModal() == .OK, let url = panel.url else { return nil }

        return url.path
    }

    /// What a HAR file is, as macOS types it.
    ///
    /// There is no registered type for `.har`, so the extension is declared
    /// against JSON — which is what the file is — and other types are allowed
    /// besides, because a colleague's export may be named anything.
    private static let harTypes: [UTType] = [
        UTType(filenameExtension: "har") ?? .json,
        .json,
    ]
}

/// What an exported file is called when nobody says otherwise.
public enum HarFileName {
    /// A name for an export of everything, or of one host's traffic.
    ///
    /// The host when the list is narrowed to one, so a directory of exports
    /// reads as a list of services rather than a list of dates alone; the
    /// date either way, because the next export is a different session.
    public static func suggested(host: String?, at moment: Date = Date()) -> String {
        let stamp = DateFormatter.harStamp.string(from: moment)

        guard let host, !host.isEmpty else { return "ProxyLab \(stamp).har" }

        return "\(safe(host)) \(stamp).har"
    }

    /// The host with anything that is not a file name taken out.
    ///
    /// A host is a DNS name and is already safe, but the filter is here
    /// rather than assumed: what is being built is a file name, and a `/` in
    /// one is a directory somebody did not ask for.
    private static func safe(_ host: String) -> String {
        let kept = host.filter { character in
            character.isLetter || character.isNumber || character == "." || character == "-"
        }

        return kept.isEmpty ? "ProxyLab" : kept
    }
}

private extension DateFormatter {
    /// `2026-09-20 at 10.11.12`, which is how macOS itself stamps a file.
    ///
    /// A fixed locale and calendar, so the name does not depend on where the
    /// developer is: a file name is not a thing to localise.
    static let harStamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"

        return formatter
    }()
}
