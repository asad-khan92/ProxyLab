//  A core that answers saved-request requests with whatever a test told it to.
//
//  The model's job is what it does with an answer — what it shows, what it
//  sends next, what it refuses to change until the core agrees — and none of
//  that needs a socket. Every call returns what it was configured to return and
//  records that it happened, so a test asserts on the requests that were made
//  rather than on a delay.

import Foundation

@testable import ProxyLabCore

/// One call a test can assert was made.
enum SavedRequestCall: Equatable {
    case list
    case get(String)
    case create(NewSavedRequest)
    case fromTraffic(exchangeID: String, name: String)
    case update(String, SavedRequestUpdate)
    case delete(String)
}

/// A `SavedRequestsSession` whose answers are set by the test.
actor FakeSavedRequestsSession: SavedRequestsSession {
    /// Every call made, in order.
    private(set) var calls: [SavedRequestCall] = []

    private var lists: [Result<[SavedRequest], IPCError>] = [.success([])]
    private var getResult: Result<SavedRequest, IPCError>?
    private var createResult: Result<SavedRequest, IPCError>?
    private var fromTrafficResult: Result<SavedRequest, IPCError>?
    private var updateResult: Result<SavedRequest, IPCError>?
    private var deleteResult: Result<SavedRequestDeletedResult, IPCError>?

    // MARK: - What the test sets

    /// The answers to successive `listSavedRequests` calls, in order.
    ///
    /// The last one is repeated once they run out, so a test that only cares
    /// about the first list sets one.
    func answerLists(with answers: [Result<[SavedRequest], IPCError>]) { lists = answers }

    /// The answer to every `listSavedRequests` call.
    func answerList(with requests: [SavedRequest]) { lists = [.success(requests)] }

    func answerGet(with answer: Result<SavedRequest, IPCError>) { getResult = answer }
    func answerCreate(with answer: Result<SavedRequest, IPCError>) { createResult = answer }
    func answerFromTraffic(with answer: Result<SavedRequest, IPCError>) {
        fromTrafficResult = answer
    }
    func answerUpdate(with answer: Result<SavedRequest, IPCError>) { updateResult = answer }
    func answerDelete(with answer: Result<SavedRequestDeletedResult, IPCError>) {
        deleteResult = answer
    }

    // MARK: - SavedRequestsSession

    func listSavedRequests() async throws -> [SavedRequest] {
        calls.append(.list)

        guard !lists.isEmpty else { return [] }

        return try (lists.count == 1 ? lists[0] : lists.removeFirst()).get()
    }

    func savedRequest(id: String) async throws -> SavedRequest {
        calls.append(.get(id))

        return try answer(getResult, id: id)
    }

    func createSavedRequest(_ request: NewSavedRequest) async throws -> SavedRequest {
        calls.append(.create(request))

        return try answer(createResult, id: "created")
    }

    func saveRequest(fromExchange id: String, name: String) async throws -> SavedRequest {
        calls.append(.fromTraffic(exchangeID: id, name: name))

        return try answer(fromTrafficResult, id: "saved")
    }

    func updateSavedRequest(id: String, _ update: SavedRequestUpdate) async throws -> SavedRequest {
        calls.append(.update(id, update))

        return try answer(updateResult, id: id)
    }

    func deleteSavedRequest(id: String) async throws -> SavedRequestDeletedResult {
        calls.append(.delete(id))

        guard let deleteResult else {
            return SavedRequestDeletedResult(savedRequestID: id, deleted: true)
        }

        return try deleteResult.get()
    }

    /// The configured answer, or a plausible one under `id`.
    private func answer(
        _ result: Result<SavedRequest, IPCError>?,
        id: String
    ) throws -> SavedRequest {
        guard let result else { return SavedRequestFixtures.request(id: id) }

        return try result.get()
    }
}

/// Saved requests the tests recognise.
enum SavedRequestFixtures {
    /// A `POST` with a credential and a JSON body, which is the shape that
    /// matters: a saved request keeps what the request carried.
    static func request(
        id: String = "0199c0de-0000-7000-8000-0000000000aa",
        name: String = "Create a user",
        method: String = "POST",
        url: String = "https://example.com/users",
        body: BodyCapture = .complete(Data(#"{"name":"ada"}"#.utf8))
    ) -> SavedRequest {
        SavedRequest(
            id: id,
            name: name,
            method: method,
            url: url,
            headers: Headers([
                Header(name: "Content-Type", value: .text("application/json")),
                Header(name: "Authorization", value: .text("Bearer sk-secret")),
            ]),
            body: body,
            createdAtMs: 1_700_000_000_000,
            updatedAtMs: 1_700_000_000_000
        )
    }

    /// One whose captured body the core's limit cut short.
    static var truncated: SavedRequest {
        request(
            id: "0199c0de-0000-7000-8000-0000000000bb",
            name: "A big upload",
            body: BodyCapture(data: Data("the first bit".utf8), size: 5_000_000, truncated: true)
        )
    }
}

/// A file chooser that answers with whatever a test told it to.
///
/// So a test can say what path would have been chosen without a panel that
/// needs a person to click it, and without a test run leaving files on a
/// developer's disk.
@MainActor
final class FakeHarFileChooser: HarFileChooser {
    /// What `chooseExportPath` gives back. `nil` is a cancelled panel.
    var exportPath: String?
    /// What `chooseImportPath` gives back.
    var importPath: String?

    /// The names that were suggested for an export, in order.
    private(set) var suggestedNames: [String] = []
    /// How many times an import location was asked for.
    private(set) var importPrompts = 0

    init(exportPath: String? = nil, importPath: String? = nil) {
        self.exportPath = exportPath
        self.importPath = importPath
    }

    func chooseExportPath(suggestedName: String) -> String? {
        suggestedNames.append(suggestedName)

        return exportPath
    }

    func chooseImportPath() -> String? {
        importPrompts += 1

        return importPath
    }
}
