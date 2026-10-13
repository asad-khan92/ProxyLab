//  The bar above the traffic list: what is being looked for.
//
//      ┌──────────────────────────────────────────────┐
//      │ 🔍 Search requests                     ✕     │
//      │ Method ▾  Status ▾  Type ▾  HTTP ▾  Kind ▾  Host ▾  Sort ▾  Clear │
//      └──────────────────────────────────────────────┘
//
//  Native controls throughout: a search field and four menus. Nothing here
//  holds state of its own — every control reads `model.filters` and changes it
//  through the model, so there is one answer to what the list is showing and
//  it is not spread across four views.
//
//  Typing does not send a request per keystroke. The model waits for the
//  typing to settle; Return asks straight away.

import SwiftUI

/// Search, filters, and sort, above the list.
struct TrafficFilterBar: View {
    let model: TrafficInspectorModel
    var searchFocused: FocusState<Bool>.Binding

    var body: some View {
        VStack(spacing: 6) {
            searchField

            // The menus are as wide as their titles. In a narrow list column
            // they scroll sideways rather than drawing past the column; when
            // they fit, sort stays at the trailing edge.
            ViewThatFits(in: .horizontal) {
                filterControls(alignSortToTrailing: true)
                ScrollView(.horizontal) {
                    filterControls(alignSortToTrailing: false)
                }
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }

    private func filterControls(alignSortToTrailing: Bool) -> some View {
        HStack(spacing: 6) {
            methodMenu
            statusMenu
            contentTypeMenu
            versionMenu
            kindMenu
            hostMenu

            if alignSortToTrailing {
                Spacer(minLength: 4)
            }

            sortMenu

            if model.filters.isNarrowed {
                Button("Clear") {
                    Task { await model.clearFilters() }
                }
                .controlSize(.small)
                .help("Clear the search and every filter")
            }
        }
    }

    // MARK: - Search

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)

            TextField("Search URL, headers, body, or messages", text: search)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .focused(searchFocused)
                .onSubmit { Task { await model.applySearchNow() } }
                .accessibilityLabel("Search traffic")

            Button {
                Task { await model.setSearchRegex(!model.filters.searchRegex) }
            } label: {
                Text(".*")
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(model.filters.searchRegex ? Color.accentColor : Color.secondary)
            }
            .buttonStyle(.plain)
            .help("Treat the search as a regular expression")
            .accessibilityLabel("Regular expression")
            .accessibilityValue(model.filters.searchRegex ? "On" : "Off")

            Button {
                Task { await model.setSearchCaseSensitive(!model.filters.searchCaseSensitive) }
            } label: {
                Text("Aa")
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(
                        model.filters.searchCaseSensitive ? Color.accentColor : Color.secondary
                    )
            }
            .buttonStyle(.plain)
            .help("Match letter case")
            .accessibilityLabel("Match case")
            .accessibilityValue(model.filters.searchCaseSensitive ? "On" : "Off")

            if !model.filters.search.isEmpty {
                Button {
                    Task {
                        model.setSearch("")
                        await model.applySearchNow()
                    }
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .help("Clear the search")
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 6))
    }

    /// The field's text, which the model owns.
    private var search: Binding<String> {
        Binding(
            get: { model.filters.search },
            set: { model.setSearch($0) }
        )
    }

    // MARK: - Filters

    private var methodMenu: some View {
        FilterMenu(
            title: "Method",
            selection: model.filters.method,
            options: HTTPMethodFilter.all.map { ($0, $0) }
        ) { method in
            Task { await model.setMethod(method) }
        }
    }

    private var statusMenu: some View {
        FilterMenu(
            title: "Status",
            selection: statusSelectionLabel,
            options: statusOptions
        ) { label in
            Task { await chooseStatus(label) }
        }
    }

    private var statusSelectionLabel: String? {
        if model.filters.failed { return "Failed" }
        if let code = model.filters.statusCode { return String(code) }
        return model.filters.status?.label
    }

    private var statusOptions: [(id: String, label: String)] {
        var options = StatusClass.allCases.map { ($0.label, $0.label) }
        options.append(("Failed", "Failed"))
        options.append(contentsOf: HTTPStatusCodeFilter.common.map { (String($0), String($0)) })
        return options
    }

    private func chooseStatus(_ label: String?) async {
        switch label {
        case nil:
            await model.setStatus(nil)
            await model.setStatusCode(nil)
            await model.setFailed(false)
        case "Failed":
            await model.setStatus(nil)
            await model.setStatusCode(nil)
            await model.setFailed(true)
        case let value? where UInt16(value) != nil:
            await model.setStatus(nil)
            await model.setFailed(false)
            await model.setStatusCode(UInt16(value))
        default:
            let status = StatusClass.allCases.first { $0.label == label }
            await model.setStatusCode(nil)
            await model.setFailed(false)
            await model.setStatus(status)
        }
    }

    private var contentTypeMenu: some View {
        FilterMenu(
            title: "Type",
            selection: model.filters.contentType,
            options: ContentTypeFilter.presets.map { ($0.id, $0.label) }
        ) { id in
            Task { await model.setContentType(id) }
        }
    }

    private var versionMenu: some View {
        FilterMenu(
            title: "HTTP",
            selection: versionLabel,
            options: [("HTTP/1.1", "HTTP/1.1"), ("HTTP/2", "HTTP/2")]
        ) { label in
            Task { await model.setHTTPVersion(version(from: label)) }
        }
    }

    private var versionLabel: String? {
        switch model.filters.httpVersion {
        case .http11: "HTTP/1.1"
        case .http2: "HTTP/2"
        default: nil
        }
    }

    private func version(from label: String?) -> HTTPVersion? {
        switch label {
        case "HTTP/1.1": .http11
        case "HTTP/2": .http2
        default: nil
        }
    }

    private var kindMenu: some View {
        FilterMenu(
            title: "Kind",
            selection: kindLabel,
            options: [("websocket", "WebSocket"), ("sse", "SSE")]
        ) { id in
            Task { await model.setStreamKind(id) }
        }
    }

    private var kindLabel: String? {
        if model.filters.websocket { return "websocket" }
        if model.filters.sse { return "sse" }
        return nil
    }

    private var hostMenu: some View {
        FilterMenu(
            title: "Host",
            selection: model.filters.host,
            options: model.knownHosts.map { ($0, $0) },
            emptyNote: "No hosts in the list yet"
        ) { host in
            Task { await model.setHost(host) }
        }
    }

    private var sortMenu: some View {
        Menu {
            ForEach(TrafficSort.allCases) { sort in
                Button {
                    Task { await model.setSort(sort) }
                } label: {
                    if sort == model.filters.sort {
                        Label(sort.label, systemImage: "checkmark")
                    } else {
                        Text(sort.label)
                    }
                }
            }
        } label: {
            Label(model.filters.sort.label, systemImage: "arrow.up.arrow.down")
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .controlSize(.small)
        .help("Which end of the order the list is read from")
    }
}

/// One "All, or this one" menu.
///
/// The same shape three times: a title, what is selected, and the values to
/// choose from. `nil` is "All" and is always offered, because a filter a
/// developer cannot get out of is a trap.
private struct FilterMenu: View {
    let title: String
    let selection: String?
    let options: [(id: String, label: String)]
    var emptyNote: String?
    let choose: (String?) -> Void

    /// The menu's title uses the option's label, not its id.
    private var shown: String? {
        selection.flatMap { id in
            options.first { $0.id == id }?.label ?? id
        }
    }

    var body: some View {
        Menu {
            Button {
                choose(nil)
            } label: {
                if selection == nil {
                    Label("All", systemImage: "checkmark")
                } else {
                    Text("All")
                }
            }

            if options.isEmpty, let emptyNote {
                Divider()
                Text(emptyNote)
            } else {
                Divider()
                ForEach(options, id: \.id) { option in
                    Button {
                        choose(option.id)
                    } label: {
                        if option.id == selection {
                            Label(option.label, systemImage: "checkmark")
                        } else {
                            Text(option.label)
                        }
                    }
                }
            }
        } label: {
            Text(shown.map { "\(title): \($0)" } ?? "\(title): All")
                .font(.system(size: 11))
                .foregroundStyle(selection == nil ? .secondary : .primary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .controlSize(.small)
    }
}
