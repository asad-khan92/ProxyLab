//  JSON as a collapsible tree, without losing the order it was sent in.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("JSON node")
struct JSONNodeTests {
    @Test("an object keeps its members in the order they were written")
    func objectOrder() throws {
        let tree = try #require(JSONNode.parse(Data(#"{"zebra":1,"apple":2}"#.utf8)))

        #expect(tree.kind == .object)
        #expect(tree.children.map(\.key) == ["zebra", "apple"])
        #expect(tree.children.map(\.copyableValue) == ["1", "2"])
        #expect(tree.serialized == #"{"zebra": 1, "apple": 2}"#)
    }

    @Test("a string is copied without its quotes")
    func copyStringValue() throws {
        let tree = try #require(JSONNode.parse(Data(#"{"name":"Ada"}"#.utf8)))
        let name = try #require(tree.children.first)

        #expect(name.copyableValue == "Ada")
        #expect(name.serialized == #""Ada""#)
    }

    @Test("an array node serializes its elements")
    func array() throws {
        let tree = try #require(JSONNode.parse(Data("[1,{\"a\":true}]".utf8)))

        #expect(tree.kind == .array)
        #expect(tree.children.count == 2)
        #expect(tree.serialized == #"[1, {"a": true}]"#)
    }

    @Test("bytes that are not JSON produce no tree")
    func notJSON() {
        #expect(JSONNode.parse(Data("hello".utf8)) == nil)
    }
}
