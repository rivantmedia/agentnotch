import CoreGraphics
import Foundation
import Testing
@testable import ClaudeControl

/// The burst gate that turns transitions arriving together into one chime and
/// one peek, and the folded notch's row of dots.
struct C_BurstAndRestingMarkTests {
    typealias Policy = ClaudeAttentionPolicy
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func at(_ seconds: TimeInterval) -> Date { t0.addingTimeInterval(seconds) }

    // MARK: - Burst

    @Test func theFirstTransitionOpensABurstAndTheRestJoinIt() {
        var burst = Policy.Burst()
        #expect(burst.begin(at: at(0)) == at(Policy.burstWindow))
        #expect(burst.begin(at: at(0.1)) == nil)
        #expect(burst.begin(at: at(0.4)) == nil)
        #expect(burst.pending == 3)
    }

    @Test func aBurstClosesOnceItsWindowHasPassedAndEveryDecisionIsIn() {
        var burst = Policy.Burst()
        _ = burst.begin(at: at(0))
        _ = burst.begin(at: at(0.2))
        burst.finish([.chime(.finished), .peek(pid: 1, kind: .readyForReview)])
        // Too early, and one decision still out.
        #expect(burst.close(at: at(0.3)) == nil)
        #expect(burst.close(at: at(0.6)) == nil)
        burst.finish([.chime(.finished), .peek(pid: 2, kind: .readyForReview)])
        // Every decision is in, but the window is still open.
        var early = burst
        #expect(early.close(at: at(0.49)) == nil)
        let merged = burst.close(at: at(0.6))
        #expect(merged == [.chime(.finished), .peek(pid: 2, kind: .readyForReview)])
    }

    /// A decision that takes longer than the window still lands in its own
    /// burst rather than making a second chime.
    @Test func aSlowDecisionHoldsTheBurstOpen() {
        var burst = Policy.Burst()
        _ = burst.begin(at: at(0))
        #expect(burst.close(at: at(0.5)) == nil)
        #expect(burst.close(at: at(3)) == nil)
        burst.finish([.chime(.needsInput), .autoOpen(sessionID: "s", kind: .needsInput)])
        #expect(burst.close(at: at(3)) == [.chime(.needsInput), .autoOpen(sessionID: "s", kind: .needsInput)])
    }

    @Test func closingStartsAfresh() {
        var burst = Policy.Burst()
        _ = burst.begin(at: at(0))
        burst.finish([.chime(.finished)])
        #expect(burst.close(at: at(1)) == [.chime(.finished)])
        #expect(burst.openedAt == nil && burst.pending == 0 && burst.decisions.isEmpty)
        // Nothing left to close, and the next transition opens a new burst.
        #expect(burst.close(at: at(2)) == nil)
        #expect(burst.begin(at: at(5)) == at(5 + Policy.burstWindow))
    }

    /// A burst whose transitions all decided on nothing closes empty; the
    /// caller then does nothing.
    @Test func aBurstOfNothingClosesEmpty() {
        var burst = Policy.Burst()
        _ = burst.begin(at: at(0))
        burst.finish([])
        #expect(burst.close(at: at(1)) == [])
    }

    @Test func finishingMoreThanWasBegunNeverGoesNegative() {
        var burst = Policy.Burst()
        burst.finish([])
        #expect(burst.pending == 0)
    }

    // MARK: - Resting marks

    @Test func dotsAreCentredOnThePill() {
        #expect(ClaudeRestingMarkLayout.alongOffsets(count: 0).isEmpty)
        #expect(ClaudeRestingMarkLayout.alongOffsets(count: 1) == [0])
        let pitch = ClaudeRestingMarkLayout.dotDiameter + ClaudeRestingMarkLayout.gap
        #expect(ClaudeRestingMarkLayout.alongOffsets(count: 2) == [-pitch / 2, pitch / 2])
        #expect(ClaudeRestingMarkLayout.alongOffsets(count: 3) == [-pitch, 0, pitch])
    }

    /// Codenotch's folded pill: 79 pt long, 9.8 pt deep, 2 pt of it past the
    /// bezel. Three dots fit at every notch size Settings offers (0.75–1.5).
    @Test func threeDotsFitThePillAtEverySize() {
        let pillLength: CGFloat = 210 * 44 / 117
        let pillDepth: CGFloat = 26 * 44 / 117
        for scale in [CGFloat(0.75), 0.8, 1, 1.25, 1.5] {
            let visible = pillDepth * scale - 2
            #expect(ClaudeRestingMarkLayout.fits(count: 3, pillLength: pillLength * scale, visibleDepth: visible), "\(scale)")
            let across = ClaudeRestingMarkLayout.acrossCentre(drawnDepth: pillDepth * scale, bleed: 2)
            #expect(across - ClaudeRestingMarkLayout.dotDiameter / 2 >= 0.5, "\(scale)")
            #expect(across + ClaudeRestingMarkLayout.dotDiameter / 2 <= visible - 0.5, "\(scale)")
        }
        #expect(!ClaudeRestingMarkLayout.fits(count: 3, pillLength: 19, visibleDepth: 8))
        #expect(!ClaudeRestingMarkLayout.fits(count: 1, pillLength: 79, visibleDepth: 4.5))
        #expect(ClaudeRestingMarkLayout.acrossCentre(drawnDepth: 1, bleed: 2) == 0)
    }
}
