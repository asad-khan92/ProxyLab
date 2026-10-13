//  A column with a footer pinned to the bottom, which scrolls instead of
//  drawing past the column.
//
//  The traffic sidebar is a domain list and, under it, the panels for saved
//  requests and the devices traffic comes from. Those panels stay at the
//  bottom when the column is tall enough, and the list takes the space above
//  them. A `safeAreaInset` laid that foot out at its full height, so in a
//  short window the last panel was drawn below the window. The foot scrolls
//  instead, and the list keeps a short strip so All Traffic stays on screen.
//
//  The list is not inside that scroll view. A `List` offered an unbounded
//  height reports no height at all, so the two scroll independently: the
//  domains in their list, the foot in its own scroll view, and only when the
//  foot does not fit.

import SwiftUI

/// How a bottom-pinned column divides the height it was given.
struct SidebarColumnPlacement: Equatable {
    var content: CGFloat
    var footer: CGFloat

    /// `minimumContent` is the height the top view keeps while the footer
    /// scrolls. On a very short column that reserved height is reduced so the
    /// footer still has most of the space.
    static func heights(
        available: CGFloat,
        footerIdeal: CGFloat,
        minimumContent: CGFloat
    ) -> SidebarColumnPlacement {
        let available = max(0, available)
        let footerIdeal = max(0, footerIdeal)
        let contentFloor = min(max(0, minimumContent), available * 0.34)

        if footerIdeal + contentFloor <= available {
            return SidebarColumnPlacement(
                content: available - footerIdeal,
                footer: footerIdeal
            )
        }

        return SidebarColumnPlacement(
            content: contentFloor,
            footer: available - contentFloor
        )
    }
}

/// The top view fills whatever room the footer does not need. The footer
/// scrolls when that room is shorter than the footer.
struct BottomPinnedColumn<Content: View, Footer: View>: View {
    var minimumContentHeight: CGFloat = 120
    @ViewBuilder var content: () -> Content
    @ViewBuilder var footer: () -> Footer

    var body: some View {
        BottomPinnedColumnLayout(minimumContentHeight: minimumContentHeight) {
            content()
            ScrollView {
                footer()
                    // A short proposal must not collapse the foot. The last
                    // control — Turn On, on a first run — is the piece a
                    // compressed stack throws away, and then there is nothing
                    // left to scroll to.
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollBounceBehavior(.basedOnSize, axes: .vertical)
            .modifier(ZeroScrollMargins())
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct BottomPinnedColumnLayout: Layout {
    var minimumContentHeight: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        proposal.replacingUnspecifiedDimensions(
            by: CGSize(width: 230, height: minimumContentHeight)
        )
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        guard let content = subviews.first, subviews.count > 1 else {
            subviews.first?.place(
                at: bounds.origin,
                proposal: ProposedViewSize(width: bounds.width, height: bounds.height)
            )
            return
        }

        let footer = subviews[1]
        let footerIdeal = footer.sizeThatFits(
            ProposedViewSize(width: bounds.width, height: nil)
        ).height
        let placed = SidebarColumnPlacement.heights(
            available: bounds.height,
            footerIdeal: footerIdeal,
            minimumContent: minimumContentHeight
        )

        content.place(
            at: bounds.origin,
            proposal: ProposedViewSize(width: bounds.width, height: placed.content)
        )
        footer.place(
            at: CGPoint(x: bounds.minX, y: bounds.minY + placed.content),
            proposal: ProposedViewSize(width: bounds.width, height: placed.footer)
        )
    }
}

/// Scroll views on newer systems inset their content on every edge. The foot
/// runs to the sides of the column, so the horizontal inset is removed.
///
/// The bottom inset stays. It is the window's status bar. Clearing it lays
/// the last control — Turn On, while the extension is not installed yet —
/// underneath that bar. The scroll view then believes the foot fits, so the
/// control cannot be scrolled back into sight either.
private struct ZeroScrollMargins: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 15, *) {
            content.contentMargins(.horizontal, 0, for: .scrollContent)
        } else {
            content
        }
    }
}
