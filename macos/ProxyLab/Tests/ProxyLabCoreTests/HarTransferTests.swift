//  Exporting the traffic as HAR, importing a HAR file, and the wire both
//  travel on.
//
//  The app never writes or parses a HAR document: it asks macOS for a path and
//  the core does the rest. So what is under test here is the boundary — which
//  request was sent, what was chosen, and what is said afterwards — and the
//  format itself is tested in `proxy-har`, in Rust, with no socket.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("HAR requests on the wire")
struct HarRequestEncodingTests {
    private let encoder = JSONEncoder()

    private func encoded(_ operation: IPCOperation) throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: "1", operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("the operation names are the ones the core implements")
    func names() {
        #expect(
            IPCOperation.exportHar(HarExportRequest(path: "/x.har", selection: .exchanges([])))
                .name == "export_har"
        )
        #expect(IPCOperation.importHar(path: "/x.har").name == "import_har")
    }

    @Test("an export of a selection sends the path and the ids and nothing else")
    func selection() throws {
        let json = try encoded(
            .exportHar(HarExportRequest(path: "/tmp/x.har", selection: .exchanges(["a", "b"])))
        )

        #expect(Set(json.keys) == ["version", "request_id", "type", "path", "exchange_ids"])
        #expect(json["path"] as? String == "/tmp/x.har")
        #expect(json["exchange_ids"] as? [String] == ["a", "b"])
    }

    @Test("an export of everything sends the path alone")
    func everything() throws {
        // No filters set is the whole history, and the core refuses a
        // parameter it does not know — including a key whose value is null.
        let json = try encoded(
            .exportHar(
                HarExportRequest(path: "/tmp/x.har", selection: .filtered(ListTrafficParameters()))
            )
        )

        #expect(Set(json.keys) == ["version", "request_id", "type", "path"])
    }

    @Test("a filtered export sends the filters and never a limit, cursor, or order")
    func filtered() throws {
        // An export is every match rather than a page of them, and it is
        // written in the order the traffic happened whichever end of the list
        // is being read.
        var filters = TrafficFilters.none
        filters.host = "example.com"
        filters.method = "POST"
        filters.search = "orders"
        filters.sort = .oldestFirst

        let json = try encoded(
            .exportHar(
                HarExportRequest(
                    path: "/tmp/x.har",
                    selection: .filtered(filters.exportParameters)
                )
            )
        )

        #expect(
            Set(json.keys)
                == ["version", "request_id", "type", "path", "host", "method", "search"]
        )
        #expect(json["host"] as? String == "example.com")
    }

    @Test("an export never sends ids and filters at once")
    func neverBoth() throws {
        // The core refuses a request that names both, because either reading
        // of it would export something nobody asked for.
        let json = try encoded(
            .exportHar(HarExportRequest(path: "/tmp/x.har", selection: .exchanges(["a"])))
        )

        #expect(json["host"] == nil)
        #expect(json["search"] == nil)
    }

    @Test("an import sends one parameter, the path")
    func importing() throws {
        let json = try encoded(.importHar(path: "/tmp/theirs.har"))

        #expect(Set(json.keys) == ["version", "request_id", "type", "path"])
        #expect(json["path"] as? String == "/tmp/theirs.har")
    }

    @Test("the results are read as the core writes them")
    func results() throws {
        let export = """
            {"version":1,"request_id":"1","ok":true,"type":"export_har",
             "result":{"entries":12,"bytes":4096,"truncated":false}}
            """
        let imported = """
            {"version":1,"request_id":"2","ok":true,"type":"import_har",
             "result":{"imported":9,"skipped":1}}
            """

        let decoder = JSONDecoder()
        guard case let .response(first) = try decoder.decode(
            IPCMessage.self,
            from: Data(export.utf8)
        ),
            case let .exportHar(result) = try #require(first.result)
        else {
            Issue.record("not an export result")
            return
        }
        #expect(result.entries == 12)
        #expect(result.bytes == 4_096)
        #expect(!result.truncated)

        guard case let .response(second) = try decoder.decode(
            IPCMessage.self,
            from: Data(imported.utf8)
        ),
            case let .importHar(result) = try #require(second.result)
        else {
            Issue.record("not an import result")
            return
        }
        #expect(result.imported == 9)
        #expect(result.skipped == 1)
    }

    @Test("the core's HAR refusal is a code the app can match on")
    func errorCode() throws {
        let line = """
            {"version":1,"request_id":"1","ok":false,
             "error":{"code":"har_unavailable","message":"the HAR file could not be written"}}
            """

        guard case let .response(response) = try JSONDecoder().decode(
            IPCMessage.self,
            from: Data(line.utf8)
        ) else {
            Issue.record("not a response")
            return
        }

        #expect(response.error?.code == .harUnavailable)
    }
}

@Suite("HAR in the traffic inspector")
@MainActor
struct HarTrafficModelTests {
    private func model(
        _ session: FakeSession,
        chooser: FakeHarFileChooser
    ) -> TrafficInspectorModel {
        let model = TrafficInspectorModel(
            session: session,
            socketPath: "/tmp/proxylab.sock",
            clipboard: FakeClipboard(),
            reconnect: .manual
        )
        model.harFiles = chooser

        return model
    }

    @Test("exporting the selection sends the selected exchange's id")
    func exportSelected() async {
        let session = FakeSession()
        await session.answerLists(
            with: [.success(Fixtures.page(["e1", "e2"].map { Fixtures.summary(id: $0) }))]
        )
        let chooser = FakeHarFileChooser(exportPath: "/tmp/one.har")
        let model = model(session, chooser: chooser)
        await model.loadFirstPage()
        await model.select("e2")

        await model.exportSelectedAsHar()

        let exports = await session.exports
        #expect(exports.count == 1)
        #expect(exports.first?.path == "/tmp/one.har")
        #expect(exports.first?.selection == .exchanges(["e2"]))
    }

    @Test("exporting with nothing selected sends nothing")
    func exportWithoutSelection() async {
        let session = FakeSession()
        let chooser = FakeHarFileChooser(exportPath: "/tmp/one.har")
        let model = model(session, chooser: chooser)

        await model.exportSelectedAsHar()

        #expect(await session.exports.isEmpty)
    }

    @Test("exporting the list sends the filters the list is showing")
    func exportFiltered() async {
        let session = FakeSession()
        let chooser = FakeHarFileChooser(exportPath: "/tmp/all.har")
        let model = model(session, chooser: chooser)
        await model.setHost("api.example.com")

        await model.exportFilteredAsHar()

        let exports = await session.exports
        guard case let .filtered(filters) = try! #require(exports.first?.selection) else {
            Issue.record("not a filtered export")
            return
        }
        #expect(filters.host == "api.example.com")
        #expect(filters.limit == nil)
        #expect(filters.cursor == nil)
        #expect(filters.order == nil)
    }

    @Test("the suggested file name names the host the list is narrowed to")
    func suggestedName() async {
        let session = FakeSession()
        let chooser = FakeHarFileChooser(exportPath: "/tmp/all.har")
        let model = model(session, chooser: chooser)
        await model.setHost("api.example.com")

        await model.exportFilteredAsHar()

        #expect(chooser.suggestedNames.first?.hasPrefix("api.example.com") == true)
        #expect(chooser.suggestedNames.first?.hasSuffix(".har") == true)
    }

    @Test("a cancelled panel sends nothing and says nothing")
    func cancelledExport() async {
        // Cancelling is an answer, not a failure.
        let session = FakeSession()
        let chooser = FakeHarFileChooser(exportPath: nil)
        let model = model(session, chooser: chooser)
        await model.setHost("example.com")

        await model.exportFilteredAsHar()

        #expect(await session.exports.isEmpty)
        #expect(model.lastError == nil)
        #expect(model.lastAction == nil)
    }

    @Test("an export that happened is reported with its count and its size")
    func exportReported() async {
        let session = FakeSession()
        await session.answerExport(
            with: .success(HarExportResult(entries: 12, bytes: 4_096, truncated: false))
        )
        let chooser = FakeHarFileChooser(exportPath: "/tmp/all.har")
        let model = model(session, chooser: chooser)

        await model.exportFilteredAsHar()

        let action = try! #require(model.lastAction)
        #expect(action.contains("12 exchanges"))
        #expect(action.contains("/tmp/all.har"))
        #expect(model.lastError == nil)
    }

    @Test("an export that hit the limit says the file has some of the history")
    func exportTruncated() async {
        let session = FakeSession()
        await session.answerExport(
            with: .success(HarExportResult(entries: 10_000, bytes: 1, truncated: true))
        )
        let chooser = FakeHarFileChooser(exportPath: "/tmp/all.har")
        let model = model(session, chooser: chooser)

        await model.exportFilteredAsHar()

        #expect(model.lastAction?.contains("more matching") == true)
    }

    @Test("an export the core refused is shown and nothing is claimed")
    func exportFails() async {
        let session = FakeSession()
        await session.answerExport(
            with: .failure(
                .server(IPCErrorBody(code: .harUnavailable, message: "that path cannot be used"))
            )
        )
        let chooser = FakeHarFileChooser(exportPath: "/tmp/nowhere/all.har")
        let model = model(session, chooser: chooser)

        await model.exportFilteredAsHar()

        #expect(model.lastError == "that path cannot be used")
        #expect(model.lastAction == nil)
    }

    @Test("importing sends the chosen path and reloads the list")
    func importing() async {
        // Imported exchanges are not published on the traffic stream — that
        // stream carries the exchanges the proxy finished — so the list is
        // asked for again.
        let session = FakeSession()
        await session.answerLists(with: [
            .success(Fixtures.page([])),
            .success(Fixtures.page(["i1", "i2"].map { Fixtures.summary(id: $0) })),
        ])
        let chooser = FakeHarFileChooser(importPath: "/tmp/theirs.har")
        let model = model(session, chooser: chooser)
        await model.loadFirstPage()

        await model.importHar()

        #expect(await session.imports == ["/tmp/theirs.har"])
        #expect(model.exchanges.map(\.id) == ["i1", "i2"])
    }

    @Test("an import says how many arrived, and that nothing was sent")
    func importReported() async {
        let session = FakeSession()
        await session.answerImport(with: .success(HarImportResult(imported: 9, skipped: 0)))
        let chooser = FakeHarFileChooser(importPath: "/tmp/theirs.har")
        let model = model(session, chooser: chooser)

        await model.importHar()

        let action = try! #require(model.lastAction)
        #expect(action.contains("9 exchanges"))
        #expect(action.contains("Nothing in the file was sent"))
    }

    @Test("entries the core could not read are counted in what is said")
    func importSkipped() async {
        let session = FakeSession()
        await session.answerImport(with: .success(HarImportResult(imported: 9, skipped: 3)))
        let chooser = FakeHarFileChooser(importPath: "/tmp/theirs.har")
        let model = model(session, chooser: chooser)

        await model.importHar()

        #expect(model.lastAction?.contains("3 entries") == true)
    }

    @Test("a cancelled open panel reads nothing")
    func cancelledImport() async {
        let session = FakeSession()
        let chooser = FakeHarFileChooser(importPath: nil)
        let model = model(session, chooser: chooser)

        await model.importHar()

        #expect(chooser.importPrompts == 1)
        #expect(await session.imports.isEmpty)
        #expect(model.lastError == nil)
    }

    @Test("an import the core refused is shown without naming the file")
    func importFails() async {
        let session = FakeSession()
        await session.answerImport(
            with: .failure(
                .server(
                    IPCErrorBody(
                        code: .harUnavailable,
                        message: "the HAR file could not be written or read"
                    )
                )
            )
        )
        let chooser = FakeHarFileChooser(importPath: "/tmp/notes.har")
        let model = model(session, chooser: chooser)

        await model.importHar()

        let error = try! #require(model.lastError)
        #expect(error == "the HAR file could not be written or read")
        #expect(!error.contains("notes.har"))
    }

    @Test("a name for a saved request is the method and the path")
    func saveName() async {
        let session = FakeSession()
        await session.answerLists(with: [.success(Fixtures.page([Fixtures.summary(id: "e1")]))])
        await session.answerExchange("e1", with: .success(Fixtures.exchange(id: "e1")))
        let model = model(session, chooser: FakeHarFileChooser())
        await model.loadFirstPage()
        await model.select("e1")

        let name = try! #require(model.suggestedSaveName)
        #expect(name.hasPrefix("GET "))
        #expect(name.contains("/v1/users"))
    }
}

@Suite("HAR file names")
struct HarFileNameTests {
    private let moment = Date(timeIntervalSince1970: 1_700_000_000)

    @Test("an export of everything is named for ProxyLab and the moment")
    func everything() {
        let name = HarFileName.suggested(host: nil, at: moment)

        #expect(name.hasPrefix("ProxyLab "))
        #expect(name.hasSuffix(".har"))
    }

    @Test("an export of one host is named for that host")
    func oneHost() {
        let name = HarFileName.suggested(host: "api.example.com", at: moment)

        #expect(name.hasPrefix("api.example.com "))
        #expect(name.hasSuffix(".har"))
    }

    @Test("nothing that is not a file name survives into one")
    func noPathSeparators() {
        // A host is already safe; the filter is here because what is being
        // built is a file name, and a `/` in one is a directory nobody asked
        // for.
        let name = HarFileName.suggested(host: "../../etc/passwd", at: moment)

        #expect(!name.contains("/"))
        #expect(name.hasSuffix(".har"))
    }

    @Test("a host with nothing usable in it still produces a name")
    func unusableHost() {
        #expect(HarFileName.suggested(host: "///", at: moment).hasPrefix("ProxyLab "))
    }
}
