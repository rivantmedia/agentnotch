import Foundation
import Testing
@testable import ClaudeControl

/// Per-ring and total counts, the 90-second "just finished" window, paused
/// rings and attention transitions (with their silent baseline).
struct A3_CountsAndFreshTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func summary(_ id: String, ring: String, _ attention: ClaudeAttention, since: Date? = nil) -> ClaudeSessionSummary {
        ClaudeSessionSummary(id: id, ringID: ring, title: id, projectName: "p", attention: attention,
                             attentionSince: since ?? now.addingTimeInterval(-600))
    }

    private let permission = ClaudeAttention.needsInput(ClaudeNeedsInput(kind: .permission, summary: "Allow Bash"))

    @Test func countsPerRingAndTotal() {
        let sessions = [
            summary("a", ring: "claude", permission),
            summary("b", ring: "claude", .working),
            summary("c", ring: "claude", .readyForReview),
            summary("d", ring: "claude-work", .needsInput(ClaudeNeedsInput(kind: .error, summary: "Rate limited"))),
            summary("e", ring: "claude-work", .idle),
            summary("f", ring: "claude-work", .working),
        ]
        let counts = ClaudeControlHub.ringCounts(sessions: sessions, ringIDs: ["claude", "claude-work", "claude-side"])
        #expect(counts["claude"] == ClaudeAttentionCounts(needsYou: 1, review: 1, working: 1, idle: 0))
        // A failed turn counts as failed, not as needing you (GUX-2).
        #expect(counts["claude-work"] == ClaudeAttentionCounts(needsYou: 0, review: 0, working: 1, idle: 1, failed: 1))
        // A ring with no sessions is there, at zero, so its badges clear.
        #expect(counts["claude-side"] == .zero)
        #expect(ClaudeAttentionCounts.of(sessions) == ClaudeAttentionCounts(needsYou: 1, review: 1, working: 2, idle: 1, failed: 1))
    }

    @Test func freshSuccessLastsNinetySecondsPerRing() {
        let sessions = [
            summary("new", ring: "claude", .readyForReview, since: now.addingTimeInterval(-20)),
            summary("older", ring: "claude", .readyForReview, since: now.addingTimeInterval(-80)),
            summary("old", ring: "claude-work", .readyForReview, since: now.addingTimeInterval(-91)),
            summary("working", ring: "claude-side", .working, since: now.addingTimeInterval(-5)),
        ]
        let fresh = ClaudeControlHub.freshSuccessUntil(sessions: sessions, now: now)
        // The newest completion on the ring decides.
        #expect(fresh == ["claude": now.addingTimeInterval(70)])
        #expect(ClaudeControlHub.freshSuccessWindow == 90)
        // The boundary: exactly 90 s after the completion it has settled.
        let boundary = now.addingTimeInterval(70)
        #expect(ClaudeControlHub.freshSuccessUntil(sessions: sessions, now: boundary.addingTimeInterval(-0.001)) == ["claude": boundary])
        #expect(ClaudeControlHub.freshSuccessUntil(sessions: sessions, now: boundary).isEmpty)
    }

    @Test func ringsSwitchedOffPauseTheirAccounts() {
        let accounts = [
            ClaudeAccountSummary(id: "/h/.claude", ringID: "claude", configDir: "/h/.claude", label: "a", isDefault: true, launchCommand: "claude"),
            ClaudeAccountSummary(id: "/h/.claude-work", ringID: "claude-work", configDir: "/h/.claude-work", label: "b", isDefault: false, launchCommand: "x"),
        ]
        #expect(ClaudeControlHub.pausedAccountIds(accounts: accounts, shownRings: nil).isEmpty)
        #expect(ClaudeControlHub.pausedAccountIds(accounts: accounts, shownRings: ["claude"]) == ["/h/.claude-work"])
        #expect(ClaudeControlHub.pausedAccountIds(accounts: accounts, shownRings: []) == ["/h/.claude", "/h/.claude-work"])
    }
}

struct A3_TransitionsBaselineTests {
    typealias Snapshot = ClaudeControlHub.AttentionSnapshot
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func snapshot(_ id: String, _ attention: SessionAttention) -> Snapshot {
        let public_: ClaudeAttention
        switch attention {
        case .needsInput: public_ = .needsInput(ClaudeNeedsInput(kind: .permission, summary: "x"))
        case .working: public_ = .working
        case .readyForReview: public_ = .readyForReview
        case .idle: public_ = .idle
        }
        return Snapshot(attention: attention, summary: ClaudeSessionSummary(
            id: id, ringID: "claude", title: id, projectName: "p", attention: public_, attentionSince: now))
    }

    private func previous(_ snapshots: [Snapshot]) -> [String: Snapshot] {
        Dictionary(uniqueKeysWithValues: snapshots.map { ($0.summary.id, $0) })
    }

    private func kinds(_ transitions: [ClaudeAttentionTransition]) -> [String] {
        transitions.map { "\($0.session.id):\($0.kind)" }
    }

    @Test func theBaselineIsSilent() {
        let waiting = [snapshot("a", .needsInput(.question)), snapshot("b", .readyForReview)]
        #expect(ClaudeControlHub.transitions(previous: nil, current: waiting).isEmpty)
    }

    /// CS-3: after the baseline, a session seen for the first time is news
    /// like any other (as the banners have it); one that starts working or
    /// idle says nothing.
    @Test func firstSeenSessionsAfterTheBaselineAreNews() {
        let before = previous([snapshot("a", .working)])
        let now = [snapshot("a", .working), snapshot("new", .needsInput(.permission(tool: "Bash"))),
                   snapshot("quiet", .working), snapshot("idle", .idle)]
        #expect(kinds(ClaudeControlHub.transitions(previous: before, current: now)) == ["new:needsInput"])
    }

    @Test func crossingsAreAnnounced() {
        let before = previous([
            snapshot("ask", .working),
            snapshot("done", .working),
            snapshot("answered", .needsInput(.permission(tool: "Bash"))),
            snapshot("reviewed", .readyForReview),
            snapshot("next", .needsInput(.permission(tool: "Bash"))),
            snapshot("same", .needsInput(.question)),
            snapshot("finishedAfterAsking", .needsInput(.planApproval)),
            snapshot("idle", .idle),
        ])
        let current = [
            snapshot("ask", .needsInput(.question)),
            snapshot("done", .readyForReview),
            snapshot("answered", .working),
            snapshot("reviewed", .idle),
            snapshot("next", .needsInput(.permission(tool: "Edit"))),
            snapshot("same", .needsInput(.question)),
            snapshot("finishedAfterAsking", .readyForReview),
            snapshot("idle", .working),
        ]
        #expect(kinds(ClaudeControlHub.transitions(previous: before, current: current)) == [
            "ask:needsInput",
            "done:readyForReview",
            "answered:resolved",
            "reviewed:resolved",
            "next:needsInput",
            // The wait is over, and the turn is done: both are said.
            "finishedAfterAsking:resolved",
            "finishedAfterAsking:readyForReview",
        ])
    }

    @Test func aWaitingSessionThatGoesAwayIsResolved() {
        let before = previous([
            snapshot("waiting", .needsInput(.permission(tool: "Bash"))),
            snapshot("review", .readyForReview),
            snapshot("working", .working),
        ])
        let transitions = ClaudeControlHub.transitions(previous: before, current: [])
        #expect(kinds(transitions) == ["review:resolved", "waiting:resolved"])
        // The last summary it had is what's reported.
        #expect(transitions.first { $0.session.id == "waiting" }?.session.attention
            == .needsInput(ClaudeNeedsInput(kind: .permission, summary: "x")))
    }
}
