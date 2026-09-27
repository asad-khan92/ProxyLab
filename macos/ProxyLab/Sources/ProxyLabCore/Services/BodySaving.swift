//  Writing a captured body to a file the person chose.
//
//      [ Save… ] ──▶ NSSavePanel ──▶ a URL ──▶ the captured bytes, unchanged
//
//  The app never picks a location. It asks macOS for one, and macOS is what
//  shows the file system, so there is no path this code can be talked into
//  writing to that a person did not choose in a panel. Nothing is created
//  until there is a URL, nothing is opened afterwards, and no permissions are
//  changed.
//
//  What is written is `BodyCapture.data`: the bytes the core captured, byte for
//  byte. A body that was truncated writes the truncated bytes — the rest was
//  never captured and there is nothing to go and get.
//
//  It is a protocol for the same reason the clipboard is one: so that a test
//  can say what would have been written without a panel that needs a person to
//  click it, and without a test run leaving files on a developer's disk.

import AppKit

/// Somewhere a captured body can be written.
@MainActor
public protocol BodyDestination: AnyObject {
    /// Asks for a location and writes `data` there.
    ///
    /// Gives back the URL that was written, or `nil` if the person cancelled —
    /// which is not an error and is not reported as one. Throws only when a
    /// location was chosen and the write failed.
    func save(_ data: Data, suggestedName: String) throws -> URL?
}

/// macOS's own save panel, and a write to what comes back from it.
@MainActor
public final class SavePanelDestination: BodyDestination {
    public init() {}

    public func save(_ data: Data, suggestedName: String) throws -> URL? {
        let panel = NSSavePanel()
        panel.title = "Save Captured Body"
        panel.message = "The bytes ProxyLab captured are written exactly as they were captured."
        panel.prompt = "Save"
        panel.nameFieldStringValue = suggestedName
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false

        guard panel.runModal() == .OK, let url = panel.url else { return nil }

        // No `.atomic`: an atomic write replaces the file through a temporary
        // one of its own, which drops the permissions the person's chosen
        // location already had.
        try data.write(to: url)

        return url
    }
}
