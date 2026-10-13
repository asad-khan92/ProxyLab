//  The fields a person edits while one exchange is held still.
//
//      paused request ──▶ method, URL, headers, body ──▶ RequestEdit
//      paused response ─▶ status, headers, body      ──▶ ResponseEdit
//      paused WebSocket ─▶ payload                   ──▶ WebSocketEdit
//
//  Text in, an edit out. The editor starts holding exactly what the core sent,
//  so a person who changes nothing produces an edit that changes nothing —
//  which is what `Continue` means when it is pressed without touching
//  anything.
//
//  # Only what changed is sent
//
//  Each field is compared with what arrived, and a field that is the same is
//  left out of the edit. That is not an optimisation: the core leaves a field
//  it was not sent exactly as it was, so leaving the body out is the only way
//  to say "send the body the client sent" for a body too long to have been
//  shown in full.
//
//  # A body is bytes, and is edited as text only when it is text
//
//  A body that is valid UTF-8 is offered as text. One that is not — an image,
//  a protocol buffer, a gzipped response — is shown as a size and cannot be
//  edited: what would be edited is a lossy decoding of it, and writing that
//  back would corrupt the message. A body the core truncated cannot be edited
//  either, for the reason the core gives: what is on screen is not all of it.

import Foundation
import Observation

/// What a person is editing about one held exchange.
@MainActor
@Observable
public final class PauseEditorModel {
    /// The exchange being held, as the core sent it.
    public let paused: PausedExchange

    /// The method the request will be sent with.
    public var method: String
    /// The URL it will be sent to.
    public var url: String
    /// The status the response will be delivered with.
    public var status: String
    /// The headers, one per line, as `Name: value`.
    public var headerText: String
    /// The body, when it is text this editor can offer.
    public var bodyText: String

    /// Why the body cannot be edited, or `nil` when it can.
    public let bodyRefusal: String?

    /// What the headers arrived as, so an untouched section is left out.
    private let originalHeaderText: String
    /// The same for the body.
    private let originalBodyText: String

    public init(_ paused: PausedExchange) {
        self.paused = paused

        let headers = paused.request?.headers ?? paused.response?.headers ?? Headers()
        let body = paused.request?.body ?? paused.response?.body ?? paused.websocket?.payload ?? .empty

        method = paused.request?.method ?? paused.method
        url = paused.request?.url ?? paused.url
        status = paused.response.map { String($0.status) } ?? ""
        originalHeaderText = Self.render(headers)
        headerText = originalHeaderText

        let text = String(data: body.data, encoding: .utf8)
        originalBodyText = text ?? ""
        bodyText = originalBodyText
        bodyRefusal = Self.refusal(for: body, text: text)
    }

    /// Which half is being held.
    public var direction: BreakpointDirection { paused.direction }

    /// Whether the body may be edited at all.
    public var isBodyEditable: Bool { bodyRefusal == nil }

    /// Why the edit cannot be sent, or `nil` when it can.
    ///
    /// The same bounds the core enforces, checked here so the editor can say
    /// so before anything is sent. It is not what decides: the core checks
    /// again, and the core is what holds the exchange.
    public var refusal: String? {
        switch direction {
        case .request:
            if method.trimmingCharacters(in: .whitespaces).isEmpty {
                return "A request needs a method."
            }
            if url.trimmingCharacters(in: .whitespaces).isEmpty {
                return "A request needs a URL."
            }
        case .response:
            guard let code = UInt16(status.trimmingCharacters(in: .whitespaces)),
                (100...599).contains(code)
            else {
                return "A status is a code from 100 to 599."
            }
        case .websocket:
            break
        }

        if direction != .websocket, Self.parse(headerText) == nil {
            return "Each header is one line, written as \"Name: value\"."
        }

        if isBodyEditable, bodyText.utf8.count > maxBreakpointBodyBytes {
            return "An edited body must be at most \(maxBreakpointBodyBytes / 1024) KB."
        }

        return nil
    }

    /// The edits to send, or `nil` when they are not ones the core would take.
    ///
    /// Exactly one of the two is filled in, for the half this pause is for.
    public func edits() -> (request: RequestEdit?, response: ResponseEdit?, websocket: WebSocketEdit?)? {
        guard refusal == nil else { return nil }

        let headers = headerText == originalHeaderText ? nil : Self.parse(headerText)
        let body: Data? = {
            guard isBodyEditable, bodyText != originalBodyText else { return nil }

            return Data(bodyText.utf8)
        }()

        switch direction {
        case .request:
            let original = paused.request
            var edit = RequestEdit(headers: headers, body: body)
            if method != original?.method { edit.method = method }
            if url != original?.url { edit.url = url }

            return (edit.isEmpty ? nil : edit, nil, nil)

        case .response:
            var edit = ResponseEdit(headers: headers, body: body)
            if let code = UInt16(status.trimmingCharacters(in: .whitespaces)),
                code != paused.response?.status
            {
                edit.status = code
            }

            return (nil, edit.isEmpty ? nil : edit, nil)

        case .websocket:
            let edit = WebSocketEdit(payload: body)

            return (nil, nil, edit.isEmpty ? nil : edit)
        }
    }

    // MARK: - The query, as part of the URL

    /// The query `url` carries, without the leading `?`.
    ///
    /// Empty when `url` is not one that has a query to edit. Changing it is
    /// a change to the URL, which is the edit the core already accepts.
    static func query(in url: String) -> String {
        guard let components = URLComponents(string: url), components.host != nil else { return "" }

        return components.query ?? ""
    }

    /// `url` with its query replaced by `query`.
    ///
    /// An empty query removes it. A URL that is not absolute is returned
    /// unchanged, because there is no query component to put back.
    static func url(_ url: String, replacingQuery query: String) -> String {
        guard var components = URLComponents(string: url), components.host != nil else { return url }

        components.query = query.isEmpty ? nil : query

        return components.string ?? url
    }

    // MARK: - Headers as text

    /// The header section written one header per line.
    ///
    /// A value that is not text is left out: it cannot be typed, and a
    /// placeholder that was typed back would replace the real bytes with the
    /// placeholder. A message carrying one is a message whose headers are best
    /// left alone.
    static func render(_ headers: Headers) -> String {
        headers
            .compactMap { header in header.value.text.map { "\(header.name): \($0)" } }
            .joined(separator: "\n")
    }

    /// The header section a person typed, or `nil` when a line is not one.
    static func parse(_ text: String) -> Headers? {
        var entries: [Header] = []

        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = line.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { continue }

            guard let separator = line.firstIndex(of: ":") else { return nil }

            let name = String(line[line.startIndex..<separator]).trimmingCharacters(
                in: .whitespaces
            )
            let value = String(line[line.index(after: separator)...]).trimmingCharacters(
                in: .whitespaces
            )
            if name.isEmpty { return nil }

            entries.append(Header(name: name, value: .text(value)))
        }

        return Headers(entries)
    }

    // MARK: - Whether a body can be edited

    /// Why `body` cannot be edited, or `nil` when it can.
    private static func refusal(for body: BodyCapture, text: String?) -> String? {
        if body.truncated {
            return "This body is longer than ProxyLab shows (\(body.size) bytes). "
                + "It will be sent unchanged."
        }
        if body.isEmpty {
            return nil
        }
        if text == nil {
            return "This body is not text (\(body.size) bytes). It will be sent unchanged."
        }

        return nil
    }
}
