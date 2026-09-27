//  Writing captured traffic out as a HAR file, and reading one back in.
//
//      [ Export… ] ──▶ NSSavePanel ──▶ a path ──▶ export_har ──▶ the core writes it
//      [ Import… ] ──▶ NSOpenPanel ──▶ a path ──▶ import_har ──▶ the core reads it
//
//  The one pair of operations where the app sends a *path* rather than the
//  thing itself. A HAR document is the captured traffic with its bodies in
//  it — megabytes, where one IPC request line is 64 KiB — so it cannot travel
//  over the socket in either direction. The app asks macOS for a location, and
//  the core is what opens it.
//
//  So this app never writes or parses a HAR file. It does not know what HAR
//  1.2 is, does not assemble one out of the rows it happens to be holding, and
//  has no second copy of the format to keep in step: the core is authoritative
//  for what was captured, and exporting it is the core writing down what it
//  already has. That is the same division `Copy as cURL` has.
//
//  # What an export contains
//
//  Whatever the traffic contained, credentials and cookies included — the
//  point of a HAR file is that it reproduces the traffic, and one that dropped
//  the `Authorization` would reproduce nothing. Which is exactly why exporting
//  is something a person does by name, through a panel, to a location they
//  chose.

import Foundation

/// Which exchanges an export is for.
///
/// One or the other, never both: an export writes what somebody picked, or it
/// writes what their list is narrowed to. The core refuses a request that
/// names both, because either reading of it would export something nobody
/// asked for.
public enum HarSelection: Sendable, Equatable {
    /// These exchanges, by id.
    case exchanges([String])
    /// Every exchange these filters match, oldest first.
    ///
    /// The same filter values a list request carries. With nothing set it is
    /// the whole stored history.
    case filtered(ListTrafficParameters)
}

/// A request to write a HAR file.
public struct HarExportRequest: Sendable, Equatable {
    /// Where to write it. An absolute path, from a save panel.
    public var path: String
    /// Which exchanges go in it.
    public var selection: HarSelection

    public init(path: String, selection: HarSelection) {
        self.path = path
        self.selection = selection
    }
}

/// What an export came to.
public struct HarExportResult: Codable, Hashable, Sendable {
    /// How many exchanges were written.
    public let entries: UInt64
    /// How many bytes the file is.
    public let bytes: UInt64
    /// Whether there were more matching exchanges than one export writes.
    public let truncated: Bool

    public init(entries: UInt64, bytes: UInt64, truncated: Bool) {
        self.entries = entries
        self.bytes = bytes
        self.truncated = truncated
    }
}

/// What an import came to.
public struct HarImportResult: Codable, Hashable, Sendable {
    /// How many entries became stored exchanges.
    public let imported: UInt64
    /// How many did not.
    ///
    /// An entry that names no request, and an entry past the limit one import
    /// takes. Counted rather than passed over, so a shorter list than the file
    /// had is something a person is told about.
    public let skipped: UInt64

    public init(imported: UInt64, skipped: UInt64) {
        self.imported = imported
        self.skipped = skipped
    }
}
