import Foundation
import Testing
@testable import ClaudeControl

/// One answer per request, and none before it has been seen.
struct B_AnswerGateTests {
    private let t0 = Date(timeIntervalSince1970: 1_000)

    @Test func aNewRequestWaitsBeforeItCanBeAnswered() {
        var gate = AnswerGate()
        gate.noteShown(["a"], now: t0)
        #expect(!gate.isArmed("a", now: t0.addingTimeInterval(0.1)))
        let claimed1 = gate.claim("a", now: t0.addingTimeInterval(0.1))
        #expect(!claimed1)
        #expect(gate.isArmed("a", now: t0.addingTimeInterval(AnswerGate.armDelay)))
        #expect(gate.nextArming(after: t0) == t0.addingTimeInterval(AnswerGate.armDelay))
        #expect(gate.nextArming(after: t0.addingTimeInterval(1)) == nil)
    }

    @Test func aDoubleClickSendsOneAnswer() {
        var gate = AnswerGate()
        gate.noteShown(["a"], now: t0)
        let later = t0.addingTimeInterval(1)
        let claimed2 = gate.claim("a", now: later)
        #expect(claimed2)
        let claimed3 = gate.claim("a", now: later.addingTimeInterval(0.1))
        #expect(!claimed3)
    }

    @Test func theRequestThatReplacesAnAnsweredOneIsNotAnsweredByTheSecondClick() {
        var gate = AnswerGate()
        gate.noteShown(["a"], now: t0)
        let claimed4 = gate.claim("a", now: t0.addingTimeInterval(1))
        #expect(claimed4)
        // The queue promotes b into the same row 100 ms later; the second
        // click of the double-click lands 120 ms after the first.
        gate.noteShown(["b"], now: t0.addingTimeInterval(1.1))
        let claimed5 = gate.claim("b", now: t0.addingTimeInterval(1.12))
        #expect(!claimed5)
        let claimed6 = gate.claim("b", now: t0.addingTimeInterval(1.5))
        #expect(claimed6)
    }

    @Test func unknownAndForgottenRequestsCantBeAnswered() {
        var gate = AnswerGate()
        let claimed7 = gate.claim("x", now: t0)
        #expect(!claimed7)
        gate.noteShown(["a"], now: t0)
        gate.noteShown([String](), now: t0.addingTimeInterval(5))
        #expect(!gate.isArmed("a", now: t0.addingTimeInterval(10)))
        #expect(gate.firstShown.isEmpty)
    }

    @Test func requestsShownArmedCanBeAnsweredAtOnce() {
        var gate = AnswerGate()
        gate.noteShownArmed(["a"])
        let claimed8 = gate.claim("a", now: t0)
        #expect(claimed8)
    }
}

/// Routes, heights, drafts and "mark all reviewed" with undo.
struct B_PanelStateTests {
    @Test func routesBackAndClose() {
        let state = ClaudePanelState(route: .sessions(ringID: "claude-work"))
        #expect(state.ringFilter == "claude-work")
        #expect(state.mode == .list)
        var closed = 0
        state.onClose = { closed += 1 }
        state.showChat(sessionId: "s1")
        #expect(state.route == .session(id: "s1"))
        #expect(state.mode == .chat)
        #expect(state.selectedSessionId == "s1")
        state.escape()
        #expect(state.route == .sessions(ringID: "claude-work"))
        #expect(closed == 0)
        state.escape()
        #expect(closed == 1)
    }

    @Test func aHighlightedRowIsSelected() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.highlightedSessionId = "needs"
        #expect(state.selectedSessionId == "needs")
    }

    @Test func heightStaysWithinTheMinimumAndTheCapForTheMode() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.reportContentHeight(40)
        #expect(state.idealContentHeight == 220)
        state.reportContentHeight(5_000)
        #expect(state.idealContentHeight == ClaudePanelGeometry.heightCap(.list))
        state.reportContentHeight(.infinity)
        #expect(state.idealContentHeight == ClaudePanelGeometry.heightCap(.list))
        state.reportContentHeight(.nan)
        #expect(state.idealContentHeight == ClaudePanelGeometry.heightCap(.list))
        state.maxContentHeight = 300
        #expect(state.idealContentHeight == 300)
        state.maxContentHeight = 100
        #expect(state.idealContentHeight == 220)
        state.maxContentHeight = nil
        state.showChat(sessionId: "s")
        #expect(state.idealContentHeight == ClaudePanelGeometry.heightCap(.chat))
        #expect(ClaudePanelState.clampedHeight(300.2, max: 680) == 301)
    }

    @Test func foldingASectionFlipsItsCurrentState() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        state.toggleSection(.idle, count: 9)
        #expect(state.sectionFolds[.idle] == false)
        state.toggleSection(.working, count: 2)
        #expect(state.sectionFolds[.working] == true)
    }

    @Test func markAllReviewedWaitsOutItsUndo() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        var marked: [[String]] = []
        state.markReviewedWithUndo(["a", "b"], schedulesCommit: false) { marked.append($0) }
        #expect(state.pendingReview?.sessionIds == ["a", "b"])
        #expect(marked.isEmpty)

        state.undoPendingReview()
        #expect(state.pendingReview == nil)
        #expect(marked.isEmpty)

        state.markReviewedWithUndo(["a"], schedulesCommit: false) { marked.append($0) }
        // A second "mark all" first carries out the one pending.
        state.markReviewedWithUndo(["c"], schedulesCommit: false) { marked.append($0) }
        #expect(marked == [["a"]])
        // Closing the panel carries it out too.
        state.isPresented = false
        #expect(marked == [["a"], ["c"]])
        #expect(state.pendingReview == nil)

        state.markReviewedWithUndo([], schedulesCommit: false) { marked.append($0) }
        #expect(state.pendingReview == nil)
    }

    @Test func markAllReviewedCommitsAfterTheUndoWindow() async throws {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        var marked: [String] = []
        state.markReviewedWithUndo(["a"]) { marked = $0 }
        try await Task.sleep(for: .seconds(ClaudePanelState.undoWindow - 1))
        #expect(marked.isEmpty, "still undoable")
        // Polled: other suites share the main actor the commit runs on.
        for _ in 0..<80 where marked.isEmpty {
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(marked == ["a"])
        #expect(state.pendingReview == nil)
    }

    @Test func answersAreClaimedOncePerRequest() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        let t0 = Date()
        state.noteShownRequests(["t"], now: t0)
        #expect(!state.claimAnswer("t", now: t0))
        #expect(state.claimAnswer("t", now: t0.addingTimeInterval(1)))
        #expect(!state.claimAnswer("t", now: t0.addingTimeInterval(2)))
    }

    @Test func motionIsFiniteAndStillsUnderReduceMotion() {
        #expect(ClaudeMotion.breathHalfCycles % 2 == 1)
        #expect(ClaudeMotion.breathHalfCycles(reduceMotion: false, isStatic: false) == ClaudeMotion.breathHalfCycles)
        #expect(ClaudeMotion.breathHalfCycles(reduceMotion: true, isStatic: false) == 0)
        #expect(ClaudeMotion.breathHalfCycles(reduceMotion: false, isStatic: true) == 0)
        #expect(ClaudeMotion.spins(reduceMotion: false, isStatic: false))
        #expect(!ClaudeMotion.spins(reduceMotion: true, isStatic: false))
        #expect(ClaudeMotion.animation(.default, reduceMotion: true) == nil)
        #expect(ClaudeMotion.animation(.default, reduceMotion: false) != nil)
    }
}
