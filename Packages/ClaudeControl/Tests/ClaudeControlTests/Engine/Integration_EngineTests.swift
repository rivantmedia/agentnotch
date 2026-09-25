//
//  Integration_EngineTests.swift
//  ClaudeControlTests
//
//  What the integration of the work packages added on the engine side:
//  quiet and pre-launch completions are never announced by the hub, failed
//  turns get their own banner, and the account projection carries A2's
//  hook kinds and organization.
//

import Foundation
import Testing
@testable import ClaudeControl

struct Integration_HubTransitionTests {
    typealias Snapshot = ClaudeControlHub.AttentionSnapshot
    let launch = Date(timeIntervalSince1970: 1_800_000_000)

    private func snapshot(_ id: String, _ attention: SessionAttention, completedAt: Date? = nil,
                          quiet: Bool = false) -> Snapshot {
        let public_: ClaudeAttention
        switch attention {
        case .needsInput: public_ = .needsInput(ClaudeNeedsInput(kind: .permission, summary: "x"))
        case .working: public_ = .working
        case .readyForReview: public_ = .readyForReview
        case .idle: public_ = .idle
        }
        return Snapshot(
            attention: attention,
            summary: ClaudeSessionSummary(id: id, ringID: "claude", title: id, projectName: "p",
                                          attention: public_, attentionSince: completedAt ?? launch),
            completedAt: completedAt,
            isQuietCompletion: quiet
        )
    }

    private func kinds(_ transitions: [ClaudeAttentionTransition]) -> [String] {
        transitions.map { "\($0.session.id):\($0.kind)" }
    }

    @Test func quietAndPreLaunchCompletionsAreNotAnnounced() {
        let before = Dictionary(uniqueKeysWithValues: [
            snapshot("final", .working), snapshot("waitsOnAgents", .working), snapshot("inferred", .idle),
        ].map { ($0.summary.id, $0) })
        let current = [
            snapshot("final", .readyForReview, completedAt: launch.addingTimeInterval(60)),
            // Waiting for background agents / a /loop tick: stays in the
            // review queue, but no chime, peek or banner.
            snapshot("waitsOnAgents", .readyForReview, completedAt: launch.addingTimeInterval(60), quiet: true),
            // Finished while the app was down (restored or inferred later).
            snapshot("inferred", .readyForReview, completedAt: launch.addingTimeInterval(-600)),
        ]
        #expect(kinds(ClaudeControlHub.transitions(previous: before, current: current, launchedAt: launch))
            == ["final:readyForReview"])
        // Without a launch time only the quiet one is held back.
        #expect(kinds(ClaudeControlHub.transitions(previous: before, current: current))
            == ["final:readyForReview", "inferred:readyForReview"])
    }

    @Test func aQuietCompletionStillResolvesAWait() {
        let before = ["s": snapshot("s", .needsInput(.question))]
        let current = [snapshot("s", .readyForReview, completedAt: launch.addingTimeInterval(5), quiet: true)]
        #expect(kinds(ClaudeControlHub.transitions(previous: before, current: current, launchedAt: launch))
            == ["s:resolved"])
    }
}

struct Integration_FailureBannerTests {
    private func failedSession(_ code: String) -> SessionState {
        var state = SessionState(sessionId: "s1", cwd: "/Users/me/app", phase: .waitingForInput)
        state.applyTitle("Refactor the parser", source: .hook)
        state.stopError = NeedsInputReason.humanizedStopError(code)
        state.stopErrorCode = code
        state.needsInputReason = .error(NeedsInputReason.humanizedStopError(code))
        return state
    }

    @Test func aFailedTurnSaysStoppedNotNeedsYou() {
        let content = SessionNotificationContent.failed(session: failedSession("overloaded"),
                                                        reason: .error("Overloaded"), accountLabel: "Work")
        #expect(content.kind == .failed)
        #expect(content.identifier == "spcn.failed.s1")
        #expect(content.title == "Refactor the parser stopped")
        #expect(!content.title.contains("needs you"))
        #expect(content.subtitle == "Work")
        #expect(content.kind.categoryIdentifier == NotificationService.failedCategory)
    }

    @Test func hintsSayWhatToDo() {
        #expect(SessionNotificationContent.failureHint(.authentication) == "run /login in its terminal")
        #expect(SessionNotificationContent.failureHint(.overloaded) == "retry in its terminal")
        #expect(SessionNotificationContent.failureHint(.rateLimit) == nil)
        #expect(SessionNotificationContent.failureHint(nil) == nil)
    }

    @Test func aFailureBannerAppliesOnlyWhileTheTurnHasFailed() {
        typealias Content = SessionNotificationContent
        #expect(Content.stillApplies(kind: .failed, attention: .needsInput(.error("Overloaded"))))
        #expect(!Content.stillApplies(kind: .failed, attention: .needsInput(.question)))
        #expect(!Content.stillApplies(kind: .failed, attention: .working))
        #expect(!Content.stillApplies(kind: .failed, attention: nil))
        let parsed = Content.parse(identifier: Content.identifier(kind: .failed, sessionId: "a.b"))
        #expect(parsed?.kind == .failed)
        #expect(parsed?.sessionId == "a.b")
        #expect(NotificationRouting.response(identifier: "spcn.failed.a.b", actionIdentifier: NotificationService.openAction)
            == .openSession("a.b"))
    }

    @Test func answerableReasonsSortBeforeFailures() {
        #expect(NeedsInputReason.question.sortRank < NeedsInputReason.error("Overloaded").sortRank)
        // The failed turn has waited longest, and still sorts last.
        var failed = SessionState(sessionId: "failed", cwd: "/a", phase: .waitingForInput,
                                  lastActivity: Date(timeIntervalSince1970: 1))
        failed.needsInputReason = .error("Overloaded")
        var asked = SessionState(sessionId: "asked", cwd: "/b", phase: .waitingForInput,
                                 lastActivity: Date(timeIntervalSince1970: 100))
        asked.needsInputReason = .question
        #expect(SessionSections.naturallyPrecedes(asked, failed, in: .needsInput))
        #expect(!SessionSections.naturallyPrecedes(failed, asked, in: .needsInput))
    }
}

struct Integration_AccountProjectionTests {
    @Test func hookKindsAndOrganizationComeFromA2() {
        let account = ClaudeAccount(configDir: "/h/.claude-work", configDirEnv: "/h/.claude-work",
                                    email: "me@x.dev", organizationUuid: "org-1")
        var status = AccountHookStatus()
        status.superpoweredVibeNotchHooksPresent = true
        let summary = ClaudeHostProjections.account(account, hookStatus: status, home: "/h")
        #expect(summary.organizationUuid == "org-1")
        #expect(summary.hooks.superpoweredVibeNotchHooksPresent)
        #expect(!summary.hooks.vibeNotchHooksPresent)

        status = AccountHookStatus()
        status.vibeNotchHooksPresent = true
        let other = ClaudeHostProjections.account(account, hookStatus: status, organizationUuid: "org-2", home: "/h")
        #expect(other.hooks.vibeNotchHooksPresent)
        #expect(!other.hooks.superpoweredVibeNotchHooksPresent)
        // The usage store's copy wins when it has one.
        #expect(other.organizationUuid == "org-2")
    }
}
