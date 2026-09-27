//  Importing a cURL command, changing the request it describes, and sending it.
//
//      ┌─ paste ──────────┐      ┌─ edit ───────────────────┐
//      │ curl 'https://…' │ ──▶  │ POST  https://…          │ ──▶ replay
//      │   -H '…'         │parse │ headers  + − ×           │
//      │   -d '…'         │      │ body                     │
//      └──────────────────┘      └──────────────────────────┘
//
//  Two stages, and the second is reached only by the first succeeding. There
//  is no third: this is an editor for one request, not a workspace — no tabs,
//  no history, no saved collections, and nothing kept when the sheet closes.
//
//  # Nothing is decided here
//
//  The command is read by the core and the request is checked by the core.
//  This model does not parse cURL, does not know what a URL may be, and does
//  not know which methods exist: it holds text, sends it, and shows what came
//  back. The one check it makes is that there is something to send at all,
//  so the button is not enabled over an empty form.
//
//  That is also why a command that does not parse is not an error here. The
//  core answers `parse_curl` with either a request or a reason, and a reason
//  is shown under the field it is about.
//
//  # A body that is not text
//
//  A request body is bytes. When the bytes are valid UTF-8 they are shown and
//  edited as text, which is the usual case and the useful one. When they are
//  not — a protocol buffer, an image, a gzip stream — they are held as they
//  are and shown as a size, and the field is not editable. Offering an edit
//  that could only corrupt the body is worse than saying it cannot be edited,
//  and replaying it unchanged is the thing a developer actually wants.
//
//  # What the core will read
//
//  One IPC request is one line, and the core reads at most 64 KiB of one. An
//  oversized line is answered without a request id — the core never reads far
//  enough to know which request it was — so a client that sent one would wait
//  for an answer that cannot come. That is the one core constraint this model
//  checks itself, and it checks it exactly, by encoding the request and
//  measuring it. A body too large to replay is said so under the button
//  rather than being a request that never returns.
//
//  # Credentials
//
//  An `Authorization` header, a `Cookie`, an API key: all of them are ordinary
//  header rows here, shown and edited like any other and sent exactly as they
//  stand. Nothing here logs one, and nothing here authenticates — an expired
//  credential comes back from the server as the `401` it is.

import Foundation
import Observation

/// One header row in the editor.
///
/// The identity is the app's own, minted when the row appears, so that
/// renaming a header or having two of the same name does not make SwiftUI
/// confuse one row with another.
public struct HeaderField: Identifiable, Hashable, Sendable {
    public let id: UUID
    /// The name, as it will be sent.
    public var name: String
    /// The value, when it is text.
    public var value: String
    /// The bytes, when the value is not text.
    ///
    /// A captured header value that was not valid UTF-8 is kept exactly and
    /// the row is not editable: see the type documentation for bodies, which
    /// is the same reasoning.
    public let bytes: Data?

    public init(name: String, value: String, bytes: Data? = nil, id: UUID = UUID()) {
        self.id = id
        self.name = name
        self.value = value
        self.bytes = bytes
    }

    /// Whether the value is bytes rather than text, and so cannot be edited.
    public var isBinary: Bool { bytes != nil }

    /// The header this row describes, or `nil` for a row with no name.
    ///
    /// A row whose name is blank is a row somebody added and did not fill in,
    /// and a header with no name is not a header.
    public var header: Header? {
        let name = name.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return nil }

        if let bytes { return Header(name: name, value: .bytes(bytes)) }

        return Header(name: name, value: .text(value))
    }
}

/// Which half of the sheet is showing.
public enum CurlImportStage: Equatable, Sendable {
    /// A command is being pasted.
    case pasting
    /// The request it described is being looked at and changed.
    case editing
}

/// What the replay is doing.
public enum ReplayState: Equatable, Sendable {
    /// Nothing has been sent.
    case idle
    /// A request is in flight.
    case sending
    /// The core would not send it, and said why.
    case failed(String)
}

/// The import sheet: a command, the request it describes, and sending it.
@MainActor
@Observable
public final class CurlImportModel {
    /// The methods the editor offers in one click.
    ///
    /// A convenience and not a restriction: the field is free text, because
    /// the core sends any method that is an HTTP token and a debugging proxy
    /// that could not send `PURGE` would be missing the point.
    public static let commonMethods = [
        "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS",
    ]

    /// The command being imported.
    public var curlText = ""
    /// The method that will be sent.
    public var method = "GET"
    /// The URL that will be sent to.
    public var url = ""
    /// The headers that will be sent, in order.
    public var headers: [HeaderField] = []
    /// The body, when it is text.
    public var bodyText = ""

    /// Which half of the sheet is showing.
    public private(set) var stage: CurlImportStage = .pasting
    /// The bytes of a body that is not text, held exactly as they arrived.
    public private(set) var binaryBody: Data?
    /// What the core's parser had to say about the command.
    public private(set) var notes: [CurlNote] = []
    /// Why the command describes no request, when it describes none.
    public private(set) var problem: String?
    /// Whether the command is being read right now.
    public private(set) var isParsing = false
    /// What the replay is doing.
    public private(set) var replayState: ReplayState = .idle

    private let session: any TrafficSession

    public init(session: any TrafficSession) {
        self.session = session
    }

    // MARK: - What the sheet can do right now

    /// Whether there is a command to read.
    public var canParse: Bool {
        !curlText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !isParsing
            && fits(.parseCurl(curlText))
    }

    /// Whether there is a request to send.
    ///
    /// The whole of the app's own checking: a URL and a method that are not
    /// blank. Whether they are a URL and a method is the core's to say, and
    /// it says so under the field.
    public var canReplay: Bool {
        guard stage == .editing, replayState != .sending else { return false }

        return !url.trimmingCharacters(in: .whitespaces).isEmpty
            && !method.trimmingCharacters(in: .whitespaces).isEmpty
            && fits(.replayRequest(request))
    }

    /// Whether the request as it stands is larger than the core will read.
    ///
    /// What the interface says when the Replay button is disabled for that
    /// reason, rather than for a blank URL.
    public var isTooLargeToSend: Bool {
        stage == .editing && !fits(.replayRequest(request))
    }

    /// Whether the pasted command is larger than the core will read.
    public var isCommandTooLarge: Bool {
        !curlText.isEmpty && !fits(.parseCurl(curlText))
    }

    /// Whether `operation` fits in one IPC request line.
    private func fits(_ operation: IPCOperation) -> Bool {
        IPCRequest(requestID: "0", operation: operation).isWithinCoreLimit
    }

    /// Whether the body can be edited.
    public var isBodyEditable: Bool { binaryBody == nil }

    /// How the body is described when it cannot be shown as text.
    public var binaryBodyDescription: String? {
        guard let binaryBody else { return nil }

        return "\(binaryBody.count) bytes that are not text. They will be sent unchanged."
    }

    /// The request the editor describes.
    public var request: ReplayRequest {
        ReplayRequest(
            method: method.trimmingCharacters(in: .whitespaces),
            url: url.trimmingCharacters(in: .whitespaces),
            headers: Headers(headers.compactMap(\.header)),
            body: binaryBody ?? Data(bodyText.utf8)
        )
    }

    // MARK: - Reading a command

    /// Reads the pasted command, and moves to the editor if it describes a
    /// request.
    public func parse() async {
        guard canParse else {
            if isCommandTooLarge {
                problem = Self.tooLargeMessage
            }

            return
        }

        isParsing = true
        problem = nil
        defer { isParsing = false }

        let parsed: CurlParseResult
        do {
            parsed = try await session.parseCurl(curlText)
        } catch {
            problem = message(for: error)
            return
        }

        guard let request = parsed.request else {
            // Not a failure of the request that carried it: the core read the
            // command and the answer is that it describes none.
            problem = parsed.problem?.message ?? "That is not a cURL command ProxyLab can import."
            return
        }

        show(request, notes: parsed.notes)
    }

    /// Fills the editor in from `request`.
    public func show(_ request: ReplayRequest, notes: [CurlNote] = []) {
        method = request.method
        url = request.url
        headers = request.headers.map { header in
            switch header.value {
            case let .text(text): HeaderField(name: header.name, value: text)
            case let .bytes(bytes):
                HeaderField(name: header.name, value: "", bytes: bytes)
            }
        }

        if let text = String(data: request.body, encoding: .utf8) {
            bodyText = text
            binaryBody = nil
        } else {
            bodyText = ""
            binaryBody = request.body
        }

        self.notes = notes
        problem = nil
        replayState = .idle
        stage = .editing
    }

    /// Goes back to the pasted command, keeping it as it was typed.
    public func backToCommand() {
        stage = .pasting
        replayState = .idle
    }

    // MARK: - Editing

    /// Adds an empty header row at the end.
    public func addHeader() {
        headers.append(HeaderField(name: "", value: ""))
    }

    /// Removes the header row with `id`.
    public func removeHeader(_ id: HeaderField.ID) {
        headers.removeAll { $0.id == id }
    }

    /// Replaces the body with text, whatever it was before.
    ///
    /// What "edit this anyway" does to a body that is not text: the bytes are
    /// let go of deliberately, by somebody who asked, rather than quietly on
    /// the way past.
    public func makeBodyEditable() {
        binaryBody = nil
        bodyText = ""
    }

    // MARK: - Sending it

    /// Sends the request the editor describes.
    ///
    /// Gives back what came of it, or `nil` when the core would not send it —
    /// in which case ``replayState`` says why. A request that was sent and
    /// failed is not `nil`: it is a result carrying the failure, because the
    /// core captured an exchange for it either way.
    @discardableResult
    public func replay() async -> ReplayResult? {
        guard canReplay else {
            if isTooLargeToSend {
                replayState = .failed(Self.tooLargeMessage)
            }

            return nil
        }

        replayState = .sending

        do {
            let result = try await session.replay(request)
            replayState = .idle

            return result
        } catch {
            replayState = .failed(message(for: error))

            return nil
        }
    }

    /// What a person is told about a request the core will not read.
    static let tooLargeMessage =
        "This is larger than the ProxyLab core will read in one request "
        + "(64 KB, including the body). Replay a smaller body."

    /// What a person is told about `error`.
    private func message(for error: any Error) -> String {
        (error as? IPCError)?.message ?? "Something went wrong talking to the ProxyLab core."
    }
}
