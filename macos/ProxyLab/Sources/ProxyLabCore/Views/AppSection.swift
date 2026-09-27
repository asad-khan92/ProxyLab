//  The three things this window is for.
//
//      ┌──────────────────────────────────────────────────┐
//      │ ● Connected  [ Traffic | Rules | Breaks ]     ⌫  │
//      ├──────────────────────────────────────────────────┤
//
//  A segmented control in the toolbar rather than a sidebar column: the
//  traffic inspector is already a list and a detail, and so are the other two,
//  and a sidebar holding three words would push all of them along without
//  telling anyone anything they could not read in one control.

import SwiftUI

/// Which screen the window is showing.
public enum AppSection: String, CaseIterable, Identifiable, Sendable {
    /// The exchanges the proxy has carried.
    case traffic
    /// The rules it applies to them.
    case rules
    /// The exchanges it holds still, and how slow it is pretending to be.
    case breakpoints

    public var id: String { rawValue }

    /// What the control calls it.
    public var title: String {
        switch self {
        case .traffic: "Traffic"
        case .rules: "Rules"
        case .breakpoints: "Breaks"
        }
    }

    /// The symbol beside it.
    public var symbol: String {
        switch self {
        case .traffic: "arrow.left.arrow.right"
        case .rules: "slider.horizontal.3"
        case .breakpoints: "pause.circle"
        }
    }
}

/// The control that switches between them.
struct SectionPicker: View {
    @Binding var section: AppSection

    var body: some View {
        Picker("Section", selection: $section) {
            ForEach(AppSection.allCases) { section in
                Label(section.title, systemImage: section.symbol)
                    .tag(section)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: 260)
        .help("Switch between captured traffic, the rules applied to it, and breakpoints")
    }
}
