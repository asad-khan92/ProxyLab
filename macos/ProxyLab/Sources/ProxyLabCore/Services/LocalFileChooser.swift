//  Choosing the file a Map Local rule answers with.
//
//      [ Choose File… ] ──▶ NSOpenPanel ──▶ a path, or nothing
//
//  The app never opens the file. It asks macOS for a path, checks that the path
//  is one the core could read, and sends the path — the core reads the file
//  when the rule runs, and is the only thing that ever does. So this is a
//  picker and not a file browser: there is no directory listing, no preview,
//  and nothing on disk is copied, moved, or created.
//
//  It is a protocol so the editor's behaviour — a cancelled pick, a file that
//  is not there, a directory chosen anyway — is testable without a panel that
//  needs a person to click it.

import Foundation

/// Why a chosen path is not one a Map Local rule can name.
public enum LocalFileError: Error, Sendable, Equatable {
    /// The field is empty.
    case empty
    /// The path is relative, and the core has nothing to resolve it against.
    case notAbsolute
    /// There is nothing at that path.
    case missing
    /// There is a directory at that path.
    case isDirectory
    /// The file is there, and this user cannot read it.
    case unreadable

    /// What a person is told.
    public var message: String {
        switch self {
        case .empty: "Choose the file this rule answers with."
        case .notAbsolute: "A Map Local path must be absolute."
        case .missing: "There is no file at that path."
        case .isDirectory: "That is a folder. Choose a file."
        case .unreadable: "That file cannot be read."
        }
    }
}

/// Something that can ask a person for a file.
@MainActor
public protocol LocalFileChooser: AnyObject {
    /// Asks for one file, and gives back its path — or `nil` if the person
    /// cancelled, which is not an error and is not reported as one.
    func chooseFile(startingAt current: String?) -> String?
}

/// Whether a path is one the core could answer a request with.
///
/// Checked here so the editor can say what is wrong under the field, and
/// checked again by the core when the rule is written and once more when the
/// rule runs. This never opens the file: existence, kind, and readability are
/// all the file system can be asked without reading a byte.
public enum LocalFilePath {
    /// Says why `path` is not a file this rule could name, or nothing.
    public static func check(
        _ path: String,
        in fileManager: FileManager = .default
    ) -> LocalFileError? {
        let path = path.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !path.isEmpty else { return .empty }
        // Relative to what? A proxy has no meaningful working directory, and
        // the one it happens to have started in is not something a rule
        // should silently depend on.
        guard path.hasPrefix("/") else { return .notAbsolute }

        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: path, isDirectory: &isDirectory) else {
            return .missing
        }
        if isDirectory.boolValue { return .isDirectory }
        guard fileManager.isReadableFile(atPath: path) else { return .unreadable }

        return nil
    }
}
