//
//  ClaudeControlHub+SealedDemo.swift
//  ClaudeControl
//
//  The sealed demo's timeline: changes to the fixture accounts and sessions
//  that a sealed run plays after launch, so the notch can be seen reacting
//  without a restart: prompts being answered, a third account signing in, a
//  burst of sessions finishing, a session stopping for a permission prompt.
//
//  Sealed runs only; a live hub ignores every step. Nothing here reads or
//  writes anything outside the process.
//
//  Owned by WP-C (the app's `ClaudeSealedDemo` schedules the steps).
//

import Foundation

extension ClaudeControlHub {
    /// One change the sealed demo can make, in the order it makes them.
    @_spi(Sealed) public enum SealedDemoStep: String, CaseIterable, Sendable {
        /// The work account's permission prompt is allowed and its
        /// rate-limited turn retried: both go back to work, so that ring
        /// shows the working spinner while the personal one still pulses
        /// amber. Resolutions, so nothing chimes.
        case answerWorkPrompts
        /// A third account (`~/.claude-side`, "Side project") appears, with a
        /// session that finished 85 s ago (its ring's green arc settles five
        /// seconds later) and an idle one.
        case addThirdAccount
        /// Two working sessions on two rings finish at the same moment: one
        /// burst of ready-for-review transitions.
        case finishTwoSessions
        /// A working session stops for a permission prompt.
        case askPermission

        /// When the demo plays this step, in seconds after launch. The whole
        /// timeline fits a sealed run of ten seconds or less, and the third
        /// account's ring settles before it ends.
        public var secondsAfterLaunch: TimeInterval {
            switch self {
            case .answerWorkPrompts: return 1.5
            case .addThirdAccount: return 3
            case .finishTwoSessions: return 4.5
            case .askPermission: return 6
            }
        }
    }

    /// Apply one step of the sealed demo. Does nothing unless sealed.
    @_spi(Sealed) public func runSealedDemoStep(_ step: SealedDemoStep, now: Date = Date()) async {
        guard isSealed else { return }
        let registry = AccountRegistry.shared
        // The store's sessions, not the monitor's last publish: publishes are
        // coalesced (50 ms), and a step right after another must build on it.
        let current = await SessionStore.shared.fixtureSessions()
        let change = SealedDemoScript.apply(step, accounts: registry.accounts,
                                            sessions: current.isEmpty ? ClaudeSessionMonitor.shared.instances : current,
                                            now: now)
        if change.accounts != registry.accounts {
            registry.replaceAllWithFixtures(change.accounts)
        }
        await SessionStore.shared.replaceAllWithFixtures(change.sessions)
    }
}

/// The demo's changes as pure functions of the current fixtures, for tests.
enum SealedDemoScript {
    static let thirdAccount = SampleData.side

    /// How long ago the third account's session finished when it appears.
    static let thirdAccountReviewAge: TimeInterval = 85

    /// The work account's prompts that `answerWorkPrompts` answers.
    static let answeredPrompts: Set<String> = ["needs-permission", "needs-ratelimit"]

    static func apply(
        _ step: ClaudeControlHub.SealedDemoStep,
        accounts: [ClaudeAccount],
        sessions: [SessionState],
        now: Date
    ) -> (accounts: [ClaudeAccount], sessions: [SessionState]) {
        switch step {
        case .answerWorkPrompts:
            return (accounts, sessions.map { session in
                guard answeredPrompts.contains(session.sessionId),
                      case .needsInput = session.attention else { return session }
                var working = session
                working.phase = .processing
                working.needsInputReason = nil
                working.completedAt = nil
                working.turnStartedAt = now
                working.lastActivity = now
                working.lastEventAt = now
                return working
            })
        case .addThirdAccount:
            guard !accounts.contains(where: { $0.id == thirdAccount.id }) else { return (accounts, sessions) }
            return (accounts + [thirdAccount], sessions + thirdAccountSessions(now: now))
        case .finishTwoSessions:
            let finishing: Set<String> = ["work-summary", "work-ci"]
            return (accounts, sessions.map { session in
                guard finishing.contains(session.sessionId), session.attention == .working else { return session }
                var finished = session
                finished.phase = .waitingForInput
                finished.completedAt = now
                finished.lastActivity = now
                finished.lastEventAt = now
                finished.lastAssistantMessage = "Done."
                return finished
            })
        case .askPermission:
            return (accounts, sessions.map { session in
                guard session.sessionId == "work-migration", session.attention == .working else { return session }
                var asking = session
                asking.phase = .waitingForApproval(PermissionContext(
                    toolUseId: "toolu_sealed_demo_edit",
                    toolName: "Edit",
                    toolInput: ["file_path": AnyCodable("migrations/v2_schema.sql")],
                    receivedAt: now
                ))
                asking.lastActivity = now
                asking.lastEventAt = now
                return asking
            })
        }
    }

    static func thirdAccountSessions(now: Date) -> [SessionState] {
        var review = SampleSessions.make(
            id: "side-launch-post",
            title: "Write the launch announcement",
            project: "side-site",
            account: thirdAccount,
            phase: .waitingForInput,
            tasks: SampleSessions.taskList(done: 4, active: nil, pending: 0),
            context: 31,
            completed: now,
            lastAssistant: "The announcement draft is in posts/launch.md."
        )
        review.turnStartedAt = now.addingTimeInterval(-6 * 60)
        review.completedAt = now.addingTimeInterval(-thirdAccountReviewAge)
        review.lastActivity = now.addingTimeInterval(-thirdAccountReviewAge)
        review.lastEventAt = review.lastActivity

        var idle = SampleSessions.make(
            id: "side-pricing",
            title: "Sketch the pricing page",
            project: "side-site",
            account: thirdAccount,
            phase: .idle,
            context: 9,
            lastActivity: now
        )
        idle.lastActivity = now.addingTimeInterval(-40 * 60)
        idle.lastEventAt = idle.lastActivity
        return [review, idle]
    }
}
