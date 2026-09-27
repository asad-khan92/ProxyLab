//  The saved requests: what is kept, and what is being done to one.
//
//      ┌─ SAVED REQUESTS ────┐
//      │ POST Create a user  │──▶ View ──▶ the editor ──▶ Replay
//      │ GET  Health         │    Rename
//      │ GET  Orders         │    Delete
//      └─────────────────────┘
//
//  Four operations and an editor. There are no folders, no collections, no
//  environments, no variables, and no authentication manager: this is a
//  handful of requests somebody wants to send again, and it is deliberately
//  not a request workspace.
//
//  # Nothing on screen changes before the core says it changed
//
//  Saving, renaming, editing and deleting all go to the core first, and the
//  list is what came back. A row that has vanished from a list but not from
//  the store is worse than one that takes a moment, which is the same rule the
//  rules screen follows.
//
//  # Replaying is the replay every other request gets
//
//  A saved request is turned into a ``ReplayRequest`` and sent through
//  ``TrafficSession/replay(_:)`` — the one path through the core's rules, TLS,
//  capture and storage. The new exchange arrives on the traffic subscription
//  like any other and takes its place in the list; nothing here selects it,
//  for the reason the cURL import sheet does not.
//
//  # Editing before replaying
//
//  The editor is ``CurlImportModel``'s, filled in from the saved request
//  rather than from a pasted command. So the header rows, the body that is not
//  text, and the size check against the core's request limit are the ones that
//  already exist, and there is one request editor in this app rather than two.
//  What the sheet adds is a second button: send it, or save the changes.
//
//  # Credentials
//
//  A saved request carries whatever the request carried. Nothing here logs
//  one, and nothing here authenticates — an expired token comes back from the
//  server as the `401` it is.

import Foundation
import Observation

/// What the saved requests list is doing.
public enum SavedRequestsState: Equatable, Sendable {
    /// Nothing has been asked for yet.
    case idle
    /// The list is being fetched.
    case loading
    /// The list is what the core has.
    case loaded
    /// The list could not be fetched.
    case failed(String)
}

/// One saved request being looked at, changed, or sent.
///
/// The editor is the cURL import sheet's, filled in from the request rather
/// than from a pasted command — so there is one request editor in this app.
@MainActor
@Observable
public final class SavedRequestEditor {
    /// Which saved request this is editing.
    public let id: String
    /// What it is called. Editable here, which is what makes Rename part of
    /// the same sheet rather than a second one.
    public var name: String
    /// Whether the body the core kept is less than the request sent.
    public let sendsLessThanItWas: Bool

    /// The request itself, and everything that can be done to it.
    public let request: CurlImportModel

    /// What the last save came to, for the sheet to show.
    public private(set) var saveState: SavedRequestSaveState = .idle

    public init(saved: SavedRequest, session: any TrafficSession) {
        id = saved.id
        name = saved.name
        sendsLessThanItWas = saved.sendsLessThanItWas
        request = CurlImportModel(session: session)
        request.show(saved.replayRequest)
    }

    /// Whether there is a name to save under.
    public var hasName: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// The change this editor describes.
    public var update: SavedRequestUpdate {
        var update = SavedRequestUpdate.request(request.request)
        update.name = name.trimmingCharacters(in: .whitespaces)

        return update
    }

    /// Says what a save came to, for the sheet to show.
    func saved(_ state: SavedRequestSaveState) {
        saveState = state
    }
}

/// What the last save of an open editor came to.
public enum SavedRequestSaveState: Equatable, Sendable {
    /// Nothing has been saved yet.
    case idle
    /// A save is in flight.
    case saving
    /// The changes are in the core.
    case saved
    /// The core would not keep them, and said why.
    case failed(String)
}

/// One exchange being kept, and what it is about to be called.
public struct SaveRequestPrompt: Identifiable, Hashable, Sendable {
    /// The exchange whose request is being kept.
    public let exchangeID: String
    /// What to call it.
    public var name: String

    public var id: String { exchangeID }

    public init(exchangeID: String, name: String) {
        self.exchangeID = exchangeID
        self.name = name
    }
}

/// The saved requests, and what is being done to one.
@MainActor
@Observable
public final class SavedRequestsModel {
    /// The requests, in the order they were saved.
    public private(set) var requests: [SavedRequest] = []
    /// What the list is doing.
    public private(set) var state: SavedRequestsState = .idle
    /// The editor, while one is open.
    public private(set) var editor: SavedRequestEditor?
    /// The request being renamed in place, while one is.
    public private(set) var renaming: SavedRequest?
    /// The exchange being saved, and what to call it, while one is.
    public var saving: SaveRequestPrompt?
    /// The last thing that went wrong, for the status bar.
    public private(set) var lastError: String?
    /// What the last thing that worked came to, for the status bar.
    public private(set) var lastAction: String?

    private let session: any SavedRequestsSession
    private let traffic: any TrafficSession

    public init(session: any SavedRequestsSession, traffic: any TrafficSession) {
        self.session = session
        self.traffic = traffic
    }

    /// Whether there is anything kept at all.
    public var isEmpty: Bool { requests.isEmpty }

    // MARK: - The list

    /// Asks the core for the list.
    ///
    /// Called when the section appears and after anything this app changed.
    /// Saved requests are not events and the core says so, so there is no
    /// subscription and no polling — which is the same discipline the rules
    /// screen follows.
    public func reload() async {
        state = .loading

        do {
            requests = try await session.listSavedRequests()
            state = .loaded
            lastError = nil
        } catch {
            let failure = message(for: error)
            state = .failed(failure)
            lastError = failure
        }
    }

    // MARK: - Keeping one

    /// Keeps the request of the exchange `id` names, under `name`.
    ///
    /// The request is the core's: it stored the request and is what knows what
    /// it was, so nothing here assembles one out of the parts of a row.
    @discardableResult
    public func save(exchange id: String, as name: String) async -> SavedRequest? {
        do {
            let saved = try await session.saveRequest(fromExchange: id, name: name)
            await reload()

            lastError = nil
            lastAction = saved.sendsLessThanItWas
                ? "Saved “\(saved.name)” — the captured body was truncated, so it sends less "
                    + "than the request did."
                : "Saved “\(saved.name)”."

            return saved
        } catch {
            lastAction = nil
            lastError = message(for: error)

            return nil
        }
    }

    /// Keeps a request this app has described.
    @discardableResult
    public func save(_ request: NewSavedRequest) async -> SavedRequest? {
        do {
            let saved = try await session.createSavedRequest(request)
            await reload()

            lastError = nil
            lastAction = "Saved “\(saved.name)”."

            return saved
        } catch {
            lastAction = nil
            lastError = message(for: error)

            return nil
        }
    }

    /// Asks what to call the request of the exchange `id` names.
    ///
    /// A prompt rather than a name chosen here, because a saved request's
    /// name is the whole of how it is found again and a list of
    /// `GET /v1/items` is a list nobody can read. The suggestion is the
    /// method and the path, which is what a person would type anyway.
    public func beginSave(exchange id: String, suggestedName: String) {
        lastAction = nil
        saving = SaveRequestPrompt(exchangeID: id, name: suggestedName)
    }

    /// Stops saving, keeping nothing.
    public func cancelSave() {
        saving = nil
    }

    /// Keeps the request the prompt names, under the name it carries.
    public func confirmSave() async {
        guard let prompt = saving else { return }
        let name = prompt.name.trimmingCharacters(in: .whitespaces)

        guard !name.isEmpty else {
            lastAction = nil
            lastError = "A saved request must have a name."

            return
        }

        saving = nil
        await save(exchange: prompt.exchangeID, as: name)
    }

    // MARK: - Renaming and removing

    /// Starts renaming `request` in place.
    public func beginRename(_ request: SavedRequest) {
        renaming = request
    }

    /// Stops renaming, keeping nothing.
    public func cancelRename() {
        renaming = nil
    }

    /// Renames `id` to `name`, once the core says it did.
    public func rename(_ id: String, to name: String) async {
        let name = name.trimmingCharacters(in: .whitespaces)
        renaming = nil

        guard !name.isEmpty else {
            lastAction = nil
            lastError = "A saved request must have a name."

            return
        }

        do {
            let saved = try await session.updateSavedRequest(id: id, .rename(name))
            await reload()

            lastError = nil
            lastAction = "Renamed to “\(saved.name)”."
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// Removes `id`, once the core says it is gone.
    public func delete(_ id: String) async {
        let name = requests.first { $0.id == id }?.name

        do {
            _ = try await session.deleteSavedRequest(id: id)
            if editor?.id == id { editor = nil }
            await reload()

            lastError = nil
            lastAction = name.map { "Deleted “\($0)”." } ?? "Deleted the saved request."
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    // MARK: - Looking at one

    /// Opens the editor on `request`.
    ///
    /// Fetched from the core rather than taken from the row this app is
    /// holding, so what is shown is what the core has now — and so a request
    /// somebody else removed is said to be gone rather than opened.
    public func open(_ request: SavedRequest) async {
        do {
            let saved = try await session.savedRequest(id: request.id)
            editor = SavedRequestEditor(saved: saved, session: traffic)
            lastError = nil
        } catch let error as IPCError where error.serverCode == .notFound {
            editor = nil
            await reload()
            lastAction = nil
            lastError = "That saved request no longer exists."
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// Closes the editor, keeping nothing that was not saved.
    public func closeEditor() {
        editor = nil
    }

    /// Saves what the editor describes, and leaves it open.
    ///
    /// Open on purpose: an edit is usually made in order to send it, and a
    /// sheet that closed on Save would have to be reopened to press Replay.
    public func saveEdits() async {
        guard let editor else { return }
        guard editor.hasName else {
            editor.saved(.failed("A saved request must have a name."))

            return
        }

        editor.saved(.saving)

        do {
            _ = try await session.updateSavedRequest(id: editor.id, editor.update)
            editor.saved(.saved)
            await reload()

            lastError = nil
            lastAction = "Saved “\(editor.name)”."
        } catch {
            editor.saved(.failed(message(for: error)))
        }
    }

    // MARK: - Sending one

    /// Sends what the editor describes, through the core's own replay.
    ///
    /// The sheet stays open: a saved request is a thing to send more than
    /// once, and the exchange it produced is in the traffic list where every
    /// other one is. What was sent is whatever the editor is showing, changed
    /// or not — the change is only written down if Save is pressed.
    public func replayEdits() async {
        guard let editor else { return }
        guard let result = await editor.request.replay() else { return }

        lastError = nil
        lastAction = report(result, name: editor.name)
    }

    /// Sends `request` as it stands, without opening the editor.
    public func replay(_ request: SavedRequest) async {
        do {
            let result = try await traffic.replay(request.replayRequest)

            lastError = nil
            lastAction = report(result, name: request.name)
        } catch {
            lastAction = nil
            lastError = message(for: error)
        }
    }

    /// What the status bar says about a replay that happened.
    private func report(_ result: ReplayResult, name: String) -> String {
        let outcome = result.status.map(String.init) ?? (result.error?.summary ?? "no response")

        return "Replayed “\(name)” — \(outcome). The new exchange is in the list."
    }

    /// What a person is told about `error`.
    private func message(for error: any Error) -> String {
        (error as? IPCError)?.message ?? "Something went wrong talking to the ProxyLab core."
    }
}
