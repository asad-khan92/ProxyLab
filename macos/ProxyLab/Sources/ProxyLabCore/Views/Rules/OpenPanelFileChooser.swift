//  The one place AppKit is used for anything but activating the application.
//
//  `NSOpenPanel` is macOS's file picker, and using it is what keeps the app
//  from having to be a file browser: it shows the file system the way every
//  other application does, it is the only thing that sees any of it, and what
//  comes back is one path.
//
//  Files only — `canChooseDirectories` is off — so "a directory was chosen" is
//  a case the panel does not produce. The editor still checks for it, because a
//  rule read back from the core may name a path that has since become one.

import AppKit

/// A file chooser that is macOS's own open panel.
@MainActor
public final class OpenPanelFileChooser: LocalFileChooser {
    public init() {}

    public func chooseFile(startingAt current: String?) -> String? {
        let panel = NSOpenPanel()
        panel.title = "Choose a File to Answer With"
        panel.message = "ProxyLab's core reads this file each time the rule answers a request."
        panel.prompt = "Choose"
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.resolvesAliases = true
        panel.showsHiddenFiles = false

        // Opening where the rule already points is the difference between
        // re-picking a file and hunting for it again.
        if let current, !current.isEmpty {
            panel.directoryURL = URL(fileURLWithPath: current).deletingLastPathComponent()
            panel.nameFieldStringValue = (current as NSString).lastPathComponent
        }

        guard panel.runModal() == .OK, let url = panel.url else { return nil }

        return url.path
    }
}
