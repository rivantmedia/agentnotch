//
//  AttentionTracker.swift
//  ClaudeControl
//
//  Turns the session list into per-session attention transitions ("session X
//  now needs input", "session Y is ready for review") for notifications,
//  sounds and auto-open.
//
//  What is not news:
//  - anything seen before the launch baseline closes: that is once every
//    account's session registry has been read (plus a short settle), so the
//    sessions of the second, third, ... account found at launch don't all
//    alert (each registry arrives as its own snapshot);
//  - a completion from before this launch (restored from review-state.json,
//    or inferred from a transcript that finished while the app was down);
//  - a quiet completion: a turn that ended waiting for background agents
//    that will wake Claude again, or a /loop or cron tick. They stay in the
//    review queue; the final turn is announced.
//

import Combine
import Foundation

/// One session's attention changing.
nonisolated struct AttentionTransition: Sendable, Equatable {
    let session: SessionState
    /// Nil when the session just appeared.
    let from: SessionAttention?
    let to: SessionAttention

    /// The session newly needs input, or needs it for a different reason.
    var becameNeedsInput: Bool {
        guard case .needsInput(let reason) = to else { return false }
        if case .needsInput(let previous) = from { return previous != reason }
        return true
    }

    /// The session newly became ready for review.
    var becameReadyForReview: Bool {
        to == .readyForReview && from != .readyForReview
    }

    /// The turn failed (rate limit, overload, sign-in, billing): blocked on
    /// the user, but with nothing to answer. Worth a different sound and
    /// banner, and one banner per account rather than one per session.
    var isFailure: Bool {
        to.isError
    }
}

@MainActor
final class AttentionTracker {
    static let shared = AttentionTracker()

    /// Emits on the main actor, once per session whose attention changed.
    let transitions = PassthroughSubject<AttentionTransition, Never>()

    /// After the registries were first read, changes are still recorded
    /// silently this long (late snapshots, first transcript syncs).
    static let settleInterval: TimeInterval = 2
    /// The baseline closes by itself this long after `start`, even if the
    /// registry scanner never reports its first pass.
    static let maxBaselineInterval: TimeInterval = 15

    private var lastAttention: [String: SessionAttention] = [:]
    private var cancellable: AnyCancellable?
    /// Completions from before this are not news (the hub uses it too).
    let launchedAt: Date
    private var startedAt: Date?
    private var baselineEndsAt: Date?

    private init() {
        launchedAt = Date()
    }

    /// A tracker for tests.
    init(launchedAt: Date) {
        self.launchedAt = launchedAt
    }

    /// Subscribes to the shared session monitor. Idempotent.
    func start(now: Date = Date()) {
        guard cancellable == nil else { return }
        startedAt = now
        cancellable = ClaudeSessionMonitor.shared.$instances
            .sink { [weak self] sessions in
                self?.update(sessions)
            }
    }

    /// Stop following the monitor (the hub stopped). A later `start` takes a
    /// new launch baseline.
    func stop() {
        cancellable = nil
        startedAt = nil
        baselineEndsAt = nil
    }

    /// Every account's session registry has been read once.
    func initialScanCompleted(now: Date = Date()) {
        guard baselineEndsAt == nil else { return }
        baselineEndsAt = now.addingTimeInterval(Self.settleInterval)
    }

    /// Current attention for a session, as last seen by the tracker.
    func attention(for sessionId: String) -> SessionAttention? {
        lastAttention[sessionId]
    }

    /// Whether changes are still being recorded as the launch baseline.
    func isInBaseline(now: Date = Date()) -> Bool {
        if let baselineEndsAt { return now < baselineEndsAt }
        guard let startedAt else { return true }
        return now < startedAt.addingTimeInterval(Self.maxBaselineInterval)
    }

    func update(_ sessions: [SessionState], now: Date = Date()) {
        var next: [String: SessionAttention] = [:]
        var changes: [AttentionTransition] = []
        for session in sessions {
            let attention = session.attention
            next[session.sessionId] = attention
            let previous = lastAttention[session.sessionId]
            if previous != attention {
                changes.append(AttentionTransition(session: session, from: previous, to: attention))
            }
        }
        lastAttention = next

        guard !isInBaseline(now: now) else { return }
        for change in changes where isNews(change) {
            transitions.send(change)
        }
    }

    /// Every change is passed on (banners are withdrawn on any of them),
    /// except a completion that `AttentionNews` says isn't news: from before
    /// this launch, or quiet.
    private func isNews(_ change: AttentionTransition) -> Bool {
        guard change.becameReadyForReview else { return true }
        return AttentionNews.kinds(from: change.from, to: change.to,
                                   isQuietCompletion: change.session.completionIsQuiet,
                                   completedAt: change.session.completedAt,
                                   launchedAt: launchedAt).contains(.readyForReview)
    }
}
