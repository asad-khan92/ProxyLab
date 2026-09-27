//  Working out how a captured body can honestly be shown.
//
//      declared text, and it decodes   ──▶  text, in the encoding it decoded as
//      text that parses as JSON        ──▶  laid out as well as raw
//      an image                        ──▶  previewed, or described if it will not decode
//      declared text, nothing decodes  ──▶  said so, and offered as bytes
//      anything else                   ──▶  said to be binary, with its size
//
//  Nothing here writes back to the exchange. Laying JSON out makes a second
//  string for the view; the stored body stays the bytes the core captured, and
//  is what a copy or a save produces. A body that was truncated usually will
//  not parse as JSON, and falls through to text rather than being repaired
//  into something the server never sent.
//
//  The decision is separate from the view so it can be tested as what it is —
//  a decision — rather than through a rendered hierarchy, and so it can be
//  made once per selected exchange, off the main actor, instead of on every
//  redraw.

import Foundation

/// How one captured body can be shown, and what it is.
public struct BodyInspection: Equatable, Sendable {
    /// The bytes, and how much of the body they are.
    public let capture: BodyCapture
    /// What the message said the body is, if it said anything.
    public let contentType: ContentType?
    /// What can be made of it.
    public let content: Content

    public init(capture: BodyCapture, contentType: ContentType?, content: Content) {
        self.capture = capture
        self.contentType = contentType
        self.content = content
    }

    /// What a body turned out to be.
    public enum Content: Equatable, Sendable {
        /// There was no body.
        case empty
        /// Text, decoded.
        case text(TextBody)
        /// Bytes that are an image, for the viewer to try to draw.
        case image(ImageBody)
        /// A body the sender called text, which no encoding would decode.
        case undecodable
        /// Bytes that are not text, described rather than decoded.
        case binary
    }

    /// How much text is put in front of SwiftUI at once.
    ///
    /// The core captures at most a megabyte, and a `Text` is laid out whole
    /// however little of it is on screen — a scroll view does not make the
    /// laying out lazy. This is about eight hundred lines at the pane's usual
    /// width, which is a long read and a responsive one. What is over it is
    /// still captured, still copied, and still saved: it is only not drawn.
    public static let displayedCharacterLimit = 64 * 1_024

    /// How many captured bytes will be shown as Base64 inside the window.
    ///
    /// Base64 is four characters for every three bytes, so this is the same
    /// judgement as ``displayedCharacterLimit`` made against the encoded form.
    public static let displayedBase64ByteLimit = 32 * 1_024

    /// Whether there is no body at all.
    public var isEmpty: Bool { content == .empty }
}

/// A body that decoded as text.
public struct TextBody: Equatable, Sendable {
    /// The text, as far as it is shown.
    public let text: String
    /// The same text laid out for reading, when there is a layout to apply.
    public let formatted: String?
    /// A collapsible tree of the same JSON, when `formatted` is set.
    public let tree: JSONNode?
    /// What the text is, as far as anything can say.
    public let language: Language
    /// The encoding it decoded as.
    public let encoding: String.Encoding
    /// Whether more was captured than is shown here.
    public let shortened: Bool

    public init(
        text: String,
        formatted: String? = nil,
        tree: JSONNode? = nil,
        language: Language = .plain,
        encoding: String.Encoding = .utf8,
        shortened: Bool = false
    ) {
        self.text = text
        self.formatted = formatted
        self.tree = tree
        self.language = language
        self.encoding = encoding
        self.shortened = shortened
    }

    /// What kind of text a body is.
    public enum Language: String, Equatable, Sendable {
        case json = "JSON"
        case xml = "XML"
        case html = "HTML"
        case plain = "Text"
    }

    /// The name of the encoding, for the viewer to show.
    public var encodingName: String {
        String.localizedName(of: encoding).isEmpty
            ? "unknown encoding"
            : String.localizedName(of: encoding)
    }
}

/// A body that is an image.
public struct ImageBody: Equatable, Sendable {
    /// What kind of image, in the shortest form worth printing: `PNG`.
    public let format: String

    public init(format: String) {
        self.format = format
    }
}

extension BodyInspection {
    /// How to show `capture`, given what `contentType` said it is.
    ///
    /// Not `@MainActor`: this is the expensive half of showing a body, and it
    /// is meant to be done away from the actor that draws.
    public static func of(_ capture: BodyCapture, contentType: ContentType? = nil)
        -> BodyInspection
    {
        BodyInspection(
            capture: capture,
            contentType: contentType,
            content: content(of: capture, contentType: contentType)
        )
    }

    /// How to show a WebSocket payload or SSE event data.
    ///
    /// Text messages and SSE data are read the way an HTTP body with no
    /// `Content-Type` is: UTF-8 if it decodes, JSON laid out when the bytes
    /// are JSON. A binary WebSocket payload is shown as bytes even when those
    /// bytes happen to be UTF-8 — the opcode is what it is, not a guess.
    public static func ofStream(_ capture: BodyCapture, kind: StreamKind) -> BodyInspection {
        switch kind {
        case .websocketBinary:
            let content: Content =
                capture.isEmpty || capture.data.isEmpty ? .empty : .binary

            return BodyInspection(capture: capture, contentType: nil, content: content)

        case .websocketText, .sse:
            return of(capture)
        }
    }

    /// The bodies of one exchange, worked out together.
    public static func of(_ exchange: TrafficExchange) -> ExchangeInspection {
        ExchangeInspection(
            exchangeID: exchange.id,
            request: of(
                exchange.request.body,
                contentType: ContentType.of(exchange.request.headers)
            ),
            response: exchange.response.map {
                of($0.body, contentType: ContentType.of($0.headers))
            }
        )
    }

    private static func content(of capture: BodyCapture, contentType: ContentType?) -> Content {
        guard !capture.isEmpty, !capture.data.isEmpty else { return .empty }

        // What the bytes are comes before what they decode to: an image is
        // an image whatever the message called it. SVG is the exception —
        // it is a picture written as text, and the text is the useful way to
        // read it.
        if contentType?.essence != "image/svg+xml",
            let image = image(of: capture.data, contentType: contentType)
        {
            return .image(image)
        }

        guard let decoded = text(of: capture, contentType: contentType) else {
            // The sender said it was text and it is not. That is worth saying
            // out loud, because it is usually a bug and not a binary body.
            return contentType?.isTextual == true ? .undecodable : .binary
        }

        return .text(decoded)
    }

    // MARK: - Text

    /// `capture` decoded, laid out if it can be, or `nil` if no encoding read
    /// it.
    private static func text(of capture: BodyCapture, contentType: ContentType?) -> TextBody? {
        guard let (whole, encoding) = decode(capture, contentType: contentType) else { return nil }

        let shortened = whole.count > displayedCharacterLimit
        let shown = shortened ? String(whole.prefix(displayedCharacterLimit)) : whole
        let declared = declaredLanguage(contentType)

        // Laid out from the captured bytes rather than from the decoded
        // string, so every digit and every escape is the sender's own. Not
        // attempted for a body shortened for display: what is drawn is no
        // longer the whole of what was captured, and laying out half a
        // document would be laying out something nobody sent.
        //
        // Tried when the sender said JSON, and tried again when the sender
        // said nothing structured but the body opens like JSON — which is
        // most of what arrives as `text/plain` from an API being debugged.
        let worthFormatting =
            !shortened && (declared == .json || (declared == nil && opensLikeJSON(whole)))
        let formatted = worthFormatting ? prettyPrintedJSON(capture.data) : nil
        let tree = formatted == nil ? nil : JSONNode.parse(capture.data)

        return TextBody(
            text: shown,
            formatted: formatted,
            tree: tree,
            language: declared ?? (formatted == nil ? .plain : .json),
            encoding: encoding,
            shortened: shortened
        )
    }

    /// `capture` as text, with the encoding that read it.
    ///
    /// The declared character set first, because the sender is the one who
    /// knows; UTF-8 after it, because a great many senders declare nothing and
    /// mean UTF-8. Nothing is tried after that: decoding arbitrary bytes as
    /// Latin-1 always "succeeds", and a page of mojibake presented as the
    /// body is worse than being told the body is not text.
    private static func decode(_ capture: BodyCapture, contentType: ContentType?)
        -> (String, String.Encoding)?
    {
        var encodings: [String.Encoding] = []
        if let declared = contentType?.declaredEncoding { encodings.append(declared) }
        if !encodings.contains(.utf8) { encodings.append(.utf8) }

        for encoding in encodings {
            if let text = String(data: capture.data, encoding: encoding) {
                return (text, encoding)
            }
            // A capture cut at the limit can end in the middle of a character,
            // which is not a body that is not text — it is a body that is
            // missing its last few bytes. Only the tail is given up, and only
            // for a capture that says it was cut.
            if capture.truncated,
                let text = decodeTrimmingTail(capture.data, as: encoding)
            {
                return (text, encoding)
            }
        }

        return nil
    }

    /// `data` decoded after giving up as many as three trailing bytes.
    ///
    /// Three because that is the most a UTF-8 character can be missing and
    /// still be a character; the same allowance covers the two-byte units of
    /// the UTF-16 and UTF-32 encodings a `charset` may name.
    private static func decodeTrimmingTail(_ data: Data, as encoding: String.Encoding) -> String? {
        for dropped in 1...3 where data.count > dropped {
            if let text = String(data: data.dropLast(dropped), encoding: encoding) {
                return text
            }
        }

        return nil
    }

    /// What the sender said the body is, or `nil` if it said nothing that
    /// names a syntax.
    private static func declaredLanguage(_ contentType: ContentType?) -> TextBody.Language? {
        guard let contentType else { return nil }

        if contentType.isJSON { return .json }
        if contentType.isHTML { return .html }
        if contentType.isXML { return .xml }

        return nil
    }

    /// Whether `text` opens the way a JSON object or array does.
    ///
    /// A cheap look before the parse: handing `JSONSerialization` a megabyte
    /// of prose to be told it is prose is work worth not doing.
    private static func opensLikeJSON(_ text: String) -> Bool {
        guard let first = text.first(where: { !$0.isWhitespace }) else { return false }

        return first == "{" || first == "["
    }

    /// `data` as indented JSON, or `nil` if it is not JSON.
    ///
    /// Foundation decides whether the bytes are JSON; `JSONPrettyPrinter`
    /// decides how they are laid out. Foundation's own pretty-printer is not
    /// used for the second half because it prints a parsed dictionary, and a
    /// dictionary has lost the order the fields were sent in.
    private static func prettyPrintedJSON(_ data: Data) -> String? {
        guard let value = try? JSONSerialization.jsonObject(with: data, options: []) else {
            return nil
        }
        // An object or an array. A bare string or number is JSON too, and
        // laying one out over several lines gains nothing over the text that
        // is already there.
        guard value is [Any] || value is [String: Any] else { return nil }

        return JSONPrettyPrinter.format(data)
    }

    // MARK: - Images

    /// What kind of image `data` is, from its own first bytes, falling back to
    /// what the message called it.
    ///
    /// The bytes come first because a message that calls a PNG
    /// `application/octet-stream` is common and a message that calls a ZIP
    /// `image/png` is a bug worth seeing.
    private static func image(of data: Data, contentType: ContentType?) -> ImageBody? {
        if let sniffed = sniffImageFormat(data) { return ImageBody(format: sniffed) }
        guard contentType?.isImage == true, let contentType else { return nil }

        return ImageBody(format: contentType.subtype.uppercased())
    }

    /// The image format `data` starts with, by its magic number.
    private static func sniffImageFormat(_ data: Data) -> String? {
        func starts(with bytes: [UInt8], at offset: Int = 0) -> Bool {
            guard data.count >= offset + bytes.count else { return false }

            let start = data.index(data.startIndex, offsetBy: offset)

            return Array(data[start..<data.index(start, offsetBy: bytes.count)]) == bytes
        }

        if starts(with: [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) { return "PNG" }
        if starts(with: [0xFF, 0xD8, 0xFF]) { return "JPEG" }
        if starts(with: Array("GIF87a".utf8)) || starts(with: Array("GIF89a".utf8)) { return "GIF" }
        if starts(with: Array("RIFF".utf8)), starts(with: Array("WEBP".utf8), at: 8) {
            return "WebP"
        }
        if starts(with: [0x42, 0x4D]) { return "BMP" }
        if starts(with: [0x49, 0x49, 0x2A, 0x00]) || starts(with: [0x4D, 0x4D, 0x00, 0x2A]) {
            return "TIFF"
        }
        if starts(with: [0x00, 0x00, 0x01, 0x00]) { return "ICO" }

        return nil
    }
}

/// The bodies of one exchange, worked out once.
///
/// Held beside the exchange it belongs to rather than inside it: the exchange
/// is the core's record and this is a reading of it, and keeping the id here
/// is what lets a view be sure the two are about the same request.
public struct ExchangeInspection: Equatable, Sendable {
    /// Which exchange these bodies belong to.
    public let exchangeID: String
    /// The request's body.
    public let request: BodyInspection
    /// The response's body, when there was a response.
    public let response: BodyInspection?

    public init(exchangeID: String, request: BodyInspection, response: BodyInspection?) {
        self.exchangeID = exchangeID
        self.request = request
        self.response = response
    }
}
