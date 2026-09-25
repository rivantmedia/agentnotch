import CoreGraphics
import Testing
@testable import ClaudeControl

/// GUX-11: needs-you is a bar, the others dots, laid out edge to edge.
struct Fix_RestingMarkShapeTests {
    typealias Layout = ClaudeRestingMarkLayout

    @Test func needsYouIsLongerThanADot() {
        #expect(Layout.length(of: .needsYou) > Layout.length(of: .working))
        #expect(Layout.length(of: .review) == Layout.dotDiameter)
    }

    @Test func marksAreCentredAndAGapApart() {
        #expect(Layout.alongOffsets(marks: []).isEmpty)
        #expect(Layout.alongOffsets(marks: [.needsYou]) == [0])
        let offsets = Layout.alongOffsets(marks: [.needsYou, .review, .working])
        // Symmetric run.
        let first = offsets[0] - Layout.needsYouLength / 2
        let last = offsets[2] + Layout.dotDiameter / 2
        #expect(abs(first + last) < 0.001)
        // Edge to edge, a gap apart.
        #expect(abs((offsets[1] - Layout.dotDiameter / 2) - (offsets[0] + Layout.needsYouLength / 2) - Layout.gap) < 0.001)
        #expect(abs((offsets[2] - Layout.dotDiameter / 2) - (offsets[1] + Layout.dotDiameter / 2) - Layout.gap) < 0.001)
        // Dots only: as before.
        #expect(Layout.alongOffsets(marks: [.review, .working]) == Layout.alongOffsets(count: 2))
    }

    @Test func allThreeStillFitTheSmallestPill() {
        #expect(Layout.fits(marks: [.needsYou, .review, .working], pillLength: 40, visibleDepth: 6))
        #expect(!Layout.fits(marks: [.needsYou, .review, .working], pillLength: 20, visibleDepth: 6))
    }
}
