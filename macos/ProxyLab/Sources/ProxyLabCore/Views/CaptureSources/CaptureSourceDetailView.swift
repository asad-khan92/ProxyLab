//  The setup for one capture source, opened from its row.
//
//  A name, one state, the facts that state needs, and the single next
//  action. Start and Stop are not in here: those stay on the window toolbar.

import SwiftUI

/// The detail popover. Pickers, when the presentation asks for one, sit in
/// `extras` above the action.
struct CaptureSourceDetailView<Extras: View>: View {
    let presentation: CaptureSourcePresentation
    var refreshDisabled: Bool
    var onRefresh: () -> Void
    var perform: (CaptureSourceControl) -> Void
    @ViewBuilder var extras: () -> Extras

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            if !presentation.facts.isEmpty {
                facts
            }
            extras()
            if let message = presentation.message {
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(messageColor)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            if !presentation.certificateSteps.isEmpty {
                steps
            }
            if !presentation.controls.isEmpty {
                controls
            }
            if let technical = presentation.technicalDetail {
                DisclosureGroup("Details…") {
                    Text(technical)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(.system(size: 11))
                .accessibilityLabel("Details")
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            CaptureSourceMark(tone: presentation.state.tone, filled: presentation.state.filled)
            VStack(alignment: .leading, spacing: 1) {
                Text(presentation.state.title)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                Text(presentation.state.subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(presentation.state.tone == .fault ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Button(action: onRefresh) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11))
            }
            .buttonStyle(.borderless)
            .disabled(refreshDisabled)
            .help("Look again")
            .accessibilityLabel("Refresh \(presentation.state.title)")
        }
    }

    private var facts: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(presentation.facts) { fact in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(fact.label)
                        .foregroundStyle(.secondary)
                        .frame(width: 118, alignment: .leading)
                    Text(fact.value)
                        .foregroundStyle(.primary)
                        .textSelection(.enabled)
                    Spacer(minLength: 0)
                }
                .font(.system(size: 11))
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(fact.label): \(fact.value)")
            }
        }
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(presentation.certificateSteps.enumerated()), id: \.offset) { index, step in
                Text("\(index + 1). \(step)")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var controls: some View {
        HStack(spacing: 6) {
            ForEach(presentation.controls) { item in
                Button(item.control.title) { perform(item.control) }
                    .controlSize(.small)
                    .disabled(!item.enabled)
            }
        }
    }

    private var messageColor: AnyShapeStyle {
        if presentation.state.tone == .fault { return AnyShapeStyle(.red) }
        if presentation.messageIsWarning { return AnyShapeStyle(.orange) }
        return AnyShapeStyle(.secondary)
    }
}

/// The small dot on a source row.
struct CaptureSourceMark: View {
    var tone: CaptureSourceTone
    var filled: Bool

    var body: some View {
        Circle()
            .fill(filled ? tone.color : Color.clear)
            .overlay(Circle().strokeBorder(tone.color, lineWidth: 1))
            .frame(width: 7, height: 7)
            .accessibilityHidden(true)
    }
}
