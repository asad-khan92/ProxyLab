//  The requests worth keeping, at the foot of the sidebar.
//
//      ┌ SAVED REQUESTS             ↻ ┐
//      │ POST  Create a user          │──▶ the editor
//      │ GET   Health                 │
//      │ GET   Orders for a customer  │
//      └──────────────────────────────┘
//
//  Under the domains rather than among them, for the reason the simulator and
//  emulator panels are: the list above is a selection of hosts, and a saved
//  request is not a host. Selecting one here must not change what the traffic
//  list is showing, so these rows are buttons rather than list selections —
//  the two selections in this window are the host filter and the exchange
//  being inspected, and this is neither.
//
//  What a row offers is the four operations and nothing else: open it, send
//  it, rename it, remove it. There are no folders, no collections, and no
//  environments — see ``SavedRequestsModel`` for why not.

import SwiftUI

/// The saved requests section.
struct SavedRequestsSectionView: View {
    let model: SavedRequestsModel

    /// Whether the window's connection to the core is up. The saved requests
    /// are the core's, so without one there is nothing to show and nothing to
    /// press.
    let coreConnected: Bool

    @State private var expanded = true
    @State private var renameText = ""
    @State private var confirmingDelete: SavedRequest?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header

            if expanded {
                content
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        // Asked for whenever the connection comes up, and not before: the
        // saved requests are the core's, so a section that loaded once at
        // launch would stay empty for anyone who started the app first. It
        // is the same reason the rules screen reloads when it appears —
        // saved requests are not events and the core says so, so there is no
        // subscription and nothing to fall behind.
        .task(id: coreConnected) {
            guard coreConnected else { return }

            await model.reload()
        }
        .confirmationDialog(
            "Delete “\(confirmingDelete?.name ?? "")”?",
            isPresented: deleteBinding,
            presenting: confirmingDelete
        ) { request in
            Button("Delete Saved Request", role: .destructive) {
                Task { await model.delete(request.id) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("The request is removed from the ProxyLab core. This cannot be undone.")
        }
    }

    private var header: some View {
        HStack(spacing: 4) {
            Button {
                expanded.toggle()
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                    Text("SAVED REQUESTS")
                        .font(.system(size: 10, weight: .semibold))
                }
                .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(expanded ? "Hide saved requests" : "Show saved requests")

            Spacer(minLength: 4)

            Button {
                Task { await model.reload() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 9))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .disabled(!coreConnected)
            .help("Ask the core for the saved requests again")
            .accessibilityLabel("Refresh saved requests")
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.state {
        case .idle:
            note(coreConnected ? "Loading…" : "Waiting for the ProxyLab core.")
        case .loading where model.isEmpty:
            note("Loading…")
        case let .failed(reason):
            Text(UserFacingNotice(reason).summary)
                .font(.system(size: 10))
                .foregroundStyle(StatusTone.fault.color)
                .fixedSize(horizontal: false, vertical: true)
                .help(UserFacingNotice(reason).detail ?? reason)
        case .loaded where model.isEmpty:
            VStack(alignment: .leading, spacing: 2) {
                Text(SavedEmptyCopy.title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                Text(SavedEmptyCopy.detail)
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .combine)
        default:
            rows
        }
    }

    private var rows: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(model.requests) { request in
                if model.renaming?.id == request.id {
                    renameField(for: request)
                } else {
                    row(request)
                }
            }
        }
    }

    private func row(_ request: SavedRequest) -> some View {
        SavedRequestRow(request: request) {
            Task { await model.open(request) }
        }
        .disabled(!coreConnected)
        .contextMenu {
            Button("View and Edit…") { Task { await model.open(request) } }
            Button("Replay") { Task { await model.replay(request) } }
            Divider()
            Button("Rename…") {
                renameText = request.name
                model.beginRename(request)
            }
            Button("Delete Saved Request…", role: .destructive) { confirmingDelete = request }
        }
    }

    /// Renaming happens in the row, not in a sheet.
    ///
    /// A name is one field, and a sheet for one field is a sheet to dismiss.
    private func renameField(for request: SavedRequest) -> some View {
        TextField("Name", text: $renameText)
            .textFieldStyle(.roundedBorder)
            .font(.system(size: 11))
            .onSubmit { Task { await model.rename(request.id, to: renameText) } }
            .onExitCommand { model.cancelRename() }
            .accessibilityLabel("Rename \(request.name)")
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10))
            .foregroundStyle(.tertiary)
            .fixedSize(horizontal: false, vertical: true)
    }

    /// Whether the delete confirmation is up. Set only to close it, which is
    /// the only thing a dialog's own dismissal can mean.
    private var deleteBinding: Binding<Bool> {
        Binding(
            get: { confirmingDelete != nil },
            set: { showing in if !showing { confirmingDelete = nil } }
        )
    }
}

/// One saved request: what it does, and what it is called.
struct SavedRequestRow: View {
    let request: SavedRequest
    let open: () -> Void

    @State private var hovering = false

    var body: some View {
        Button(action: open) {
            HStack(spacing: 6) {
                Text(request.method.uppercased())
                    .font(.system(size: 9, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .frame(width: 42, alignment: .leading)

                Text(request.name)
                    .font(.system(size: 12))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .foregroundStyle(.primary)

                Spacer(minLength: 4)

                if request.sendsLessThanItWas {
                    Image(systemName: "scissors")
                        .font(.system(size: 9))
                        .foregroundStyle(.tertiary)
                        .help("The captured body was truncated, so this sends less than the "
                            + "request did")
                }
            }
            .padding(.vertical, 2)
            .padding(.horizontal, 4)
            .contentShape(Rectangle())
            .background(
                hovering ? AnyShapeStyle(.quaternary.opacity(0.5)) : AnyShapeStyle(.clear),
                in: RoundedRectangle(cornerRadius: 4)
            )
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(request.url)
        .accessibilityLabel("Saved request \(request.name), \(request.method)")
        .accessibilityHint("Open it to view, edit, or replay")
    }
}
