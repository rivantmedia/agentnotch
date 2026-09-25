import Foundation
import Testing
@testable import ClaudeControl

/// Privacy of what leaves the panel: rows (served to the phone) and banners
/// name a session by its title, never by its first prompt, and never quote
/// Claude.
struct A3_PublicTextTests {
    private func session(title: String? = nil, source: SessionTitleSource = .hook, summary: String? = nil) -> SessionState {
        var state = SessionState(sessionId: "s1", cwd: "/Users/me/code/acme-web", phase: .waitingForInput)
        if let title { state.applyTitle(title, source: source) }
        state.conversationInfo = ConversationInfo(
            summary: summary, lastMessage: "SECRET-LAST", lastMessageRole: "assistant",
            lastToolName: nil, firstUserMessage: "SECRET-PROMPT please fix my login", lastUserMessageDate: nil
        )
        state.lastAssistantMessage = "SECRET-ASSISTANT all done"
        return state
    }

    @Test func titlesNeverFallBackToThePrompt() {
        typealias P = ClaudeHostProjections
        // What the panel would show before Claude Code names the session.
        #expect(session().displayTitle.contains("SECRET-PROMPT"))
        #expect(P.publicTitle(session()) == "acme-web")
        #expect(P.publicTitle(session(title: "Fix the login loop")) == "Fix the login loop")
        #expect(P.publicTitle(session(summary: "Login redirect fix")) == "Login redirect fix")
        // A name derived from the folder loses to a transcript title, and
        // still beats the bare folder.
        #expect(P.publicTitle(session(title: "acme", source: .derivedName, summary: "Login fix")) == "Login fix")
        #expect(P.publicTitle(session(title: "acme", source: .derivedName)) == "acme")
        #expect(P.publicTitle(session(title: "  Two\n lines ")) == "Two lines")
    }

    @Test func rowsAndBannersCarryNoPromptOrAssistantText() {
        let state = session()
        let summary = ClaudeHostProjections.session(state, home: "/Users/me")
        let row = ClaudeHostProjections.activityRow(summary)
        let review = SessionNotificationContent.readyForReview(session: state, accountLabel: nil)
        let asks = SessionNotificationContent.needsInput(session: state, reason: .question, accountLabel: nil)
        let limit = LimitNotificationContent.make(ringID: "claude", accountLabel: nil,
                                                  sessionTitles: [ClaudeHostProjections.publicTitle(state)], limitReset: nil)
        for text in [summary.title, row.name, row.detail, row.waitingFor ?? "",
                     review.title, review.body, asks.title, asks.body, limit.title, limit.body] {
            #expect(!text.contains("SECRET"), "\(text)")
        }
        #expect(review.title == "Done: acme-web")
        #expect(review.body == "Ready for review · acme-web")
    }
}

/// Hub projection rules added in review: the ring of an account the
/// registry doesn't know, the next moment to republish, and `.resolved`
/// before a crossing into the other kind.
struct A3_HubReviewTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func sessionsOfUnknownAccountsGoToTheDefaultRing() {
        let known: Set<String> = ["claude", "claude-work"]
        #expect(ClaudeControlHub.ringID("claude-work", knownRingIDs: known) == "claude-work")
        #expect(ClaudeControlHub.ringID("claude-new", knownRingIDs: known) == "claude")
        #expect(ClaudeControlHub.ringID("claude-dir-0badf00d", knownRingIDs: []) == "claude")
    }

    @Test func theHubRepublishesWhenAWindowResetsOrAnArcSettles() {
        let reading = ClaudeRingReading(windows: [
            .init(id: "session", usedFraction: 1, resetsAt: now.addingTimeInterval(600)),
            .init(id: "weekly_all", usedFraction: 0.4, resetsAt: now.addingTimeInterval(86_400)),
            .init(id: "weekly_opus", usedFraction: 0.1, resetsAt: now.addingTimeInterval(-5)),
            .init(id: "extra_usage", usedFraction: 0.2),
        ], status: .ok)
        typealias Hub = ClaudeControlHub
        // A used-up session window lifts in 10 minutes: republish then.
        #expect(Hub.nextBoundary(freshSuccessUntil: [:], readings: ["claude": reading], now: now)
            == now.addingTimeInterval(600))
        // A ring settling sooner comes first.
        #expect(Hub.nextBoundary(freshSuccessUntil: ["claude-work": now.addingTimeInterval(30)],
                                 readings: ["claude": reading], now: now) == now.addingTimeInterval(30))
        // Nothing ahead: nothing scheduled.
        #expect(Hub.nextBoundary(freshSuccessUntil: ["claude": now.addingTimeInterval(-1)],
                                 readings: ["claude": ClaudeRingReading(status: .waitingForFirstReading)], now: now) == nil)
    }

    private func snapshot(_ id: String, _ attention: SessionAttention) -> ClaudeControlHub.AttentionSnapshot {
        let summaryAttention: ClaudeAttention
        switch attention {
        case .needsInput: summaryAttention = .needsInput(ClaudeNeedsInput(kind: .question, summary: "Question"))
        case .working: summaryAttention = .working
        case .readyForReview: summaryAttention = .readyForReview
        case .idle: summaryAttention = .idle
        }
        return .init(attention: attention, summary: ClaudeSessionSummary(
            id: id, ringID: "claude", title: id, projectName: "p", attention: summaryAttention, attentionSince: now))
    }

    @Test func leavingAWaitIsResolvedBeforeTheNextCrossing() {
        let before = [
            "asked-while-unreviewed": snapshot("asked-while-unreviewed", .readyForReview),
            "answered": snapshot("answered", .needsInput(.question)),
            "denied-and-stopped": snapshot("denied-and-stopped", .needsInput(.permission(tool: "Bash"))),
        ]
        let current = [
            snapshot("asked-while-unreviewed", .needsInput(.question)),
            snapshot("answered", .working),
            snapshot("denied-and-stopped", .idle),
        ]
        let kinds = ClaudeControlHub.transitions(previous: before, current: current).map { "\($0.session.id):\($0.kind)" }
        #expect(kinds == [
            "asked-while-unreviewed:resolved",
            "asked-while-unreviewed:needsInput",
            "answered:resolved",
            "denied-and-stopped:resolved",
        ])
    }
}

/// "Is the user looking at this session?" once the frontmost app is known
/// to host it.
struct A3_FocusPrecisionTests {
    @Test func theSelectedTabDecidesWhenTheTerminalCanSay() {
        typealias D = TerminalVisibilityDetector
        #expect(D.isFocused(sessionTTY: "ttys004", selectedTTY: "/dev/ttys004", hostPid: 10, otherHostPids: [10, 10]))
        #expect(!D.isFocused(sessionTTY: "ttys004", selectedTTY: "/dev/ttys005", hostPid: 10, otherHostPids: []))
    }

    @Test func otherwiseOnlyASessionAloneInItsAppIsLookedAt() {
        typealias D = TerminalVisibilityDetector
        #expect(D.isFocused(sessionTTY: nil, selectedTTY: nil, hostPid: 10, otherHostPids: []))
        #expect(D.isFocused(sessionTTY: "ttys004", selectedTTY: nil, hostPid: 10, otherHostPids: [11, nil]))
        // Four Claude sessions in one VS Code: activating it says nothing
        // about which one the user reads.
        #expect(!D.isFocused(sessionTTY: nil, selectedTTY: nil, hostPid: 10, otherHostPids: [11, 10, nil]))
    }
}
