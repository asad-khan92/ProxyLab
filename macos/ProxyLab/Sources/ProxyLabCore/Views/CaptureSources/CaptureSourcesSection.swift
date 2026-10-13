//  Capture sources, as a short list at the foot of the sidebar.
//
//      CAPTURE SOURCES
//        ● This Mac                 Listening
//        ● iPhone 17 Pro Max        Connected · CA trusted
//        ○ Android                  No emulator
//        ○ Physical iPhone          Waiting for a local network
//
//  A row opens that source's setup. It does not change the host filter: the
//  list above this one is the only selection that narrows traffic.

import SwiftUI

/// The four sources, collapsed. The detail is a popover.
struct CaptureSourcesSection: View {
    let simulator: SimulatorSessionModel
    let android: AndroidSessionModel
    let iPhone: IPhoneSetupModel
    let transparent: TransparentProxyModel
    let lifecycle: CaptureLifecycle
    let coreConnected: Bool

    @State private var open: CaptureSourceKind?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("CAPTURE SOURCES")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .padding(.top, 8)
                .padding(.bottom, 2)

            ForEach(rows) { row in
                sourceRow(row)
            }
        }
        .padding(.bottom, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Capture sources")
        .task { await simulator.refresh() }
        .task { await android.refresh() }
        .task { await iPhone.refresh() }
        .task { await transparent.refresh() }
    }

    private var rows: [CaptureSourceState] {
        [
            mac.state,
            CaptureSourcePresentation.simulator(
                model: simulator,
                transparentCaptureListening: lifecycle.phase == .listening,
                coreConnected: coreConnected
            ).state,
            CaptureSourcePresentation.android(model: android, coreConnected: coreConnected).state,
            CaptureSourcePresentation.iPhone(model: iPhone, coreConnected: coreConnected).state,
        ]
    }

    /// Read separately so This Mac's lifecycle fields are observed.
    private var mac: CaptureSourcePresentation {
        CaptureSourcePresentation.thisMac(
            lifecycle: lifecycle,
            lan: iPhone,
            coreConnected: coreConnected
        )
    }

    private func sourceRow(_ row: CaptureSourceState) -> some View {
        Button {
            open = open == row.source ? nil : row.source
        } label: {
            CaptureSourceRowLabel(state: row, selected: open == row.source)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(row.title), \(row.subtitle)")
        .accessibilityHint("Show setup")
        .popover(isPresented: presented(row.source), arrowEdge: .leading) {
            detail(for: row.source)
                .frame(width: 300)
        }
    }

    private func presented(_ source: CaptureSourceKind) -> Binding<Bool> {
        Binding(
            get: { open == source },
            set: { isPresented in
                if isPresented {
                    open = source
                } else if open == source {
                    open = nil
                }
            }
        )
    }

    @ViewBuilder
    private func detail(for source: CaptureSourceKind) -> some View {
        switch source {
        case .mac:
            TransparentProxySectionView(
                model: transparent,
                coreConnected: coreConnected,
                lifecycle: lifecycle,
                lan: iPhone
            )
        case .simulator:
            SimulatorSectionView(
                model: simulator,
                coreConnected: coreConnected,
                transparentCaptureListening: lifecycle.phase == .listening
            )
        case .android:
            AndroidSectionView(model: android, coreConnected: coreConnected)
        case .iphone:
            IPhoneSectionView(model: iPhone, coreConnected: coreConnected)
        }
    }
}

/// One source: a dot, a name, and one state. No buttons and no paragraph.
struct CaptureSourceRowLabel: View {
    let state: CaptureSourceState
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            CaptureSourceMark(tone: state.tone, filled: state.filled)
            VStack(alignment: .leading, spacing: 0) {
                Text(state.title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Text(state.subtitle)
                    .font(.system(size: 10))
                    .foregroundStyle(state.tone == .fault ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(rowBackground)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    private var rowBackground: Color {
        if selected { return Color.accentColor.opacity(0.18) }
        if hovering { return Color.primary.opacity(0.06) }
        return .clear
    }
}
