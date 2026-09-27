//  Putting text on the Mac's clipboard.
//
//      [ Copy as cURL ] ──▶ NSPasteboard.general
//
//  One method, and a protocol around it so that a test can say what was
//  copied without a test run writing over whatever the developer had on their
//  clipboard. `SystemClipboard` is the only implementation that ships.
//
//  There is no clipboard history here, and nothing reads the clipboard: this
//  writes, and that is all it does.
//
//  # Nothing copied is logged
//
//  A generated cURL command carries whatever the request carried — an
//  `Authorization` header, a session cookie, a body — which is the point of
//  copying it and is exactly why it is never written anywhere but the
//  clipboard.

import AppKit

/// Somewhere text can be copied to.
@MainActor
public protocol Clipboard: AnyObject {
    /// Replaces the clipboard's contents with `text`.
    func copy(_ text: String)
}

/// The Mac's own clipboard.
///
/// `clearContents()` before writing is required by AppKit — a pasteboard must
/// be cleared to take ownership of it — and is also what makes a copy a
/// replacement rather than an addition to whatever was there.
@MainActor
public final class SystemClipboard: Clipboard {
    private let pasteboard: NSPasteboard

    /// A clipboard writing to `pasteboard`, the general one by default.
    public init(pasteboard: NSPasteboard = .general) {
        self.pasteboard = pasteboard
    }

    public func copy(_ text: String) {
        pasteboard.clearContents()
        // As a plain string, so a paste into a terminal, an editor, or a chat
        // window is the command and not a styled version of it. The newlines
        // and the backslashes that continue them go across unchanged.
        pasteboard.setString(text, forType: .string)
    }
}
