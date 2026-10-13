//  The Messages face of a WebSocket handshake or an event stream.
//
//  A timeline of captured messages, and the selected payload shown the way
//  an HTTP body already is. Nothing here is a debugger of its own: it is
//  another face of the same pane, filled from the same inspector model.

import SwiftUI

/// Captured WebSocket messages or SSE events on the selected exchange.
struct StreamMessagesView: View {
    let exchange: TrafficExchange
    let model: TrafficInspectorModel

    var body: some View {
        DetailSection(title: sectionTitle) {
            timeline
        }

        payload
    }

    private var sectionTitle: String {
        let noun = exchange.isEventStream ? "Events" : "Messages"

        return "\(noun) (\(model.streamMessages.count))"
    }

    @ViewBuilder
    private var timeline: some View {
        if let error = model.streamError {
            Text(error)
                .font(.system(size: 11))
                .foregroundStyle(.red)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        } else if model.isLoadingStreams && model.streamMessages.isEmpty {
            Text("Loading messages…")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
        } else if model.streamMessages.isEmpty {
            Text(
                exchange.isEventStream
                    ? "No events captured yet. The stream is still open, or none have arrived."
                    : "No messages captured yet. The socket is still open, or none have arrived."
            )
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        } else {
            VStack(alignment: .leading, spacing: 2) {
                ForEach(model.streamMessages) { message in
                    StreamMessageRow(
                        message: message,
                        selected: message.id == model.selectedStreamID
                    )
                    .contentShape(Rectangle())
                    .onTapGesture {
                        Task { await model.selectStream(message.id) }
                    }
                    .onAppear {
                        guard
                            let index = model.streamMessages.firstIndex(where: { $0.id == message.id }),
                            model.shouldLoadMoreStreams(atRow: index)
                        else { return }

                        Task { await model.loadMoreStreams() }
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var payload: some View {
        switch model.streamDetail {
        case .none:
            EmptyView()

        case .loading:
            DetailSection(title: "Payload") {
                Text("Loading payload…")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

        case .unavailable:
            DetailSection(title: "Payload") {
                Text("This message is gone.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

        case let .failed(reason):
            DetailSection(title: "Payload") {
                Text(reason)
                    .font(.system(size: 11))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }

        case let .loaded(message):
            if let inspection = model.streamInspection {
                StreamPayloadView(message: message, inspection: inspection, model: model)
            }
        }
    }
}

/// One row of the message timeline.
private struct StreamMessageRow: View {
    let message: StreamMessageSummary
    let selected: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(message.direction == .clientToServer ? "→" : "←")
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(message.direction == .clientToServer ? Color.blue : Color.green)
                .frame(width: 14)

            Text(message.headline)
                .font(.system(size: 11, weight: selected ? .semibold : .regular, design: .monospaced))
                .lineLimit(1)

            Spacer(minLength: 8)

            Text(TrafficDisplay.size(message.payloadSize))
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)

            Text(TrafficDisplay.time(message.capturedAt))
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(
            selected
                ? Color.accentColor.opacity(0.14)
                : Color.clear,
            in: RoundedRectangle(cornerRadius: 4)
        )
        .help(message.direction.title)
    }
}

/// The selected message's fields and payload.
private struct StreamPayloadView: View {
    let message: StreamMessage
    let inspection: BodyInspection
    let model: TrafficInspectorModel

    var body: some View {
        DetailSection(title: "About") {
            FieldGrid(fields)
        }

        DetailSection(title: "Payload") {
            BodyView(
                inspection: inspection,
                copySubject: "the payload",
                copy: { text, what in model.copy(text, as: what) }
            )
        }
    }

    private var fields: [FieldGrid.Field] {
        var fields = [
            FieldGrid.Field(name: "Direction", value: message.direction.title),
            FieldGrid.Field(name: "Kind", value: message.kind.title),
            FieldGrid.Field(name: "Time", value: TrafficDisplay.time(message.capturedAt)),
            FieldGrid.Field(
                name: "Size",
                value: TrafficDisplay.size(message.payload.size),
                note: message.payload.truncated
                    ? "Captured "
                        + TrafficDisplay.size(UInt64(message.payload.capturedByteCount))
                        + " — truncated"
                    : nil
            ),
        ]

        if message.kind == .sse {
            fields.append(
                FieldGrid.Field(
                    name: "Event",
                    value: message.eventName.flatMap { $0.isEmpty ? nil : $0 } ?? "message"
                )
            )
            if let eventID = message.eventID, !eventID.isEmpty {
                fields.append(FieldGrid.Field(name: "ID", value: eventID))
            }
            if let retry = message.retryMs {
                fields.append(FieldGrid.Field(name: "Retry", value: "\(retry) ms"))
            }
        }

        return fields
    }
}
