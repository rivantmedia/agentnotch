import Foundation
import Testing
@testable import ClaudeControl

/// GUX-2 / BHV-2: a turn that stopped on an error is "failed", never amber
/// "needs you": it counts apart, gets one soft cue, no auto-open, and can be
/// dismissed for good (across a relaunch) until the next failure.
/// BHV-9: a deferred "mark all reviewed" commits with the click time.
struct Fix_FailedTurnTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-fix-failed")
        transcript = account.transcript("s1")
    }

    private func summary(_ attention: ClaudeAttention, id: String = "s1") -> ClaudeSessionSummary {
        ClaudeSessionSummary(id: id, ringID: "claude", pid: 42, title: "T", projectName: "p",
                             attention: attention, attentionSince: Date())
    }

    @Test func aFailedTurnCountsAsFailedNotNeedsYou() {
        let counts = ClaudeAttentionCounts.of([
            summary(.needsInput(ClaudeNeedsInput(kind: .error, summary: "Rate limited")), id: "a"),
            summary(.needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash")), id: "b"),
            summary(.readyForReview, id: "c"),
        ])
        #expect(counts == ClaudeAttentionCounts(needsYou: 1, review: 1, failed: 1))
        #expect(ClaudeAttentionPolicy.combined(counts, counts).failed == 2)
        // Nothing amber for failures alone: no Dock badge, no resting dot.
        let failedOnly = ClaudeAttentionCounts(failed: 3)
        #expect(ClaudeAttentionPolicy.dockBadgeLabel(needsYou: failedOnly.needsYou, enabled: true) == nil)
        #expect(ClaudeAttentionPolicy.restingMarks(failedOnly).isEmpty)
    }

    @Test func aFailureGetsOneSoftCueAndNeverOpensThePanel() {
        let failure = ClaudeAttentionTransition(kind: .needsInput,
            session: summary(.needsInput(ClaudeNeedsInput(kind: .error, summary: "Rate limited"))))
        #expect(failure.isFailure)
        for policy in AutoOpenPolicy.allCases {
            let context = ClaudeAttentionPolicy.Context(autoOpen: policy, chimes: true, peeks: true, terminalFocused: false,
                                                        anyTerminalVisible: false, fullScreen: false, panelOpen: false,
                                                        ringShown: true)
            let decisions = ClaudeAttentionPolicy.decide(failure, context: context)
            #expect(decisions == [.chime(.finished), .peek(pid: 42, kind: .needsInput)], "\(policy)")
        }
        let question = ClaudeAttentionTransition(kind: .needsInput,
            session: summary(.needsInput(ClaudeNeedsInput(kind: .question, summary: "Question"))))
        #expect(!question.isFailure)
    }

    private func event(_ name: String, at date: Date, stopError: String? = nil) -> SessionEvent {
        .hookReceived(HookEvent(sessionId: "s1", event: name, status: "waiting_for_input", cwd: "/tmp/proj",
                                transcriptPath: transcript, attended: true, entrypoint: "cli",
                                stopError: stopError, source: name == "UserPromptSubmit" ? "user" : nil,
                                prompt: name == "UserPromptSubmit" ? "go" : nil, receivedAt: date))
    }

    @Test func aDismissedFailureStaysDismissedAcrossARelaunch() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        let store = SessionStore.forTests(reviewStore: reviews, effects: .none, completionTiming: .immediate)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", at: t0))
        await store.process(event("StopFailure", at: t0.addingTimeInterval(1), stopError: "rate_limit"))
        #expect(await store.session(for: "s1")?.hasFailedTurn == true)

        await store.process(.dismissFailure(sessionId: "s1", at: t0.addingTimeInterval(2)))
        let dismissed = try #require(await store.session(for: "s1"))
        #expect(!dismissed.hasFailedTurn)
        #expect(!dismissed.attention.isError)
        reviews.flush()
        #expect(ReviewStateStore(fileURL: account.reviewFile, createsFolder: false).record(for: "s1")?.stopError == nil)

        // Next run: not restored as failed.
        let second = SessionStore.forTests(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none, completionTiming: .immediate)
        await second.process(.registrySnapshot(configDir: account.configDir.path, entries: [
            SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: "idle",
                                 waitingFor: nil, statusUpdatedAt: t0.addingTimeInterval(3)),
        ]))
        #expect(await second.session(for: "s1")?.hasFailedTurn == false)
    }

    @Test func aFailureNewerThanTheClickStays() async throws {
        let store = SessionStore.forTests(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none, completionTiming: .immediate)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", at: t0))
        await store.process(event("StopFailure", at: t0.addingTimeInterval(5), stopError: "overloaded"))
        await store.process(.dismissFailure(sessionId: "s1", at: t0.addingTimeInterval(1)))
        #expect(await store.session(for: "s1")?.hasFailedTurn == true)
    }

    @MainActor
    @Test func aFailedRowIsDismissedWithCommandRButNotCommandReturn() {
        var target = ClaudeKeyRouter.Target(sessionId: "s1", actions: .none, canMarkReviewed: false, canJump: false)
        target.canDismissFailure = true
        #expect(ClaudeKeyRouter.command(for: .character("r"), modifiers: .command, in: .list(selection: target))
                == .markReviewed(sessionId: "s1"))
        #expect(ClaudeKeyRouter.command(for: .returnKey, modifiers: .command, in: .list(selection: target)) == nil)
    }

    @MainActor
    @Test func markAllReviewedCommitsWithTheClickTime() {
        let state = ClaudePanelState(route: .sessions(ringID: nil))
        let click = Date(timeIntervalSince1970: 1_900_000_000)
        var committed: (ids: [String], at: Date)?
        state.markReviewedWithUndo(["a", "b"], now: click, schedulesCommit: false) { ids, at in committed = (ids, at) }
        state.commitPendingReview()
        #expect(committed?.ids == ["a", "b"])
        #expect(committed?.at == click)
    }
}
