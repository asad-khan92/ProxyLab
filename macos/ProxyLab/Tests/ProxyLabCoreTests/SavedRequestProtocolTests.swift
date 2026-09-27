//  The saved-request half of the wire protocol, in both directions, and the
//  two views that draw one.
//
//  What a request must carry, what a response must be read as, and that each
//  new view lays out at all. The shapes here are the ones `proxy-storage`
//  serializes and `core/ipc` parses: a flattened request on the way in, a body
//  that is a capture rather than plain bytes on the way back.

import Foundation
import SwiftUI
import Testing

@testable import ProxyLabCore

@Suite("Saved request encoding")
struct SavedRequestEncodingTests {
    private let encoder = JSONEncoder()

    private func encoded(_ operation: IPCOperation) throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: "1", operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("the operation names are the ones the core implements")
    func names() {
        #expect(IPCOperation.listSavedRequests.name == "list_saved_requests")
        #expect(IPCOperation.getSavedRequest(savedRequestID: "x").name == "get_saved_request")
        #expect(
            IPCOperation.createSavedRequest(NewSavedRequest(name: "n", method: "GET", url: "u"))
                .name == "create_saved_request"
        )
        #expect(
            IPCOperation.saveRequestFromTraffic(exchangeID: "e", name: "n").name
                == "save_request_from_traffic"
        )
        #expect(
            IPCOperation.updateSavedRequest(savedRequestID: "x", SavedRequestUpdate()).name
                == "update_saved_request"
        )
        #expect(
            IPCOperation.deleteSavedRequest(savedRequestID: "x").name == "delete_saved_request"
        )
    }

    @Test("listing takes no parameters, so none are sent")
    func listTakesNothing() throws {
        #expect(try Set(encoded(.listSavedRequests).keys) == ["version", "request_id", "type"])
    }

    @Test("a request naming one saved request sends its id and nothing else")
    func byID() throws {
        for operation in [
            IPCOperation.getSavedRequest(savedRequestID: "s1"),
            .deleteSavedRequest(savedRequestID: "s1"),
        ] {
            let json = try encoded(operation)

            #expect(
                Set(json.keys) == ["version", "request_id", "type", "saved_request_id"]
            )
            #expect(json["saved_request_id"] as? String == "s1")
        }
    }

    @Test("a saved request is sent flattened, with its body as base64")
    func createFlattened() throws {
        let json = try encoded(
            .createSavedRequest(
                NewSavedRequest(
                    name: "Create a user",
                    method: "POST",
                    url: "https://example.com/users",
                    headers: Headers([
                        Header(name: "Content-Type", value: .text("application/json"))
                    ]),
                    body: Data(#"{"name":"ada"}"#.utf8)
                )
            )
        )

        #expect(
            Set(json.keys)
                == ["version", "request_id", "type", "name", "method", "url", "headers", "body"]
        )
        #expect(json["name"] as? String == "Create a user")
        #expect(json["body"] as? String == "eyJuYW1lIjoiYWRhIn0=")
    }

    @Test("saving from traffic sends the exchange and the name, and no request")
    func fromTraffic() throws {
        // What the request *was* is the core's to read out of its own store,
        // which is why `curl_from_traffic` carries an id and not a request.
        let json = try encoded(.saveRequestFromTraffic(exchangeID: "e1", name: "The order call"))

        #expect(
            Set(json.keys) == ["version", "request_id", "type", "exchange_id", "name"]
        )
        #expect(json["exchange_id"] as? String == "e1")
    }

    @Test("an update sends only the fields it changes")
    func updateIsSparse() throws {
        let json = try encoded(.updateSavedRequest(savedRequestID: "s1", .rename("New name")))

        #expect(
            Set(json.keys) == ["version", "request_id", "type", "saved_request_id", "name"]
        )
        #expect(json["name"] as? String == "New name")
    }

    @Test("an update that replaces the request sends every part of it")
    func updateWholeRequest() throws {
        let json = try encoded(
            .updateSavedRequest(
                savedRequestID: "s1",
                .request(
                    ReplayRequest(
                        method: "PUT",
                        url: "https://example.com/users/1",
                        body: Data("{}".utf8)
                    )
                )
            )
        )

        #expect(
            Set(json.keys)
                == [
                    "version", "request_id", "type", "saved_request_id", "method", "url",
                    "headers", "body",
                ]
        )
        #expect(json["body"] as? String == "e30=")
    }

    @Test("a saved request is read back with its body as the capture it is")
    func decoding() throws {
        let line = """
            {"version":1,"request_id":"1","ok":true,"type":"get_saved_request","result":{
              "saved_request":{
                "id":"0199c0de-0000-7000-8000-0000000000aa",
                "name":"Create a user",
                "method":"POST",
                "url":"https://example.com/users",
                "headers":[{"name":"Authorization","value":{"text":"Bearer sk-secret"}}],
                "body":{"data":"eyJuYW1lIjoiYWRhIn0=","size":14,"truncated":false},
                "created_at_ms":1700000000000,
                "updated_at_ms":1700000000001
              }}}
            """

        guard case let .response(response) = try JSONDecoder().decode(
            IPCMessage.self,
            from: Data(line.utf8)
        ),
            case let .getSavedRequest(saved) = try #require(response.result)
        else {
            Issue.record("not a saved request")
            return
        }

        #expect(saved.id == "0199c0de-0000-7000-8000-0000000000aa")
        #expect(saved.name == "Create a user")
        #expect(saved.headers.text("authorization") == "Bearer sk-secret")
        #expect(saved.body.data == Data(#"{"name":"ada"}"#.utf8))
        #expect(saved.body.size == 14)
        #expect(!saved.sendsLessThanItWas)
        #expect(saved.createdAtMs == 1_700_000_000_000)
        #expect(saved.updatedAtMs == 1_700_000_000_001)
    }

    @Test("a truncated capture is read back as one that sends less")
    func decodingTruncated() throws {
        let line = """
            {"version":1,"request_id":"1","ok":true,"type":"save_request_from_traffic","result":{
              "saved_request":{"id":"s1","name":"A big upload","method":"POST",
                "url":"https://example.com/upload","headers":[],
                "body":{"data":"dGhlIGZpcnN0IGJpdA==","size":5000000,"truncated":true},
                "created_at_ms":1,"updated_at_ms":1}}}
            """

        guard case let .response(response) = try JSONDecoder().decode(
            IPCMessage.self,
            from: Data(line.utf8)
        ),
            case let .saveRequestFromTraffic(saved) = try #require(response.result)
        else {
            Issue.record("not a saved request")
            return
        }

        #expect(saved.sendsLessThanItWas)
        #expect(saved.body.size == 5_000_000)
        #expect(saved.replayRequest.body == Data("the first bit".utf8))
    }

    @Test("a list and a delete are read as what they are")
    func decodingListAndDelete() throws {
        let list = """
            {"version":1,"request_id":"1","ok":true,"type":"list_saved_requests",
             "result":{"saved_requests":[]}}
            """
        let deleted = """
            {"version":1,"request_id":"2","ok":true,"type":"delete_saved_request",
             "result":{"saved_request_id":"s1","deleted":true}}
            """

        let decoder = JSONDecoder()
        guard case let .response(first) = try decoder.decode(
            IPCMessage.self,
            from: Data(list.utf8)
        ),
            case let .listSavedRequests(page) = try #require(first.result)
        else {
            Issue.record("not a list")
            return
        }
        #expect(page.savedRequests.isEmpty)

        guard case let .response(second) = try decoder.decode(
            IPCMessage.self,
            from: Data(deleted.utf8)
        ),
            case let .deleteSavedRequest(result) = try #require(second.result)
        else {
            Issue.record("not a delete")
            return
        }
        #expect(result.savedRequestID == "s1")
        #expect(result.deleted)
    }

    @Test("the core's refusals are codes the app can match on")
    func errorCodes() throws {
        for (raw, expected) in [
            ("invalid_saved_request", IPCErrorCode.invalidSavedRequest),
            ("invalid_saved_request_id", .invalidSavedRequestID),
        ] {
            let line = """
                {"version":1,"request_id":"1","ok":false,
                 "error":{"code":"\(raw)","message":"no"}}
                """

            guard case let .response(response) = try JSONDecoder().decode(
                IPCMessage.self,
                from: Data(line.utf8)
            ) else {
                Issue.record("not a response")
                return
            }

            #expect(response.error?.code == expected)
        }
    }
}

@MainActor
@Suite("Saved request views")
struct SavedRequestViewRenderingTests {
    /// Lays `view` out offscreen and gives back the size it came out at.
    ///
    /// A body that traps or lays out to nothing fails when it is drawn, and
    /// drawing it needs no window and nobody watching. Smoke tests,
    /// deliberately not snapshot comparisons.
    private func rendered(_ view: some View, width: CGFloat = 560) -> CGSize? {
        let renderer = ImageRenderer(content: view.frame(width: width))
        renderer.scale = 1

        return renderer.nsImage?.size
    }

    private func model(_ requests: [SavedRequest] = []) async -> SavedRequestsModel {
        let session = FakeSavedRequestsSession()
        await session.answerList(with: requests)
        let model = SavedRequestsModel(session: session, traffic: FakeSession())
        await model.reload()

        return model
    }

    @Test("the section draws empty, with rows, and while renaming one")
    func sectionDraws() async throws {
        let empty = await model()
        #expect(
            try #require(rendered(
                SavedRequestsSectionView(model: empty, coreConnected: true),
                width: 230
            )).height > 0
        )

        let filled = await model([
            SavedRequestFixtures.request(id: "s1"),
            SavedRequestFixtures.truncated,
        ])
        #expect(
            try #require(rendered(
                SavedRequestsSectionView(model: filled, coreConnected: true),
                width: 230
            )).height > 0
        )

        filled.beginRename(SavedRequestFixtures.request(id: "s1"))
        #expect(
            try #require(rendered(
                SavedRequestsSectionView(model: filled, coreConnected: true),
                width: 230
            )).height > 0
        )
    }

    @Test("the section draws what it says when the core is not answering")
    func sectionDrawsDisconnected() async throws {
        let session = FakeSavedRequestsSession()
        await session.answerLists(with: [.failure(.notConnected)])
        let model = SavedRequestsModel(session: session, traffic: FakeSession())
        await model.reload()

        let size = try #require(
            rendered(SavedRequestsSectionView(model: model, coreConnected: false), width: 230)
        )
        #expect(size.height > 0)
    }

    @Test("the editor draws, with a truncated body and with a failure on it")
    func editorDraws() async throws {
        let model = await model()
        await model.open(SavedRequestFixtures.truncated)
        let editor = try #require(model.editor)

        #expect(try #require(rendered(SavedRequestSheet(editor: editor, model: model))).height > 0)

        editor.request.show(
            ReplayRequest(
                method: "POST",
                url: "https://example.com/upload",
                body: Data([0xff, 0xfe, 0x00])
            )
        )
        #expect(try #require(rendered(SavedRequestSheet(editor: editor, model: model))).height > 0)
    }

    @Test("the naming sheet draws")
    func promptDraws() async throws {
        let model = await model()
        model.beginSave(exchange: "e1", suggestedName: "GET /v1/users")
        var prompt = try #require(model.saving)

        let size = try #require(
            rendered(
                SaveRequestSheet(model: model, prompt: Binding(get: { prompt }, set: { prompt = $0 })),
                width: 420
            )
        )
        #expect(size.height > 0)
    }
}
