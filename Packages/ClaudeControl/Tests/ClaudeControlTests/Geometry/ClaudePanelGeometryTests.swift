import CoreGraphics
import Testing
@testable import ClaudeControl

/// The sessions panel's placement (design §7): every edge with the ring near
/// the start, the middle and the end of the notch; the split top around the
/// camera; screen corners; small screens; content taller than the maximum;
/// the tail clamped out of the corners; list and chat widths; the floating
/// fallback. Screen coordinates, y up, as AppKit gives them.
struct ClaudePanelGeometryTests {
    /// Codenotch's metrics: `NotchLayout.tailLength`, `tailHeight` (the tail's
    /// width across the card), `cardCorner`.
    let chrome = ClaudePanelChrome(tailLength: 28.2, tailWidth: 32.7, corner: 18.6)
    /// A 14" MacBook Pro: 1512 x 982 with a 38 pt menu bar.
    let screen = CGRect(x: 0, y: 0, width: 1512, height: 982)
    let visible = CGRect(x: 0, y: 0, width: 1512, height: 944)
    /// Bezel to tail tip: `notchDrawnDepth + tailGap` at medium size.
    let inset: CGFloat = 80.5

    // MARK: - Helpers

    func place(_ anchor: ClaudePanelAnchor?, _ mode: ClaudePanelMode = .list, ideal: CGFloat = 400,
               fallback: CGRect? = nil) -> ClaudePanelPlacement {
        ClaudePanelGeometry.place(anchor: anchor, mode: mode, idealContentHeight: ideal, chrome: chrome,
                                  fallbackVisibleFrame: fallback ?? visible)
    }

    func expectInside(_ rect: CGRect, _ bounds: CGRect, margin: CGFloat = 0,
                      sourceLocation: SourceLocation = #_sourceLocation) {
        let inner = bounds.insetBy(dx: margin, dy: margin).insetBy(dx: -0.001, dy: -0.001)
        #expect(inner.contains(rect), "\(rect) is not inside \(inner)", sourceLocation: sourceLocation)
    }

    func expectClose(_ a: CGFloat, _ b: CGFloat, sourceLocation: SourceLocation = #_sourceLocation) {
        #expect(abs(a - b) < 0.001, "\(a) != \(b)", sourceLocation: sourceLocation)
    }

    func expectClose(_ a: CGPoint?, _ b: CGPoint, sourceLocation: SourceLocation = #_sourceLocation) {
        guard let a else {
            Issue.record("no tail tip", sourceLocation: sourceLocation)
            return
        }
        #expect(abs(a.x - b.x) < 0.001 && abs(a.y - b.y) < 0.001, "\(a) != \(b)", sourceLocation: sourceLocation)
    }

    /// The panel window holds the card and the tail, and nothing else.
    func expectWindowWrapsCard(_ p: ClaudePanelPlacement, edge: ClaudePanelEdge,
                               sourceLocation: SourceLocation = #_sourceLocation) {
        #expect(p.windowFrame.contains(p.cardRect), sourceLocation: sourceLocation)
        let isSide = edge == .right || edge == .left
        expectClose(isSide ? p.windowFrame.width : p.windowFrame.height,
                    (isSide ? p.cardRect.width : p.cardRect.height) + chrome.tailLength,
                    sourceLocation: sourceLocation)
        #expect(abs(p.tailOffset) <= ClaudePanelGeometry.tailOffsetLimit(
            cardLength: isSide ? p.cardRect.height : p.cardRect.width, chrome: chrome) + 0.001,
                sourceLocation: sourceLocation)
    }

    // A side notch: 334 wide (body plus the reserved tooltip depth), 782 tall.
    func sideNotch(_ edge: ClaudePanelEdge) -> CGRect {
        edge == .right ? CGRect(x: 1512 - 334, y: 100, width: 334, height: 782)
                       : CGRect(x: 0, y: 100, width: 334, height: 782)
    }

    // A top or bottom bar: 700 long, 300 deep.
    func barNotch(_ edge: ClaudePanelEdge) -> CGRect {
        edge == .top ? CGRect(x: 406, y: 982 - 300, width: 700, height: 300)
                     : CGRect(x: 406, y: 0, width: 700, height: 300)
    }

    // MARK: - Side edges

    @Test(arguments: [ClaudePanelEdge.right, .left], [CGFloat(60), 391, 720])
    func sideEdgeHangsOffTheRingOnTheInnerSide(edge: ClaudePanelEdge, along: CGFloat) {
        let notch = sideNotch(edge)
        let anchor = ClaudePanelAnchor(edge: edge, notchWindowFrame: notch, ringAlong: along,
                                       tailTipInset: inset, visibleFrame: visible)
        let p = place(anchor)
        let ring = ClaudePanelGeometry.ringPoint(anchor)
        #expect(p.hasTail)
        #expect(p.contentWidth == 400)
        expectWindowWrapsCard(p, edge: edge)
        expectInside(p.cardRect, visible, margin: chrome.margin)
        // The tail's tip lands on the ring, `tooltipInset` in from the bezel.
        expectClose(ClaudePanelGeometry.tailTip(of: p, edge: edge), ring)
        if edge == .right {
            expectClose(p.windowFrame.maxX, notch.maxX - inset)
            expectClose(p.cardRect.maxX, notch.maxX - inset - chrome.tailLength)
        } else {
            expectClose(p.windowFrame.minX, notch.minX + inset)
            expectClose(p.cardRect.minX, notch.minX + inset + chrome.tailLength)
        }
        // Centred on the ring unless the screen's edge is in the way.
        if ring.y - 200 >= visible.minY + chrome.margin, ring.y + 200 <= visible.maxY - chrome.margin {
            expectClose(p.cardRect.midY, ring.y)
            expectClose(p.tailOffset, 0)
        }
    }

    @Test func sideEdgeRingNearTheTopSlidesTheTailUp() {
        let anchor = ClaudePanelAnchor(edge: .right, notchWindowFrame: sideNotch(.right), ringAlong: 60,
                                       tailTipInset: inset, visibleFrame: visible)
        let p = place(anchor)
        // Card pinned under the top margin; the tail moves up along it.
        expectClose(p.cardRect.maxY, visible.maxY - chrome.margin)
        #expect(p.tailOffset < 0)
    }

    @Test func sideEdgeRingAtTheVeryCornerClampsTheTailOutOfTheRoundedCorner() {
        let notch = CGRect(x: 1512 - 334, y: 0, width: 334, height: 982)
        let anchor = ClaudePanelAnchor(edge: .right, notchWindowFrame: notch, ringAlong: 2,
                                       tailTipInset: inset, visibleFrame: visible)
        let p = place(anchor)
        let limit = ClaudePanelGeometry.tailOffsetLimit(cardLength: p.cardRect.height, chrome: chrome)
        expectClose(p.tailOffset, -limit)
        expectInside(p.cardRect, visible, margin: chrome.margin)
    }

    @Test func sideEdgeChatIsWiderButNeverWiderThanTheRoomBesideTheNotch() {
        let anchor = ClaudePanelAnchor(edge: .left, notchWindowFrame: sideNotch(.left), ringAlong: 391,
                                       tailTipInset: inset, visibleFrame: visible)
        #expect(place(anchor, .chat).contentWidth == 440)
        let narrow = CGRect(x: 0, y: 0, width: 500, height: 800)
        let squeezed = ClaudePanelAnchor(edge: .left, notchWindowFrame: CGRect(x: 0, y: 0, width: 334, height: 800),
                                         ringAlong: 400, tailTipInset: inset, visibleFrame: narrow)
        let p = place(squeezed, .chat)
        expectClose(p.contentWidth, narrow.maxX - chrome.margin - (inset + chrome.tailLength))
        expectInside(p.cardRect, narrow, margin: chrome.margin)
    }

    @Test func sideEdgeCardStaysOutOfADockOnTheSameSide() {
        // A Dock on the right leaves the visible frame 132 pt short of the
        // bezel, under where the tail's root would be.
        let docked = CGRect(x: 0, y: 0, width: 1380, height: 944)
        let anchor = ClaudePanelAnchor(edge: .right, notchWindowFrame: sideNotch(.right), ringAlong: 391,
                                       tailTipInset: inset, visibleFrame: docked)
        let p = place(anchor)
        expectInside(p.cardRect, docked, margin: chrome.margin)
        expectWindowWrapsCard(p, edge: .right)
    }

    // MARK: - Top and bottom

    @Test(arguments: [ClaudePanelEdge.top, .bottom], [CGFloat(40), 350, 660])
    func barHangsOffTheRingOnTheInnerSide(edge: ClaudePanelEdge, along: CGFloat) {
        let notch = barNotch(edge)
        let anchor = ClaudePanelAnchor(edge: edge, notchWindowFrame: notch, ringAlong: along,
                                       tailTipInset: inset, visibleFrame: visible)
        let p = place(anchor)
        let ring = ClaudePanelGeometry.ringPoint(anchor)
        #expect(p.hasTail)
        #expect(p.contentWidth == 440)
        expectWindowWrapsCard(p, edge: edge)
        expectInside(p.cardRect, visible, margin: chrome.margin)
        expectClose(ClaudePanelGeometry.tailTip(of: p, edge: edge), ring)
        if edge == .top {
            expectClose(p.windowFrame.maxY, notch.maxY - inset)
            expectClose(p.cardRect.maxY, notch.maxY - inset - chrome.tailLength)
        } else {
            expectClose(p.windowFrame.minY, notch.minY + inset)
            expectClose(p.cardRect.minY, notch.minY + inset + chrome.tailLength)
        }
        expectClose(p.cardRect.midX, ring.x)
    }

    @Test func splitTopAroundTheCameraHangsJustUnderTheMenuBar() {
        // Beside the camera the bar is the hardware's depth (38 pt), so
        // `tooltipInset` is 38 + tailGap. The rings sit in the two ears.
        let notch = CGRect(x: 356, y: 982 - 330, width: 800, height: 330)
        let splitInset: CGFloat = 38 + 10.5
        for along in [CGFloat(250), 560] {
            let anchor = ClaudePanelAnchor(edge: .top, notchWindowFrame: notch, ringAlong: along,
                                           tailTipInset: splitInset, visibleFrame: visible)
            let p = place(anchor)
            expectClose(p.windowFrame.maxY, screen.maxY - splitInset)
            #expect(p.cardRect.maxY <= visible.maxY - chrome.margin)
            #expect(p.cardRect.maxY > visible.maxY - 40, "the card starts right under the menu bar")
            expectClose(ClaudePanelGeometry.tailTip(of: p, edge: .top), ClaudePanelGeometry.ringPoint(anchor))
            expectInside(p.cardRect, visible, margin: chrome.margin)
        }
    }

    @Test func barRingAtTheScreenCornerClampsTheCardAndTheTail() {
        let notch = CGRect(x: -150, y: 982 - 300, width: 700, height: 300)
        let anchor = ClaudePanelAnchor(edge: .top, notchWindowFrame: notch, ringAlong: 160,
                                       tailTipInset: inset, visibleFrame: visible)
        let p = place(anchor)
        expectClose(p.cardRect.minX, visible.minX + chrome.margin)
        let limit = ClaudePanelGeometry.tailOffsetLimit(cardLength: p.cardRect.width, chrome: chrome)
        expectClose(p.tailOffset, -limit)

        let right = ClaudePanelAnchor(edge: .bottom, notchWindowFrame: CGRect(x: 1100, y: 0, width: 700, height: 300),
                                      ringAlong: 400, tailTipInset: inset, visibleFrame: visible)
        let q = place(right)
        expectClose(q.cardRect.maxX, visible.maxX - chrome.margin)
        expectClose(q.tailOffset, ClaudePanelGeometry.tailOffsetLimit(cardLength: q.cardRect.width, chrome: chrome))
    }

    @Test func barHeightIsBoundedByTheRoomBeyondTheNotch() {
        let top = ClaudePanelAnchor(edge: .top, notchWindowFrame: barNotch(.top), ringAlong: 350,
                                    tailTipInset: inset, visibleFrame: visible)
        let p = place(top, .chat, ideal: 5000)
        expectClose(p.cardRect.height, 780)
        expectClose(p.maxContentHeight, 780)
        #expect(p.contentWidth == 520)

        let short = CGRect(x: 0, y: 0, width: 1280, height: 600)
        let bottom = ClaudePanelAnchor(edge: .bottom, notchWindowFrame: CGRect(x: 290, y: 0, width: 700, height: 300),
                                       ringAlong: 350, tailTipInset: inset, visibleFrame: short)
        let q = place(bottom, .chat, ideal: 5000)
        expectClose(q.maxContentHeight, short.maxY - chrome.margin - (inset + chrome.tailLength))
        expectClose(q.cardRect.height, q.maxContentHeight)
        expectInside(q.cardRect, short, margin: chrome.margin)
    }

    // MARK: - Heights

    @Test(arguments: [ClaudePanelEdge.right, .left, .top, .bottom])
    func heightStaysBetweenTheMinimumAndTheMaximum(edge: ClaudePanelEdge) {
        let notch = edge == .right || edge == .left ? sideNotch(edge) : barNotch(edge)
        let anchor = ClaudePanelAnchor(edge: edge, notchWindowFrame: notch, ringAlong: 350,
                                       tailTipInset: inset, visibleFrame: visible)
        for mode in [ClaudePanelMode.list, .chat] {
            let small = place(anchor, mode, ideal: 10)
            #expect(small.cardRect.height == ClaudePanelGeometry.minimumHeight)
            let tall = place(anchor, mode, ideal: 5000)
            expectClose(tall.cardRect.height, tall.maxContentHeight)
            #expect(tall.maxContentHeight <= ClaudePanelGeometry.heightCap(mode))
            expectInside(tall.cardRect, visible, margin: chrome.margin)
            let fits = place(anchor, mode, ideal: 333)
            expectClose(fits.cardRect.height, 333)
        }
    }

    @Test func aNonFiniteHeightFallsBackToTheMinimum() {
        let anchor = ClaudePanelAnchor(edge: .right, notchWindowFrame: sideNotch(.right), ringAlong: 391,
                                       tailTipInset: inset, visibleFrame: visible)
        for ideal in [CGFloat.nan, .infinity] {
            let p = place(anchor, ideal: ideal)
            #expect(p.cardRect.height == ClaudePanelGeometry.minimumHeight)
            #expect(p.windowFrame.origin.x.isFinite && p.windowFrame.origin.y.isFinite)
        }
    }

    @Test func smallScreensKeepTheCardOnScreen() {
        let small = CGRect(x: 0, y: 0, width: 800, height: 560)
        for edge in [ClaudePanelEdge.right, .left, .top, .bottom] {
            let notch: CGRect
            switch edge {
            case .right: notch = CGRect(x: 800 - 334, y: 0, width: 334, height: 560)
            case .left: notch = CGRect(x: 0, y: 0, width: 334, height: 560)
            case .top: notch = CGRect(x: 50, y: 560 - 300, width: 700, height: 300)
            case .bottom: notch = CGRect(x: 50, y: 0, width: 700, height: 300)
            }
            let anchor = ClaudePanelAnchor(edge: edge, notchWindowFrame: notch, ringAlong: 280,
                                           tailTipInset: inset, visibleFrame: small)
            for mode in [ClaudePanelMode.list, .chat] {
                let p = place(anchor, mode, ideal: 5000)
                expectInside(p.cardRect, small, margin: chrome.margin)
                expectWindowWrapsCard(p, edge: edge)
            }
        }
    }

    @Test func aScreenShorterThanTheMinimumKeepsTheCardsTopOnScreen() {
        let tiny = CGRect(x: 0, y: 0, width: 800, height: 200)
        let anchor = ClaudePanelAnchor(edge: .right, notchWindowFrame: CGRect(x: 466, y: 0, width: 334, height: 200),
                                       ringAlong: 100, tailTipInset: inset, visibleFrame: tiny)
        let p = place(anchor)
        #expect(p.cardRect.height == ClaudePanelGeometry.minimumHeight)
        expectClose(p.cardRect.maxY, tiny.maxY - chrome.margin)
    }

    // MARK: - Widths

    @Test func listAndChatWidths() {
        #expect(ClaudePanelGeometry.width(edge: .right, mode: .list) == 400)
        #expect(ClaudePanelGeometry.width(edge: .left, mode: .chat) == 440)
        #expect(ClaudePanelGeometry.width(edge: .top, mode: .list) == 440)
        #expect(ClaudePanelGeometry.width(edge: .bottom, mode: .chat) == 520)
        #expect(ClaudePanelGeometry.width(edge: nil, mode: .list) == 440)
        #expect(ClaudePanelGeometry.width(edge: nil, mode: .chat) == 520)
    }

    // MARK: - Floating

    @Test func noAnchorFloatsWithoutATailUnderTheMenuBar() {
        for (mode, width) in [(ClaudePanelMode.list, CGFloat(440)), (.chat, 520)] {
            let p = place(nil, mode, ideal: 300)
            #expect(!p.hasTail)
            #expect(p.windowFrame == p.cardRect)
            #expect(p.tailOffset == 0)
            #expect(p.contentWidth == width)
            expectClose(p.cardRect.midX, visible.midX)
            expectClose(p.cardRect.maxY, visible.maxY - chrome.margin)
            expectClose(p.maxContentHeight, visible.height - 2 * chrome.margin)
            #expect(ClaudePanelGeometry.tailTip(of: p, edge: .right) == nil)
        }
    }

    @Test func floatingUsesThePointersScreen() {
        let second = CGRect(x: 1512, y: -200, width: 1920, height: 1055)
        let p = place(nil, .list, ideal: 5000, fallback: second)
        expectInside(p.cardRect, second, margin: chrome.margin)
        expectClose(p.cardRect.midX, second.midX)
        expectClose(p.cardRect.height, second.height - 2 * chrome.margin)
    }
}
