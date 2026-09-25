import Foundation
import Testing
@testable import ClaudeControl

/// What the notch does when a Claude session starts needing you or finishes:
/// the full matrix of auto-open policy × focused terminal × visible terminal ×
/// full screen × panel open (× the sound and peek switches), then the burst
/// merge, the hover-row click, the Dock badge and the folded-notch marks.
struct C_AttentionPolicyTests {
    typealias Policy = ClaudeAttentionPolicy

    private func transition(_ kind: ClaudeAttentionTransition.Kind, id: String = "s1", pid: Int32? = 4242) -> ClaudeAttentionTransition {
        let attention: ClaudeAttention
        switch kind {
        case .needsInput: attention = .needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash"))
        case .readyForReview: attention = .readyForReview
        case .resolved: attention = .working
        }
        return ClaudeAttentionTransition(kind: kind, session: ClaudeSessionSummary(
            id: id, ringID: "claude", pid: pid, title: "Fix the build", projectName: "acme",
            attention: attention, attentionSince: Date(timeIntervalSince1970: 1_800_000_000)))
    }

    private func context(
        _ autoOpen: AutoOpenPolicy = .needsInput,
        chimes: Bool = true, peeks: Bool = true, focused: Bool = false, visible: Bool = false,
        fullScreen: Bool = false, panelOpen: Bool = false, ringShown: Bool = true
    ) -> Policy.Context {
        Policy.Context(autoOpen: autoOpen, chimes: chimes, peeks: peeks, terminalFocused: focused,
                       anyTerminalVisible: visible, fullScreen: fullScreen, panelOpen: panelOpen,
                       ringShown: ringShown)
    }

    // MARK: - The matrix

    /// Every combination, checked against the rules one at a time.
    @Test(arguments: [ClaudeAttentionTransition.Kind.needsInput, .readyForReview, .resolved])
    func matrix(kind: ClaudeAttentionTransition.Kind) {
        let flags = [false, true]
        for autoOpen in AutoOpenPolicy.allCases {
            for focused in flags { for visible in flags { for fullScreen in flags { for panelOpen in flags {
            for chimes in flags { for peeks in flags { for ringShown in flags {
                let c = context(autoOpen, chimes: chimes, peeks: peeks, focused: focused, visible: visible,
                                fullScreen: fullScreen, panelOpen: panelOpen, ringShown: ringShown)
                let decisions = Policy.decide(transition(kind), context: c)
                let label = "\(kind) \(c)"

                // Resolved, or already looking at that terminal: nothing at all.
                if kind == .resolved || focused {
                    #expect(decisions.isEmpty, "\(label)")
                    continue
                }

                // Exactly one chime when sounds are on, of the right kind.
                let chimesMade = decisions.filter { if case .chime = $0 { return true }; return false }
                #expect(chimesMade.count == (chimes ? 1 : 0), "\(label)")
                if chimes {
                    #expect(decisions.first == .chime(kind == .needsInput ? .needsInput : .finished), "\(label)")
                }

                // Auto-open only when the policy covers the kind and nothing
                // is in the way; it replaces the peek.
                let covers = kind == .needsInput ? autoOpen != .never : autoOpen == .needsInputOrDone
                let opens = covers && !visible && !fullScreen && !panelOpen
                #expect(decisions.contains(.autoOpen(sessionID: "s1", kind: kind)) == opens, "\(label)")

                // Otherwise a peek, when peeks are on, the panel is closed and
                // the ring is in the notch.
                let peeksHere = !opens && peeks && !panelOpen && ringShown
                #expect(decisions.contains(.peek(pid: 4242, kind: kind)) == peeksHere, "\(label)")

                #expect(decisions.count == (chimes ? 1 : 0) + (opens ? 1 : 0) + (peeksHere ? 1 : 0), "\(label)")
            }}}}}}}
        }
    }

    // MARK: - The cases the design names

    @Test func needsInputWithNothingOnScreenOpensThePanel() {
        #expect(Policy.decide(transition(.needsInput), context: context(.needsInput))
            == [.chime(.needsInput), .autoOpen(sessionID: "s1", kind: .needsInput)])
    }

    @Test func needsInputBesideAVisibleTerminalPeeksInstead() {
        #expect(Policy.decide(transition(.needsInput), context: context(.needsInput, visible: true))
            == [.chime(.needsInput), .peek(pid: 4242, kind: .needsInput)])
    }

    @Test func needsInputOverAFullScreenAppPeeksInstead() {
        #expect(Policy.decide(transition(.needsInput), context: context(.needsInputOrDone, fullScreen: true))
            == [.chime(.needsInput), .peek(pid: 4242, kind: .needsInput)])
    }

    @Test func readyForReviewPeeksUnlessAutoOpenCoversIt() {
        #expect(Policy.decide(transition(.readyForReview), context: context(.needsInput))
            == [.chime(.finished), .peek(pid: 4242, kind: .readyForReview)])
        #expect(Policy.decide(transition(.readyForReview), context: context(.needsInputOrDone))
            == [.chime(.finished), .autoOpen(sessionID: "s1", kind: .readyForReview)])
    }

    @Test func anOpenPanelOnlyChimes() {
        #expect(Policy.decide(transition(.needsInput), context: context(.needsInputOrDone, panelOpen: true))
            == [.chime(.needsInput)])
    }

    @Test func aFocusedTerminalSilencesEverything() {
        #expect(Policy.decide(transition(.needsInput), context: context(.needsInputOrDone, focused: true)).isEmpty)
        #expect(Policy.decide(transition(.readyForReview), context: context(.needsInputOrDone, focused: true)).isEmpty)
    }

    @Test func aSessionWithoutAPidStillPeeks() {
        #expect(Policy.decide(transition(.readyForReview, pid: nil), context: context(.never))
            == [.chime(.finished), .peek(pid: nil, kind: .readyForReview)])
    }

    // MARK: - Bursts

    @Test func aBurstMakesOneChimeAndOnePeek() {
        let merged = Policy.merge([
            .chime(.finished), .peek(pid: 1, kind: .readyForReview),
            .chime(.finished), .peek(pid: 2, kind: .readyForReview),
        ])
        // The newest of equals is the one offered.
        #expect(merged == [.chime(.finished), .peek(pid: 2, kind: .readyForReview)])
    }

    @Test func theBlockedSoundAndSessionWinABurst() {
        let merged = Policy.merge([
            .chime(.finished), .peek(pid: 1, kind: .readyForReview),
            .chime(.needsInput), .peek(pid: 2, kind: .needsInput),
            .chime(.finished), .peek(pid: 3, kind: .readyForReview),
        ])
        #expect(merged == [.chime(.needsInput), .peek(pid: 2, kind: .needsInput)])
    }

    @Test func anAutoOpenInABurstReplacesEveryPeek() {
        let merged = Policy.merge([
            .peek(pid: 1, kind: .needsInput),
            .autoOpen(sessionID: "b", kind: .readyForReview),
            .autoOpen(sessionID: "a", kind: .needsInput),
            .autoOpen(sessionID: "c", kind: .readyForReview),
        ])
        #expect(merged == [.autoOpen(sessionID: "a", kind: .needsInput)])
    }

    @Test func mergingNothingDoesNothing() {
        #expect(Policy.merge([]).isEmpty)
        #expect(Policy.merge([.chime(.finished)]) == [.chime(.finished)])
    }

    /// Whatever a burst holds, the result never has two chimes or two ways of
    /// opening the notch.
    @Test func mergedBurstsAreAlwaysSingle() {
        let kinds: [ClaudeAttentionTransition.Kind] = [.needsInput, .readyForReview]
        var decisions: [Policy.Decision] = []
        for index in 0..<40 {
            let kind = kinds[index % 2]
            decisions.append(index % 3 == 0 ? .chime(.finished) : .chime(.needsInput))
            decisions.append(index % 5 == 0 ? .autoOpen(sessionID: "s\(index)", kind: kind) : .peek(pid: Int32(index), kind: kind))
            let merged = Policy.merge(decisions)
            #expect(merged.filter { if case .chime = $0 { return true }; return false }.count == 1)
            #expect(merged.filter { if case .chime = $0 { return false }; return true }.count == 1)
        }
    }

    // MARK: - Hover-card rows

    @Test func smartClickSendsWhatNeedsYouToThePanel() {
        let needs = ClaudeAttention.needsInput(ClaudeNeedsInput(kind: .question, summary: "Question"))
        #expect(Policy.sessionClick(needs, setting: .smart) == .panel)
        for other in [ClaudeAttention.working, .readyForReview, .idle] {
            #expect(Policy.sessionClick(other, setting: .smart) == .terminal)
        }
        for attention in [needs, .working, .readyForReview, .idle] {
            #expect(Policy.sessionClick(attention, setting: .panel) == .panel)
            #expect(Policy.sessionClick(attention, setting: .terminal) == .terminal)
        }
    }

    // MARK: - Dock and folded notch

    @Test func dockBadgeShowsTheNeedsYouCount() {
        #expect(Policy.dockBadgeLabel(needsYou: 3, enabled: true) == "3")
        #expect(Policy.dockBadgeLabel(needsYou: 0, enabled: true) == nil)
        #expect(Policy.dockBadgeLabel(needsYou: 3, enabled: false) == nil)
    }

    @Test func restingMarksInOrder() {
        #expect(Policy.restingMarks(.zero).isEmpty)
        #expect(Policy.restingMarks(ClaudeAttentionCounts(needsYou: 2, review: 1, working: 4, idle: 3))
            == [.needsYou, .review, .working])
        #expect(Policy.restingMarks(ClaudeAttentionCounts(review: 1, idle: 3)) == [.review])
        #expect(Policy.restingMarks(ClaudeAttentionCounts(needsYou: 1, working: 1)) == [.needsYou, .working])
        #expect(Policy.restingBreaths % 2 == 1)
    }
}
