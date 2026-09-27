//  Turning the parts of an exchange into text something else can hold.
//
//      [ Copy headers ]  ──▶  Content-Type: application/json
//                             Authorization: Bearer …
//
//  Everything here reads the stored exchange and produces a string for the
//  clipboard or a name for a save panel. Nothing is fetched, nothing is
//  re-encoded on the way out, and nothing is written anywhere: the caller
//  decides where the string goes.
//
//  There is no cURL here. A cURL command is written by the core, from the
//  request the core stored, and a second serializer in this language would be
//  a second answer to the same question.
//
//  # Nothing produced here is logged
//
//  These strings carry whatever the exchange carried — an `Authorization`
//  header, a session cookie, a body. They are shown and they are copied,
//  because that is what a debugging proxy is for, and they go nowhere else.

import Foundation
import UniformTypeIdentifiers

/// The exchange, as text.
public enum ExchangeText {
    /// Headers, one per line, in the order the message carried them.
    ///
    /// A value that was not text has nothing to write, so its length is
    /// written instead — a copy that silently dropped the header would be a
    /// copy that disagrees with what is on screen.
    public static func headers(_ headers: Headers) -> String {
        headers.entries
            .map { header in
                switch header.value {
                case let .text(text): "\(header.name): \(text)"
                case let .bytes(bytes): "\(header.name): <\(bytes.count) bytes, not text>"
                }
            }
            .joined(separator: "\n")
    }

    /// Query parameters, decoded, one per line.
    ///
    /// A parameter that had no `=` is written as the bare name it was, which
    /// is the difference between it and one whose value was empty.
    public static func queryParameters(_ parameters: [QueryParameter]) -> String {
        parameters
            .map { parameter in
                guard let value = parameter.value else { return parameter.name }

                return "\(parameter.name): \(value)"
            }
            .joined(separator: "\n")
    }

    /// One body as text to put on the clipboard.
    ///
    /// The whole captured body, not the shortened form the window draws: a
    /// copy is how a developer gets at the part that was too long to show.
    /// Bytes that are not text are Base64, because pasting them as characters
    /// would paste something the server never sent.
    public static func body(_ inspection: BodyInspection) -> String {
        switch inspection.content {
        case .empty:
            return ""
        case let .text(text):
            return String(data: inspection.capture.data, encoding: text.encoding)
                ?? text.text
        case .image, .binary, .undecodable:
            return base64(inspection.capture.data)
        }
    }

    /// Bytes as Base64, in one line.
    public static func base64(_ data: Data) -> String {
        data.base64EncodedString()
    }

    /// What to call the file a captured body is saved to.
    ///
    /// The last part of the URL's path when it has one, because that is the
    /// name a developer recognises, and the host otherwise. The extension is
    /// the one macOS gives the declared media type — not one this file keeps a
    /// list of — and the name is left without one when the type is unknown
    /// rather than being given a misleading `.txt`.
    public static func suggestedFilename(
        url: String,
        contentType: ContentType?,
        half: Half
    ) -> String {
        let components = URLComponents(string: url)
        let path = components?.path ?? ""
        let last = path.split(separator: "/").last.map(String.init) ?? ""

        var stem = last.isEmpty ? (components?.host ?? "body") : last
        if half == .request { stem = "\(stem)-request" }

        let existing = (stem as NSString).pathExtension
        guard existing.isEmpty else { return stem }
        guard let contentType,
            let type = UTType(mimeType: contentType.essence),
            let suffix = type.preferredFilenameExtension
        else {
            return stem
        }

        return "\(stem).\(suffix)"
    }

    /// Which half of an exchange something belongs to.
    public enum Half: String, Equatable, Sendable {
        case request = "Request"
        case response = "Response"
    }
}
