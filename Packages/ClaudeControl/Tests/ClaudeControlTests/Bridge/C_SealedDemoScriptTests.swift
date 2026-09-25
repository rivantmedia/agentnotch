import Foundation
import Testing
@_spi(Sealed) @testable import ClaudeControl

/// The sealed demo's timeline, as pure changes to the fixtures: each step
/// produces exactly the transition the notch is meant to react to, and only
/// that.
struct C_SealedDemoScriptTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private var fixtures: (accounts: [ClaudeAccount], sessions: [SessionState]) {
        (SampleSessions.accounts, SampleSessions.all())
    }

    private func attention(_ sessions: [SessionState]) -> [String: SessionAttention] {
        Dictionary(uniqueKeysWithValues: sessions.map { ($0.sessionId, $0.attention) })
    }

    @Test func theThirdAccountArrivesWithAReviewAndAnIdleSession() {
        let start = fixtures
        let change = SealedDemoScript.apply(.addThirdAccount, accounts: start.accounts, sessions: start.sessions, now: now)
        #expect(change.accounts.count == start.accounts.count + 1)
        #expect(change.accounts.last?.id == SealedDemoScript.thirdAccount.id)
        #expect(change.accounts.last?.email != nil)

        let added = change.sessions.filter { $0.accountId == SealedDemoScript.thirdAccount.id }
        #expect(added.map(\.attention) == [.readyForReview, .idle])
        // Finished long enough ago to settle a few seconds after it appears.
        let review = added[0]
        #expect(review.completedAt == now.addingTimeInterval(-SealedDemoScript.thirdAccountReviewAge))
        #expect(SealedDemoScript.thirdAccountReviewAge < ClaudeControlHub.freshSuccessWindow)
        #expect(ClaudeControlHub.freshSuccessWindow - SealedDemoScript.thirdAccountReviewAge <= 8)

        // Nobody else changed.
        #expect(Array(change.sessions.prefix(start.sessions.count)) == start.sessions)

        // A second time is a no-op.
        let again = SealedDemoScript.apply(.addThirdAccount, accounts: change.accounts, sessions: change.sessions, now: now)
        #expect(again.accounts == change.accounts && again.sessions == change.sessions)
    }

    @Test func theThirdAccountIsANewRing() {
        let home = "/Users/me"
        let ring = ClaudeRingIdentity.ringID(configDir: "~/.claude-side", home: home)
        #expect(ring == "claude-side")
        let others = SampleSessions.accounts.map { ClaudeRingIdentity.ringID(configDir: $0.configDir, home: home) }
        #expect(!others.contains(ring))
    }

    @Test func twoSessionsFinishTogether() {
        let start = fixtures
        let before = attention(start.sessions)
        let change = SealedDemoScript.apply(.finishTwoSessions, accounts: start.accounts, sessions: start.sessions, now: now)
        let after = attention(change.sessions)
        let moved = before.keys.filter { before[$0] != after[$0] }.sorted()
        #expect(moved == ["work-ci", "work-summary"])
        for id in moved {
            #expect(before[id] == .working)
            #expect(after[id] == .readyForReview)
            #expect(change.sessions.first { $0.sessionId == id }?.completedAt == now)
        }
        // The two are on different rings: one burst across two rings.
        let accounts = Set(change.sessions.filter { moved.contains($0.sessionId) }.compactMap(\.accountId))
        #expect(accounts.count == 2)
    }

    @Test func aWorkingSessionAsksForPermission() {
        let start = fixtures
        let change = SealedDemoScript.apply(.askPermission, accounts: start.accounts, sessions: start.sessions, now: now)
        let before = attention(start.sessions)
        let after = attention(change.sessions)
        let moved = before.keys.filter { before[$0] != after[$0] }
        #expect(moved == ["work-migration"])
        #expect(after["work-migration"]?.bucket == .needsInput)
        #expect(change.sessions.first { $0.sessionId == "work-migration" }?.activePermission?.toolName == "Edit")
    }

    /// The work ring stops asking and gets back to work, so the demo shows an
    /// amber ring (personal) beside a spinning one (work). Only the two
    /// answered sessions move, and they move into working.
    @Test func answeringTheWorkPromptsLeavesOnlyThePersonalRingAsking() {
        let start = fixtures
        let change = SealedDemoScript.apply(.answerWorkPrompts, accounts: start.accounts, sessions: start.sessions, now: now)
        let before = attention(start.sessions)
        let after = attention(change.sessions)
        let moved = before.keys.filter { before[$0] != after[$0] }.sorted()
        #expect(moved == ["needs-permission", "needs-ratelimit"])
        for id in moved {
            #expect(before[id]?.bucket == .needsInput)
            #expect(after[id] == .working)
            #expect(change.sessions.first { $0.sessionId == id }?.turnStartedAt == now)
        }
        let workID = SampleSessions.work.id
        let asking = change.sessions.filter { $0.attention.bucket == .needsInput }
        #expect(!asking.isEmpty)
        #expect(asking.allSatisfy { $0.accountId != workID })
        #expect(change.sessions.contains { $0.accountId == workID && $0.attention == .working })
    }

    /// Played in order, the last step early enough for a sealed run of ten
    /// seconds, and the third ring at the three seconds the design names,
    /// settling (85 s + the rest of the 90 s window) before the run ends.
    @Test func theTimelineFitsATenSecondRun() {
        let steps = ClaudeControlHub.SealedDemoStep.allCases
        let times = steps.map(\.secondsAfterLaunch)
        #expect(times == times.sorted())
        #expect(Set(times).count == times.count)
        #expect(ClaudeControlHub.SealedDemoStep.addThirdAccount.secondsAfterLaunch == 3)
        #expect((times.last ?? 0) < 8)
        let settles = ClaudeControlHub.SealedDemoStep.addThirdAccount.secondsAfterLaunch
            + ClaudeControlHub.freshSuccessWindow - SealedDemoScript.thirdAccountReviewAge
        #expect(settles < 10)
        // Further apart than a burst window, so each step is a burst of its own.
        for (a, b) in zip(times, times.dropFirst()) {
            #expect(b - a > ClaudeAttentionPolicy.burstWindow)
        }
    }

    /// Steps are idempotent once applied, so a demo that replays one (or runs
    /// on fixtures that already moved) changes nothing further.
    @Test func stepsDoNotRepeat() {
        var state = fixtures
        for step in ClaudeControlHub.SealedDemoStep.allCases {
            state = SealedDemoScript.apply(step, accounts: state.accounts, sessions: state.sessions, now: now)
        }
        for step in ClaudeControlHub.SealedDemoStep.allCases {
            let again = SealedDemoScript.apply(step, accounts: state.accounts, sessions: state.sessions, now: now)
            #expect(again.accounts == state.accounts, "\(step)")
            #expect(again.sessions == state.sessions, "\(step)")
        }
    }
}
