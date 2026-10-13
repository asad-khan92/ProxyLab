//  The four places traffic comes from, as toolbar buttons.
//
//      [ Simulator ] [ Android ] [ iPhone ] [ This Mac ]
//            │            │          │           │
//            └────────────┴──────────┴───────────┘
//                         the same details the sidebar
//                         rows open
//
//  Each button opens that source's setup. It does not start or stop capture,
//  and it does not connect in a second way: Connect, Disconnect, and the
//  certificate actions are the detail's own.

import SwiftUI

/// One source of traffic the toolbar can open.
enum CaptureSource: String, CaseIterable, Identifiable {
    case simulator
    case android
    case iphone
    case mac

    var id: String { rawValue }

    /// The word on the button.
    var title: String {
        switch self {
        case .simulator: "Simulator"
        case .android: "Android"
        case .iphone: "iPhone"
        case .mac: "This Mac"
        }
    }

    /// The symbol beside it.
    var symbol: String {
        switch self {
        case .simulator: "iphone"
        case .android: "candybarphone"
        case .iphone: "iphone.gen3"
        case .mac: "desktopcomputer"
        }
    }

    /// What hovering the button says.
    var help: String {
        switch self {
        case .simulator:
            "Trust ProxyLab's certificate in a booted iOS Simulator"
        case .android:
            "Point a running Android emulator at ProxyLab and send it the certificate"
        case .iphone:
            "Let a physical iPhone reach this proxy, and see what to type into it"
        case .mac:
            "Capture this Mac's HTTP and HTTPS without setting a proxy"
        }
    }
}

/// Simulator, Android, iPhone, and This Mac, each opening its existing panel.
struct CaptureSourcesToolbar: View {
    let simulator: SimulatorSessionModel
    let android: AndroidSessionModel
    let iPhone: IPhoneSetupModel
    let transparent: TransparentProxyModel

    /// Whether the window's connection to the core is up. Passed into the
    /// panels, which already decide what they can do from it.
    let coreConnected: Bool

    /// The capture lifecycle, passed into This Mac so that panel and the
    /// toolbar cannot show two different capture states.
    var lifecycle: CaptureLifecycle? = nil

    @State private var open: CaptureSource?

    var body: some View {
        HStack(spacing: 2) {
            ForEach(CaptureSource.allCases) { source in
                Button {
                    open = open == source ? nil : source
                } label: {
                    // An image and a word, rather than a `Label`. A toolbar
                    // draws a label as an icon and keeps the title for the
                    // tooltip, which is how these four were missed.
                    HStack(spacing: 4) {
                        Image(systemName: source.symbol)
                        Text(source.title)
                    }
                    .font(.system(size: 12))
                }
                .buttonStyle(.borderless)
                .help(source.help)
                .accessibilityLabel(source.title)
                .popover(isPresented: presented(source), arrowEdge: .top) {
                    panel(for: source)
                        .frame(width: 300)
                }
            }
        }
    }

    private func presented(_ source: CaptureSource) -> Binding<Bool> {
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
    func panel(for source: CaptureSource) -> some View {
        switch source {
        case .simulator:
            SimulatorSectionView(
                model: simulator,
                coreConnected: coreConnected,
                transparentCaptureListening: lifecycle?.phase == .listening
            )
        case .android:
            AndroidSectionView(model: android, coreConnected: coreConnected)
        case .iphone:
            IPhoneSectionView(model: iPhone, coreConnected: coreConnected)
        case .mac:
            TransparentProxySectionView(
                model: transparent,
                coreConnected: coreConnected,
                lifecycle: lifecycle,
                lan: iPhone
            )
        }
    }
}
