//  A request the app wants sent, and what the core has to say about a cURL
//  command.
//
//      curl '…' ──▶ parse_curl ──▶ ReplayRequest ──▶ replay_request ──▶ a new
//                                       ▲                               exchange
//                                       │
//                             the editor, where it can be changed
//
//  These are Swift mirrors of `proxy-http`'s `RequestParts` and `proxy-curl`'s
//  own result types, field for field and name for name — the same rule the
//  traffic model follows. The app never decides what a URL, a method, or a
//  header may be: it sends what it has and the core refuses what it will not
//  send, which is why this is a plain value type with no validation in it.
//
//  # The body is bytes
//
//  `Data`, not `String`. A request body is quite often a protocol buffer, a
//  gzip stream, or an image, and a body that has been through a lossy
//  conversion to text is not the body that was sent. `JSONEncoder` writes
//  `Data` as base64, which is what the core reads it as.
//
//  # Credentials
//
//  A replayed request carries whatever the original carried: `Authorization`,
//  `Cookie`, an API key in a header of a service's own naming. They are kept
//  exactly, because a replay that dropped the credential would be a replay of
//  a different request — and nothing here writes one to a log.

import Foundation

/// A request to send, as the core reads it.
public struct ReplayRequest: Codable, Hashable, Sendable {
    /// The method, as it will be sent.
    public var method: String
    /// The absolute URL to send it to.
    public var url: String
    /// The headers, in the order they should be sent.
    public var headers: Headers
    /// The body, which is bytes and need not be text.
    public var body: Data

    public init(method: String, url: String, headers: Headers = Headers(), body: Data = Data()) {
        self.method = method
        self.url = url
        self.headers = headers
        self.body = body
    }
}

// MARK: - Importing a cURL command

/// Something the core's parser did that is worth telling a person about.
///
/// A wrapper around the name rather than a closed enum, for the same reason
/// ``IPCErrorCode`` is one: a note this build has not heard of is still a note
/// worth showing, and is a poor reason to fail the whole answer.
public struct CurlNote: RawRepresentable, Codable, Hashable, Sendable {
    public let rawValue: String

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    /// `--insecure` was accepted and not applied.
    public static let insecure = CurlNote(rawValue: "insecure")
    /// `--compressed` became an `Accept-Encoding` header.
    public static let compressed = CurlNote(rawValue: "compressed")
    /// Options that only change cURL's own output were ignored.
    public static let outputOnly = CurlNote(rawValue: "output_only")
    /// cURL's own `Content-Type` for `--data` was added.
    public static let formContentType = CurlNote(rawValue: "form_content_type")
    /// Credentials in the URL were moved into an `Authorization` header.
    public static let credentialsMoved = CurlNote(rawValue: "credentials_moved")

    /// What a person is told.
    ///
    /// Written here rather than sent by the core, because it is interface
    /// text: the core sends a name, and a name a build does not know is shown
    /// as nothing rather than as a raw identifier.
    public var message: String? {
        switch self {
        case .insecure:
            "--insecure was not applied. ProxyLab always verifies the destination server's "
                + "certificate."
        case .compressed:
            "--compressed was imported as an Accept-Encoding header. The response is captured "
                + "as it arrives and is not decompressed."
        case .outputOnly:
            "Options that only change what cURL prints were ignored. They do not change the "
                + "request."
        case .formContentType:
            "Content-Type: application/x-www-form-urlencoded was added, which is what cURL "
                + "sends for --data."
        case .credentialsMoved:
            "Credentials in the URL were moved into an Authorization header, which is what "
                + "cURL sends."
        default:
            nil
        }
    }
}

/// What kind of problem a cURL command has.
///
/// A wrapper around the name, like every other code that crosses the boundary.
public struct CurlProblemCode: RawRepresentable, Codable, Hashable, Sendable {
    public let rawValue: String

    public init(rawValue: String) {
        self.rawValue = rawValue
    }

    public static let empty = CurlProblemCode(rawValue: "empty")
    public static let notCurl = CurlProblemCode(rawValue: "not_curl")
    public static let unterminatedQuote = CurlProblemCode(rawValue: "unterminated_quote")
    public static let shellExpansion = CurlProblemCode(rawValue: "shell_expansion")
    public static let shellMetacharacter = CurlProblemCode(rawValue: "shell_metacharacter")
    public static let invalidEscape = CurlProblemCode(rawValue: "invalid_escape")
    public static let unsupportedOption = CurlProblemCode(rawValue: "unsupported_option")
    public static let proxyOption = CurlProblemCode(rawValue: "proxy_option")
    public static let missingOptionValue = CurlProblemCode(rawValue: "missing_option_value")
    public static let multipleURLs = CurlProblemCode(rawValue: "multiple_urls")
    public static let missingURL = CurlProblemCode(rawValue: "missing_url")
    public static let invalidHeader = CurlProblemCode(rawValue: "invalid_header")
    public static let fileReference = CurlProblemCode(rawValue: "file_reference")
    public static let invalidUser = CurlProblemCode(rawValue: "invalid_user")
    public static let notText = CurlProblemCode(rawValue: "not_text")
    public static let invalidRequest = CurlProblemCode(rawValue: "invalid_request")
}

/// Why a cURL command describes no request.
public struct CurlProblem: Codable, Hashable, Sendable {
    /// What kind of problem it is, for the app to match on.
    public let code: CurlProblemCode
    /// What it is, in the core's own words. Safe to show: the core writes
    /// every one of these, and the only thing out of the command that is ever
    /// in one is the name of an option.
    public let message: String
    /// The option it is about, when it is about one.
    public let option: String?

    public init(code: CurlProblemCode, message: String, option: String? = nil) {
        self.code = code
        self.message = message
        self.option = option
    }
}

/// What a cURL command came to.
///
/// A command that does not parse is a *result*, not a failure: the core was
/// asked to read a command and the answer is that it describes no request. So
/// there is a request or a problem, never both and never neither.
public struct CurlParseResult: Codable, Hashable, Sendable {
    /// The request the command describes, or `nil` when it describes none.
    public let request: ReplayRequest?
    /// What the core's parser has to say about a command it did read.
    public let notes: [CurlNote]
    /// Why there is no request, when there is none.
    public let problem: CurlProblem?

    public init(request: ReplayRequest?, notes: [CurlNote] = [], problem: CurlProblem? = nil) {
        self.request = request
        self.notes = notes
        self.problem = problem
    }

    private enum CodingKeys: String, CodingKey {
        case request
        case notes
        case problem
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        request = try container.decodeIfPresent(ReplayRequest.self, forKey: .request)
        notes = try container.decodeIfPresent([CurlNote].self, forKey: .notes) ?? []
        problem = try container.decodeIfPresent(CurlProblem.self, forKey: .problem)
    }
}

// MARK: - Copying a stored request out

/// A stored request, written out as the cURL command that would send it again.
public struct CurlGenerated: Codable, Hashable, Sendable {
    /// The command.
    public let curl: String
    /// Whether the captured body it was written from was truncated, so the
    /// command sends less than the original request did.
    public let truncated: Bool

    public init(curl: String, truncated: Bool) {
        self.curl = curl
        self.truncated = truncated
    }
}

// MARK: - Sending one

/// What a replay produced.
///
/// Thin on purpose. Everything else about the replay is in the exchange the
/// core captured for it, which arrives on the traffic event stream and is in
/// the traffic list like any other — so the id here is the id of a row the
/// app already has, or is about to have.
public struct ReplayResult: Codable, Hashable, Sendable {
    /// The exchange the replay was captured as. Always a new one.
    public let exchangeID: String
    /// The status the destination answered with, if it answered.
    public let status: UInt16?
    /// Why there is no status, when there is none.
    public let error: TrafficError?

    public init(exchangeID: String, status: UInt16?, error: TrafficError?) {
        self.exchangeID = exchangeID
        self.status = status
        self.error = error
    }

    /// Whether the destination answered.
    public var succeeded: Bool { status != nil && error == nil }

    private enum CodingKeys: String, CodingKey {
        case exchangeID = "exchange_id"
        case status
        case error
    }
}
