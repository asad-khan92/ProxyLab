//  Reading a captured URL's query string.
//
//  The rule these are protecting: the captured URL is never changed, and the
//  parameters read out of it are what the server receiving it would read. A
//  name that appears twice appears twice; a parameter with no `=` is not the
//  same as one with an empty value; and a piece nobody encoded properly is
//  shown as it arrived rather than dropped.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("Query parameters")
struct QueryParameterTests {
    @Test("ordinary parameters come out in the order the URL carried them")
    func ordinary() {
        let parameters = QueryParameters.of(
            url: "https://api.example.com/users?page=2&limit=20&category_id=abc-123"
        )

        #expect(parameters.map(\.name) == ["page", "limit", "category_id"])
        #expect(parameters.map(\.value) == ["2", "20", "abc-123"])
        #expect(parameters.map(\.position) == [0, 1, 2])
    }

    @Test("a URL with no query string has no parameters")
    func none() {
        #expect(QueryParameters.of(url: "https://api.example.com/users").isEmpty)
        #expect(QueryParameters.of(url: "https://api.example.com/users?").isEmpty)
    }

    @Test("a URL that is not one is not guessed at")
    func unparsable() {
        #expect(QueryParameters.of(url: "not a url at all").isEmpty)
    }

    @Test("a request target that is not an absolute URL still gives up its query")
    func relativeTarget() {
        // The core captures an absolute URL for every exchange it can, and a
        // request refused before that was worked out keeps what it arrived
        // with.
        let parameters = QueryParameters.of(url: "/v1/users?page=2")

        #expect(parameters.map(\.name) == ["page"])
        #expect(parameters.first?.value == "2")
    }

    @Test("a fragment is not part of the query")
    func fragment() {
        let parameters = QueryParameters.of(url: "https://example.com/p?a=1#section=2")

        #expect(parameters.map(\.name) == ["a"])
        #expect(parameters.first?.value == "1")
    }

    @Test("a name that appears more than once appears more than once")
    func duplicates() {
        let parameters = QueryParameters.of(url: "https://example.com/p?tag=a&tag=b&tag=c")

        #expect(parameters.count == 3)
        #expect(parameters.map(\.name) == ["tag", "tag", "tag"])
        #expect(parameters.map(\.value) == ["a", "b", "c"])
        // Each is its own row, which is what tells them apart on screen.
        #expect(Set(parameters.map(\.id)).count == 3)
    }

    @Test("a parameter with no equals sign has no value, which is not an empty one")
    func noEquals() {
        let parameters = QueryParameters.of(url: "https://example.com/p?flag&empty=&full=1")

        #expect(parameters.map(\.name) == ["flag", "empty", "full"])
        #expect(parameters[0].value == nil)
        #expect(parameters[1].value == "")
        #expect(parameters[2].value == "1")
    }

    @Test("percent encoding is decoded for reading, and the wire is kept beside it")
    func percentEncoding() {
        let parameters = QueryParameters.of(
            url: "https://example.com/p?path=%2Fusers%2F1&q=a%26b"
        )

        #expect(parameters[0].value == "/users/1")
        #expect(parameters[0].rawValue == "%2Fusers%2F1")
        #expect(parameters[0].wasEncoded)
        // An encoded separator is part of the value, not a new parameter.
        #expect(parameters.count == 2)
        #expect(parameters[1].value == "a&b")
    }

    @Test("a plus is the space the server will read")
    func plusIsSpace() {
        let parameters = QueryParameters.of(url: "https://example.com/p?search=iPhone+17")

        #expect(parameters[0].value == "iPhone 17")
        #expect(parameters[0].rawValue == "iPhone+17")
    }

    @Test("UTF-8 values are decoded as the characters they are")
    func utf8() {
        let parameters = QueryParameters.of(
            url: "https://example.com/p?name=Ada%20L%C3%B6velace&emoji=%F0%9F%92%A1"
                + "&greek=%CE%BB%CF%8C%CE%B3%CE%BF%CF%82"
        )

        #expect(parameters[0].value == "Ada Lövelace")
        #expect(parameters[1].value == "💡")
        #expect(parameters[2].value == "λόγος")
    }

    @Test("an encoded name is decoded too")
    func encodedName() {
        let parameters = QueryParameters.of(url: "https://example.com/p?first%20name=Ada")

        #expect(parameters[0].name == "first name")
        #expect(parameters[0].rawName == "first%20name")
        #expect(parameters[0].wasEncoded)
    }

    @Test("a percent sign that is not an escape is left exactly as it arrived")
    func brokenEncoding() {
        let parameters = QueryParameters.of(url: "https://example.com/p?discount=100%&a=1")

        #expect(parameters[0].value == "100%")
        #expect(parameters[0].rawValue == "100%")
    }

    @Test("only the first equals sign separates a name from its value")
    func equalsInValue() {
        let parameters = QueryParameters.of(url: "https://example.com/p?token=abc=def==")

        #expect(parameters.count == 1)
        #expect(parameters[0].name == "token")
        #expect(parameters[0].value == "abc=def==")
    }

    @Test("an empty piece between separators is not a parameter")
    func emptyPieces() {
        let parameters = QueryParameters.of(url: "https://example.com/p?a=1&&b=2")

        #expect(parameters.map(\.name) == ["a", "b"])
    }

    @Test("a value that needed no decoding is not marked as encoded")
    func unencoded() {
        let parameters = QueryParameters.of(url: "https://example.com/p?page=2")

        #expect(!parameters[0].wasEncoded)
    }
}
