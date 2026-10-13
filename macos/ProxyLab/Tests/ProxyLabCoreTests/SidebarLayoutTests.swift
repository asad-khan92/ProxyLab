//  The traffic sidebar's column: a list, and a footer pinned under it.
//
//  The footer is what a short window used to draw past the bottom of the
//  column. These tests are the arithmetic of that split, and one drawing of
//  it, in the same spirit as the other view smoke tests: a body that traps
//  or ignores the height it was given fails here. Not a screenshot.

import AppKit
import SwiftUI
import Testing

@testable import ProxyLabCore

@Suite("Sidebar column placement")
struct SidebarLayoutTests {
    @Test("a footer that fits stays at the bottom and the list takes the rest")
    func footerFits() {
        let placed = SidebarColumnPlacement.heights(
            available: 700,
            footerIdeal: 420,
            minimumContent: 120
        )

        #expect(placed.content == 280)
        #expect(placed.footer == 420)
    }

    @Test("a footer taller than the column scrolls and the list keeps its minimum")
    func footerScrolls() {
        let placed = SidebarColumnPlacement.heights(
            available: 360,
            footerIdeal: 640,
            minimumContent: 120
        )

        #expect(placed.content == 120)
        #expect(placed.footer == 240)
    }

    @Test("a very short column still gives the footer most of the height")
    func veryShortColumn() {
        let placed = SidebarColumnPlacement.heights(
            available: 180,
            footerIdeal: 640,
            minimumContent: 120
        )

        #expect(placed.content > 0)
        #expect(placed.content < 120)
        #expect(placed.footer > placed.content)
        #expect(abs((placed.content + placed.footer) - 180) < 0.5)
    }

    @Test("no height is left over or drawn past the column")
    func heightsAddUp() {
        for available in [0, 80, 240, 560, 900] {
            for footer in [0, 100, 400, 1_200] {
                let placed = SidebarColumnPlacement.heights(
                    available: CGFloat(available),
                    footerIdeal: CGFloat(footer),
                    minimumContent: 120
                )

                #expect(placed.content >= 0)
                #expect(placed.footer >= 0)
                #expect(abs((placed.content + placed.footer) - CGFloat(available)) < 0.5)
            }
        }
    }
}

@MainActor
@Suite("Sidebar column, drawn")
struct SidebarColumnRenderingTests {
    @Test("the column takes the height it is given, short or tall")
    func fillsTheProposedHeight() {
        let short = measured(height: 280)
        let tall = measured(height: 760)

        #expect(short.width == 230)
        #expect(short.height == 280)
        #expect(tall.height == 760)
    }

    @Test("the column draws when the footer is taller than the height it was given")
    func drawsWhenTheFooterOverflows() throws {
        let column = BottomPinnedColumn(minimumContentHeight: 120) {
            Text("All Traffic")
        } footer: {
            VStack(alignment: .leading, spacing: 8) {
                Text("SAVED REQUESTS")
                Text("iOS SIMULATOR")
                Text("ANDROID EMULATOR")
                Text("IPHONE")
                Text("THIS MAC")
                Button("Turn On") {}
            }
            .padding(12)
        }

        for height in [280.0, 760.0] {
            let drawn = bitmap(column.frame(width: 230, height: CGFloat(height)))
            #expect(drawn != nil)
            #expect((drawn?.pixelsHigh ?? 0) > 0)
        }
    }

    private func measured(height: CGFloat) -> CGSize {
        let box = MeasuredSize()
        let view = MeasureProposal(box: box, width: 230, height: height) {
            BottomPinnedColumn(minimumContentHeight: 120) {
                Color.blue.frame(height: 40)
            } footer: {
                Color.green.frame(height: 480)
            }
        }
        let renderer = ImageRenderer(content: view)
        renderer.scale = 1
        _ = renderer.nsImage

        return box.size
    }

    private func bitmap(_ view: some View) -> NSBitmapImageRep? {
        let renderer = ImageRenderer(content: view)
        renderer.scale = 1

        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation else { return nil }

        return NSBitmapImageRep(data: tiff)
    }
}

private final class MeasuredSize: @unchecked Sendable {
    var size: CGSize = .zero
}

private struct MeasureProposal: Layout {
    let box: MeasuredSize
    let width: CGFloat
    let height: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let size = subviews[0].sizeThatFits(ProposedViewSize(width: width, height: height))
        box.size = size

        return size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews[0].place(
            at: bounds.origin,
            proposal: ProposedViewSize(width: width, height: height)
        )
    }
}

@Suite("Sidebar host selection")
struct SidebarHostSelectionTests {
    @Test("a selection written while rows are inserted does not narrow traffic")
    func layoutWriteIsIgnored() {
        #expect(SidebarHostSelection.shouldCommit(nil) == false)
        #expect(SidebarHostSelection.shouldCommit(.mouseMoved) == false)
        #expect(SidebarHostSelection.shouldCommit(.appKitDefined) == false)
    }

    @Test("a click or a key still chooses the host")
    func clickAndKeyCommit() {
        #expect(SidebarHostSelection.shouldCommit(.leftMouseDown))
        #expect(SidebarHostSelection.shouldCommit(.leftMouseUp))
        #expect(SidebarHostSelection.shouldCommit(.rightMouseUp))
        #expect(SidebarHostSelection.shouldCommit(.keyDown))
    }
}
