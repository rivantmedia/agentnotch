import CoreGraphics
import Testing
@testable import ClaudeControl

/// Where the needs-you and review badges sit on a Claude ring, on every edge
/// and in the strip beside the camera, and that they stay clear of what is
/// around the ring: the notch's own edges, the percent label and the next
/// ring.
///
/// The cell is Codenotch's design frame in points (`NotchLayout`, which the
/// package cannot import): a 44 pt ring, a 10.1 pt gap to its label, a 70 pt
/// deep body on the sides with the ring centred in it, 31.4 pt between cells.
/// `Tests/ClaudeBridgeTests.swift` repeats the check against the real
/// `NotchLayout` in the app's own test target.
struct C_RingBadgeLayoutTests {
    let ring: CGFloat = 44
    let labelGap: CGFloat = 26.9 * 44 / 117
    /// Taller than the percent line really is, so the check has slack.
    let labelHeight: CGFloat = 16
    let sideBodyDepth: CGFloat = 186 * 44 / 117
    let cellSpacing: CGFloat = 83.5 * 44 / 117
    var ringMargin: CGFloat { (sideBodyDepth - ring) / 2 }
    let edges: [ClaudePanelEdge] = [.right, .left, .top, .bottom]
    let slots: [ClaudeRingBadgeSlot] = [.needsYou, .review]

    /// A badge's footprint including the knockout ring drawn round it.
    private func footprint(_ slot: ClaudeRingBadgeSlot, _ edge: ClaudePanelEdge, compact: Bool = false) -> CGRect {
        ClaudeRingBadgeLayout.frame(slot, edge: edge, compact: compact, ringDiameter: ring)
            .insetBy(dx: -ClaudeRingBadgeLayout.knockout, dy: -ClaudeRingBadgeLayout.knockout)
    }

    // MARK: - Angles

    @Test func angleTableMatchesTheDesign() {
        let expected: [(ClaudePanelEdge, Double, Double)] = [
            (.right, 225, 135), (.left, 315, 45), (.top, 135, 45), (.bottom, 225, 315),
        ]
        for (edge, needsYou, review) in expected {
            #expect(ClaudeRingBadgeLayout.angle(.needsYou, edge: edge) == needsYou, "\(edge)")
            #expect(ClaudeRingBadgeLayout.angle(.review, edge: edge) == review, "\(edge)")
        }
    }

    /// Always on the side facing the middle of the screen: left of the ring on
    /// the right edge, right of it on the left edge, below it at the top,
    /// above it at the bottom. And the two badges never share a spot.
    @Test func badgesFaceTheScreenCentre() {
        let half = ring / 2
        for compact in [false, true] {
            for edge in edges {
                let needsYou = ClaudeRingBadgeLayout.center(.needsYou, edge: edge, compact: compact, ringDiameter: ring)
                let review = ClaudeRingBadgeLayout.center(.review, edge: edge, compact: compact, ringDiameter: ring)
                for point in [needsYou, review] {
                    switch edge {
                    case .right: #expect(point.x < half, "\(edge) compact=\(compact)")
                    case .left: #expect(point.x > half, "\(edge) compact=\(compact)")
                    case .top: #expect(point.y > half, "\(edge) compact=\(compact)")
                    case .bottom: #expect(point.y < half, "\(edge) compact=\(compact)")
                    }
                }
                #expect(!footprint(.needsYou, edge, compact: compact)
                    .intersects(footprint(.review, edge, compact: compact)), "\(edge) compact=\(compact)")
            }
        }
    }

    /// On a side edge needs-you is the upper badge, the one read first going
    /// down the stack; on the top and bottom edges it is the leading one.
    @Test func needsYouLeadsInReadingOrder() {
        for edge in edges {
            let needsYou = ClaudeRingBadgeLayout.center(.needsYou, edge: edge, compact: false, ringDiameter: ring)
            let review = ClaudeRingBadgeLayout.center(.review, edge: edge, compact: false, ringDiameter: ring)
            switch edge {
            case .right, .left: #expect(needsYou.y < review.y, "\(edge)")
            case .top, .bottom: #expect(needsYou.x < review.x, "\(edge)")
            }
        }
    }

    // MARK: - Clearance

    /// GUX-12: a count badge sits outside both arcs, so neither the session
    /// arc's nor the weekly ring's tip is ever under it: its nearest point
    /// (knockout included) is past the weekly ring's outer edge.
    @Test func countBadgesClearTheWeeklyRing() {
        let weeklyOuterEdge = 65 * 44 / 117 + 5 * 44 / 117 / 2   // weeklyOutsideRadius + stroke / 2
        for edge in edges {
            for slot in slots {
                let centre = ClaudeRingBadgeLayout.center(slot, edge: edge, compact: false, ringDiameter: ring)
                let distance = hypot(centre.x - ring / 2, centre.y - ring / 2)
                #expect(abs(distance - (ring / 2 + 11)) < 0.001, "\(edge) \(slot)")
                let nearest = distance - ClaudeRingBadgeLayout.badgeHeight / 2 - ClaudeRingBadgeLayout.knockout
                #expect(nearest >= CGFloat(weeklyOuterEdge), "\(edge) \(slot): \(nearest)")
            }
        }
        // Proportional for another ring size.
        let big = ClaudeRingBadgeLayout.center(.review, edge: .right, compact: false, ringDiameter: 88)
        #expect(abs(hypot(big.x - 44, big.y - 44) - 66) < 0.001)
    }

    /// Side edges: the badge stays inside the body's depth, on its inner side,
    /// and clear of this ring's label below and the previous cell's label above.
    @Test func sideEdgeBadgesClearTheBodyAndTheLabels() {
        let centre = ring / 2
        for edge in [ClaudePanelEdge.right, .left] {
            // The body spans `sideBodyDepth` across, centred on the ring.
            let bodyMin = centre - sideBodyDepth / 2
            let bodyMax = centre + sideBodyDepth / 2
            let labelTop = ring + labelGap
            let previousLabelBottom = -cellSpacing
            for slot in slots {
                let badge = footprint(slot, edge)
                #expect(badge.minX >= bodyMin && badge.maxX <= bodyMax, "\(edge) \(slot): \(badge)")
                #expect(badge.maxY < labelTop, "\(edge) \(slot) runs into the label: \(badge)")
                #expect(badge.minY > previousLabelBottom, "\(edge) \(slot) runs into the cell above: \(badge)")
            }
        }
    }

    /// Top and bottom edges: the badge keeps to its own cell along the stack
    /// (at most half the gap to the next ring) and clear of the label.
    @Test func horizontalEdgeBadgesClearTheLabelAndTheNeighbours() {
        let labelTop = ring + labelGap
        for edge in [ClaudePanelEdge.top, .bottom] {
            for slot in slots {
                let badge = footprint(slot, edge)
                #expect(badge.minX > -cellSpacing / 2 && badge.maxX < ring + cellSpacing / 2, "\(edge) \(slot): \(badge)")
                #expect(badge.maxY < labelTop, "\(edge) \(slot) runs into the label: \(badge)")
                // Inside the body: `ringMargin` of body beyond the ring on the
                // inner side (above it at the bottom; the label is below).
                #expect(badge.minY > -ringMargin, "\(edge) \(slot) leaves the body: \(badge)")
            }
        }
    }

    /// Beside the camera the strip is barely deeper than the ring, so the dot
    /// has to stay inside the ring's own square.
    @Test func compactDotsStayInsideTheRing() {
        let square = CGRect(x: 0, y: 0, width: ring, height: ring)
        for edge in edges {
            for slot in slots {
                let dot = ClaudeRingBadgeLayout.frame(slot, edge: edge, compact: true, ringDiameter: ring)
                #expect(dot.width == ClaudeRingBadgeLayout.dotDiameter && dot.height == ClaudeRingBadgeLayout.dotDiameter)
                #expect(square.contains(dot), "\(edge) \(slot): \(dot)")
                let centre = ClaudeRingBadgeLayout.center(slot, edge: edge, compact: true, ringDiameter: ring)
                #expect(abs(hypot(centre.x - ring / 2, centre.y - ring / 2) - ring / 2) < 0.001)
            }
        }
    }

    /// Wide counts are capped, narrow ones are still round.
    @Test func badgeWidthIsBounded() {
        let narrow = ClaudeRingBadgeLayout.frame(.needsYou, edge: .right, compact: false, ringDiameter: ring, badgeWidth: 4)
        #expect(narrow.width == ClaudeRingBadgeLayout.badgeHeight)
        let wide = ClaudeRingBadgeLayout.frame(.needsYou, edge: .right, compact: false, ringDiameter: ring, badgeWidth: 60)
        #expect(wide.width == ClaudeRingBadgeLayout.maxBadgeWidth)
        #expect(wide.height == ClaudeRingBadgeLayout.badgeHeight)
    }

    // MARK: - Label

    @Test func labelCountsAndCaps() {
        #expect(ClaudeRingBadgeLayout.label(count: 0) == nil)
        #expect(ClaudeRingBadgeLayout.label(count: -1) == nil)
        #expect(ClaudeRingBadgeLayout.label(count: 1) == "1")
        #expect(ClaudeRingBadgeLayout.label(count: 9) == "9")
        #expect(ClaudeRingBadgeLayout.label(count: 10) == "9+")
        #expect(ClaudeRingBadgeLayout.label(count: 250) == "9+")
    }
}
