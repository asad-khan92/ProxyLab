//  A place to save a body that is not a disk.
//
//  `SavePanelDestination` shows a panel and writes a file. A test run must do
//  neither: nobody is there to click, and a suite that leaves files behind is
//  a suite that fails differently on the second run. So the model takes a
//  `BodyDestination`, and this is what the tests give it.

import Foundation

@testable import ProxyLabCore

/// A destination that remembers what it was asked to write.
@MainActor
final class FakeBodyDestination: BodyDestination {
    /// Everything it was asked to save, in order.
    private(set) var saves: [(data: Data, name: String)] = []

    /// What to answer with: a URL, `nil` for a cancelled panel, or a throw.
    var answer: Result<URL?, any Error> = .success(URL(fileURLWithPath: "/tmp/saved.bin"))

    func save(_ data: Data, suggestedName: String) throws -> URL? {
        saves.append((data, suggestedName))

        return try answer.get()
    }
}

/// A failure a write can end in.
struct FakeWriteFailure: Error, LocalizedError {
    var errorDescription: String? { "the disk said no" }
}
