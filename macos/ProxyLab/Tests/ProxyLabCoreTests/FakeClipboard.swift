//  A clipboard a test can read back.
//
//  `NSPasteboard.general` is the developer's own clipboard. A test run that
//  wrote to it would throw away whatever they had copied, which is a rude
//  thing for a test suite to do and also makes the assertion depend on the
//  machine. So the model takes a `Clipboard`, and this is what the tests
//  give it.

import Foundation

@testable import ProxyLabCore

/// A clipboard that remembers what was copied to it.
@MainActor
final class FakeClipboard: Clipboard {
    /// Everything copied, in order.
    private(set) var copies: [String] = []

    /// The last thing copied, if anything was.
    var last: String? { copies.last }

    func copy(_ text: String) {
        copies.append(text)
    }
}
