import Foundation
import Testing
@testable import ClaudeControl

/// Grouping, ordering and counts of the Sessions tab.
struct SessionSectionsTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func session(
        _ id: String,
        phase: SessionPhase = .idle,
        reason: NeedsInputReason? = nil,
        turnStarted: TimeInterval? = nil,
        completed: TimeInterval? = nil,
        reviewed: TimeInterval? = nil,
        lastActivity: TimeInterval = -60
    ) -> SessionState {
        var session = SessionState(
            sessionId: id,
            cwd: "/tmp/\(id)",
            phase: phase,
            lastActivity: now.addingTimeInterval(lastActivity)
        )
        session.needsInputReason = reason
        session.turnStartedAt = turnStarted.map { now.addingTimeInterval($0) }
        session.completedAt = completed.map { now.addingTimeInterval($0) }
        session.reviewedAt = reviewed.map { now.addingTimeInterval($0) }
        return session
    }

    private func approval(_ tool: String, receivedAgo: TimeInterval) -> SessionPhase {
        .waitingForApproval(
            PermissionContext(toolUseId: "toolu_\(tool)", toolName: tool, toolInput: nil, receivedAt: now.addingTimeInterval(-receivedAgo))
        )
    }

    private func ids(_ sections: [SessionSection]) -> [[String]] {
        sections.map { $0.sessions.map(\.sessionId) }
    }

    @Test func sectionsFollowBucketOrderAndSkipEmptyOnes() {
        let sessions = [
            session("idle", phase: .idle),
            session("working", phase: .processing, turnStarted: -30),
            session("review", phase: .waitingForInput, completed: -10),
            session("needs", phase: approval("Bash", receivedAgo: 5)),
        ]
        let sections = SessionSections.build(sessions)
        #expect(sections.map(\.bucket) == [.needsInput, .readyForReview, .working, .idle])
        #expect(ids(sections) == [["needs"], ["review"], ["working"], ["idle"]])

        let noReview = SessionSections.build([session("w", phase: .processing), session("i")])
        #expect(noReview.map(\.bucket) == [.working, .idle])
    }

    @Test func needsYouIsOldestWaitingFirstWithFailedTurnsAfterAnswers() {
        let sessions = [
            session("recent", phase: approval("Bash", receivedAgo: 30)),
            session("oldest", phase: approval("Edit", receivedAgo: 600)),
            // Without a pending approval the last hook event is when it started waiting.
            session("dialog", phase: .waitingForInput, reason: .dialog(nil), lastActivity: -120),
            // A failed turn waits on a retry, not an answer: it never pushes
            // a prompt below the fold, however long ago it failed.
            session("error", phase: .waitingForInput, reason: .error("Rate limited"), lastActivity: -900),
            session("error2", phase: .waitingForInput, reason: .error("Overloaded"), lastActivity: -60),
        ]
        #expect(ids(SessionSections.build(sessions)) == [["oldest", "dialog", "recent", "error", "error2"]])
    }

    @Test func reviewIsNewestCompletionFirst() {
        let sessions = [
            session("old", phase: .waitingForInput, completed: -3_600),
            session("new", phase: .waitingForInput, completed: -60),
            session("mid", phase: .idle, completed: -600),
        ]
        #expect(ids(SessionSections.build(sessions)) == [["new", "mid", "old"]])
    }

    @Test func workingIsLongestRunningFirstWithUnknownStartsLast() {
        let sessions = [
            session("short", phase: .processing, turnStarted: -30),
            session("unknown", phase: .compacting),
            session("long", phase: .processing, turnStarted: -900),
        ]
        #expect(ids(SessionSections.build(sessions)) == [["long", "short", "unknown"]])
    }

    @Test func idleIsMostRecentFirst() {
        let sessions = [
            session("yesterday", lastActivity: -86_400),
            session("now", lastActivity: -5),
            session("hour", lastActivity: -3_600),
        ]
        #expect(ids(SessionSections.build(sessions)) == [["now", "hour", "yesterday"]])
    }

    @Test func tiesAreBrokenBySessionIdWhateverTheInputOrder() {
        let a = session("a", phase: .processing, turnStarted: -60)
        let b = session("b", phase: .processing, turnStarted: -60)
        let c = session("c", phase: .processing, turnStarted: -60)
        #expect(ids(SessionSections.build([c, a, b])) == [["a", "b", "c"]])
        #expect(ids(SessionSections.build([b, c, a])) == [["a", "b", "c"]])
    }

    @Test func preservedOrderKeepsRowsPutAndAppendsNewcomers() {
        let first = session("first", phase: .waitingForInput, completed: -600)
        let second = session("second", phase: .waitingForInput, completed: -300)
        // Displayed while hovered: second (newest) above first.
        let displayed = SessionSections.order(of: SessionSections.build([first, second]))
        #expect(displayed == ["second", "first"])

        // "first" finishes again (now newest) and a new review arrives: under
        // the pointer nothing moves; the newcomer goes to the end.
        var refreshed = first
        refreshed.completedAt = now.addingTimeInterval(-10)
        let newcomer = session("newcomer", phase: .waitingForInput, completed: -5)
        let frozen = SessionSections.build([refreshed, second, newcomer], preservingOrder: displayed)
        #expect(ids(frozen) == [["second", "first", "newcomer"]])

        // Without the pointer the natural order returns.
        #expect(ids(SessionSections.build([refreshed, second, newcomer])) == [["newcomer", "first", "second"]])
    }

    @Test func preservedOrderStillMovesSessionsBetweenSections() {
        let working = session("w", phase: .processing, turnStarted: -60)
        let displayed = SessionSections.order(of: SessionSections.build([working]))
        let finished = session("w", phase: .waitingForInput, completed: -1)
        let sections = SessionSections.build([finished], preservingOrder: displayed)
        #expect(sections.map(\.bucket) == [.readyForReview])
    }

    @Test func countsPerBucketWithFailedTurnsApart() {
        let counts = AttentionCounts([
            session("n1", phase: approval("Bash", receivedAgo: 1)),
            session("n2", phase: .idle, reason: .dialog(nil)),
            session("f", phase: .waitingForInput, reason: .error("Rate limited")),
            session("r", phase: .waitingForInput, completed: -1),
            session("w1", phase: .processing),
            session("w2", phase: .compacting),
            session("w3", phase: .processing),
            session("i"),
        ])
        #expect(counts == AttentionCounts(needsInput: 3, readyForReview: 1, working: 3, idle: 1, failed: 1))
        #expect(counts.answerable == 2)
        #expect(counts.total == 8)
    }

    @Test func stripSaysWhatNeedsYouAndWhatFailedApart() {
        #expect(AttentionStrip.accessibilityText(AttentionCounts(needsInput: 1, readyForReview: 2, working: 3))
            == "1 needs you, 2 ready for review, 3 working")
        #expect(AttentionStrip.accessibilityText(AttentionCounts(needsInput: 3, failed: 1)) == "2 need you, 1 failed")
        #expect(AttentionStrip.accessibilityText(AttentionCounts(idle: 4)) == "4 idle")
        #expect(AttentionStrip.accessibilityText(AttentionCounts()) == "No sessions")
    }

    @Test func sectionTitles() {
        #expect(AttentionBucket.allCases.map(\.sectionTitle) == ["Needs you", "Ready for review", "Working", "Idle"])
    }

    // MARK: Density

    @Test func onlyALongIdleListStartsFoldedAndNeedsYouNeverFolds() {
        #expect(!SessionSections.isCollapsed(.idle, count: 3, overrides: [:]))
        #expect(SessionSections.isCollapsed(.idle, count: 4, overrides: [:]))
        #expect(!SessionSections.isCollapsed(.working, count: 40, overrides: [:]))
        #expect(SessionSections.isCollapsed(.working, count: 2, overrides: [.working: true]))
        #expect(!SessionSections.isCollapsed(.idle, count: 9, overrides: [.idle: false]))
        #expect(!SessionSections.isCollapsed(.needsInput, count: 9, overrides: [.needsInput: true]))
    }

    @Test func rowsGoCompactPastEightSessions() {
        #expect(!SessionSections.isCompact(sessionCount: 8))
        #expect(SessionSections.isCompact(sessionCount: 9))
    }

    @Test func foldedSummaryNamesTheFirstTitlesAndCountsTheRest() {
        var a = session("a", phase: .processing); a.applyTitle("Write tests", source: .hook)
        var b = session("b", phase: .processing); b.applyTitle("Fix CI", source: .hook)
        var c = session("c", phase: .processing); c.applyTitle("Bump deps", source: .hook)
        #expect(SessionSections.collapsedSummary(SessionSection(bucket: .working, sessions: [a, b, c])) == "Write tests, Fix CI and 1 more")
        #expect(SessionSections.collapsedSummary(SessionSection(bucket: .working, sessions: [a])) == "Write tests")
    }

    @Test func layoutOrdersTheKeyboardThroughUnfoldedRowsOnly() {
        let sessions = [
            session("needs", phase: approval("Bash", receivedAgo: 5)),
            session("w", phase: .processing, turnStarted: -30),
            session("i1", lastActivity: -10), session("i2", lastActivity: -20),
            session("i3", lastActivity: -30), session("i4", lastActivity: -40),
        ]
        let layout = SessionListLayout.make(
            sections: SessionSections.build(sessions),
            rows: { SessionRowModel.make($0, account: nil, rateLimit: nil, canFocus: false, now: now, home: "/Users/me") },
            folds: [:]
        )
        // Four idle sessions fold by default.
        #expect(layout.sections.map(\.isCollapsed) == [false, false, true])
        #expect(layout.visibleOrder == ["needs", "w"])
        #expect(layout.row(id: "i3")?.bucket == .idle)
        #expect(!layout.isCompact)
    }
}
