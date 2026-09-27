//  The window: the domains worth coming back to, the traffic list, the
//  exchange behind whatever is selected, and the state of the connection that
//  feeds all three.
//
//      ┌──────────────────────────────────────────────────────┐
//      │ ProxyLab                      ● Connected  ⤓ ⧉ ⌫ ↻ │
//      ├───────────┬──────────────┬───────────────────────────┤
//      │ All Traff │ GET  example │ GET https://example.com/… │
//      │ PINNED    │ POST api/…   │ ┌ Request ┬ Response ┐    │
//      │  api.exa… │ GET  api/…   │ │ headers, body, timing   │
//      ├───────────┤              │                           │
//      │ iOS SIM   │              │                           │
//      │ ANDROID   │              │                           │
//      │ IPHONE    │              │                           │
//      │ THIS MAC  │              │                           │
//      ├───────────┴──────────────┴───────────────────────────┤
//      │ 12 of 12 requests                        Clear All   │
//      └──────────────────────────────────────────────────────┘
//
//  The sidebar narrows the same query the filter bar does — it sets the host
//  and nothing else — so there is one `list_traffic` and one answer to what
//  the list is showing.
//
//  Everything on screen came over the IPC socket. There is no second path to
//  the core, and nothing here is placeholder traffic: an empty list means the
//  proxy has carried nothing since its history was last cleared.

import SwiftUI

/// ProxyLab's traffic inspector.
public struct TrafficInspectorView: View {
    private let model: TrafficInspectorModel
    private let saved: SavedRequestsModel
    private let pinned: PinnedDomainsStore
    private let simulator: SimulatorSessionModel
    private let android: AndroidSessionModel
    private let iPhone: IPhoneSetupModel
    private let transparent: TransparentProxyModel

    /// Which of the window's two screens is showing. Held by the window and
    /// changed through the control this view puts in the toolbar.
    @Binding private var section: AppSection

    @State private var confirmingClear = false

    public init(
        model: TrafficInspectorModel,
        saved: SavedRequestsModel,
        pinned: PinnedDomainsStore,
        simulator: SimulatorSessionModel,
        android: AndroidSessionModel,
        iPhone: IPhoneSetupModel,
        transparent: TransparentProxyModel,
        section: Binding<AppSection>
    ) {
        self.model = model
        self.saved = saved
        self.pinned = pinned
        self.simulator = simulator
        self.android = android
        self.iPhone = iPhone
        self.transparent = transparent
        _section = section
    }

    public var body: some View {
        NavigationSplitView {
            PinnedDomainsView(model: model, pinned: pinned)
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    // The sidebar's foot rather than rows in its list: the
                    // list's rows are hosts to filter by, and none of these
                    // is one. The simulator first because it is the one that
                    // needs no network setup at all, then the emulator, which
                    // needs one setting, then the iPhone, which needs the
                    // core put on the local network.
                    VStack(spacing: 0) {
                        // Above the device panels: a saved request is about
                        // the traffic this screen shows, and the three below
                        // it are about where traffic comes from.
                        SavedRequestsSectionView(
                            model: saved,
                            coreConnected: model.connection == .connected
                        )
                        SimulatorSectionView(
                            model: simulator,
                            coreConnected: model.connection == .connected
                        )
                        AndroidSectionView(
                            model: android,
                            coreConnected: model.connection == .connected
                        )
                        IPhoneSectionView(
                            model: iPhone,
                            coreConnected: model.connection == .connected
                        )
                        // Last: every panel above points a device at the
                        // proxy, and this one points the Mac itself at it,
                        // which is the one with the widest consequence.
                        TransparentProxySectionView(
                            model: transparent,
                            coreConnected: model.connection == .connected
                        )
                    }
                }
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 340)
        } content: {
            TrafficListView(model: model, pinned: pinned, selection: selection)
                .navigationSplitViewColumnWidth(min: 280, ideal: 340, max: 520)
        } detail: {
            TrafficDetailView(model: model)
        }
        .navigationTitle("ProxyLab")
        .navigationSubtitle(subtitle)
        .toolbar { toolbar }
        .safeAreaInset(edge: .bottom, spacing: 0) { statusBar }
        .task { await model.start() }
        .confirmationDialog(
            "Clear all captured traffic?",
            isPresented: $confirmingClear
        ) {
            Button("Clear All Traffic", role: .destructive) {
                Task { await model.clearAll() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Every stored exchange is removed from the core. This cannot be undone.")
        }
        .sheet(isPresented: importBinding) {
            if let importer = model.importer {
                CurlImportView(importer: importer, model: model)
            }
        }
        .sheet(isPresented: savedEditorBinding) {
            if let editor = saved.editor {
                SavedRequestSheet(editor: editor, model: saved)
            }
        }
        .sheet(isPresented: savePromptBinding) {
            if saved.saving != nil {
                SaveRequestSheet(
                    model: saved,
                    prompt: Binding(
                        get: { saved.saving ?? SaveRequestPrompt(exchangeID: "", name: "") },
                        set: { saved.saving = $0 }
                    )
                )
            }
        }
    }

    /// Whether the saved request editor is up. Set only to close it.
    private var savedEditorBinding: Binding<Bool> {
        Binding(
            get: { saved.editor != nil },
            set: { showing in if !showing { saved.closeEditor() } }
        )
    }

    /// Whether the naming prompt is up. Set only to close it.
    private var savePromptBinding: Binding<Bool> {
        Binding(
            get: { saved.saving != nil },
            set: { showing in if !showing { saved.cancelSave() } }
        )
    }

    /// Whether the import sheet is up. Set only to close it, which is the
    /// only thing a sheet's own dismissal can mean.
    private var importBinding: Binding<Bool> {
        Binding(
            get: { model.importer != nil },
            set: { showing in if !showing { model.cancelImport() } }
        )
    }

    /// The list's selection, which is the model's and is set through it.
    private var selection: Binding<String?> {
        Binding(
            get: { model.selectedID },
            set: { id in Task { await model.select(id) } }
        )
    }

    /// What the window's title bar says under "ProxyLab".
    ///
    /// The connection first, because a disconnected window explains every
    /// other thing on screen, and the domain after it when one is selected —
    /// so what the list is narrowed to is readable without looking at the
    /// sidebar.
    private var subtitle: String {
        guard let host = model.filters.host else { return connectionSubtitle }

        return "\(connectionSubtitle) — \(host)"
    }

    private var connectionSubtitle: String {
        switch model.connection {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .idle: "Starting…"
        case .disconnected: "Disconnected"
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            ConnectionIndicator(state: model.connection)
        }

        ToolbarItem(placement: .principal) {
            SectionPicker(section: $section)
        }

        ToolbarItemGroup(placement: .primaryAction) {
            Menu {
                Button("Export Selected…") {
                    Task { await model.exportSelectedAsHar() }
                }
                .disabled(model.selectedID == nil)

                Button(model.filters.isNarrowed ? "Export Filtered…" : "Export All…") {
                    Task { await model.exportFilteredAsHar() }
                }

                Divider()

                Button("Import HAR…") {
                    Task { await model.importHar() }
                }
            } label: {
                Label("HAR", systemImage: "arrow.up.arrow.down.square")
            }
            .disabled(!model.canExport)
            .help("Write this traffic out as a HAR file, or read one in")

            Button {
                guard let id = model.selectedID else { return }

                saved.beginSave(
                    exchange: id,
                    suggestedName: model.suggestedSaveName ?? "Saved request"
                )
            } label: {
                Label("Save Request", systemImage: "bookmark")
            }
            .disabled(model.selectedID == nil)
            .help("Keep the selected request, to send again later")

            Button {
                model.beginImport()
            } label: {
                Label("Import cURL", systemImage: "square.and.arrow.down.on.square")
            }
            .disabled(model.connection != .connected)
            .help("Paste a cURL command and send the request it describes")

            Button {
                Task { await model.copySelectedAsCurl() }
            } label: {
                Label("Copy as cURL", systemImage: "doc.on.doc")
            }
            .disabled(model.selectedID == nil)
            .help("Copy the selected request to the clipboard as a cURL command")

            Button {
                Task { await model.deleteSelected() }
            } label: {
                Label("Delete Exchange", systemImage: "trash")
            }
            .disabled(model.selectedID == nil)
            .keyboardShortcut(.delete, modifiers: [])
            .help("Delete the selected exchange")

            Button {
                Task { await model.retryConnection() }
            } label: {
                Label("Reconnect", systemImage: "arrow.clockwise")
            }
            .disabled(model.connection == .connected)
            .help("Try the ProxyLab core again")
        }
    }

    private var statusBar: some View {
        HStack(spacing: 10) {
            Text(countLabel)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            if let error = model.lastError ?? saved.lastError {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(error)
            } else if let action = model.lastAction ?? saved.lastAction {
                Text(action)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(action)
            }

            Spacer()

            Text(model.socketPath)
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.head)
                .help("The IPC socket this window is reading from")

            Button("Clear All") { confirmingClear = true }
                .controlSize(.small)
                // Enabled whenever the core is answering: a filter that hides
                // every row does not mean there is nothing stored to clear.
                .disabled(model.connection != .connected)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }

    /// How many rows there are, and how many the query matched.
    ///
    /// The total is the total of what the filters match, because that is what
    /// the core counted — so a narrowed list says how much of the narrowed
    /// result is loaded, not how much of the database is.
    private var countLabel: String {
        let listed = model.exchanges.count
        let total = model.totalExchanges
        let noun = model.filters.isNarrowed ? "matching" : "requests"

        if total > UInt64(listed) {
            return model.filters.isNarrowed
                ? "\(listed) of \(total) matching"
                : "\(listed) of \(total) requests"
        }

        if listed == 1 {
            return model.filters.isNarrowed ? "1 matching" : "1 request"
        }

        return "\(listed) \(noun)"
    }
}
