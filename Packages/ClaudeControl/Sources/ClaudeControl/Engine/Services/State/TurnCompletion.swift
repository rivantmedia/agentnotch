//
//  TurnCompletion.swift
//  ClaudeControl
//
//  When a Stop is the end of a turn. Our Stop hook is one of several that
//  Claude Code runs in parallel at the end of a turn; a blocking one (the
//  built-in /goal, plugin loops) makes Claude continue, up to 8 times, with
//  no new prompt. So a Stop is only a pending completion until Claude Code's
//  session registry (`<configDir>/sessions/<pid>.json`) reports the session
//  idle after it — it stays busy while Stop hooks and any continuation run.
//  Sessions the registry doesn't follow fall back to a short quiet period.
//
//  Pure: the store feeds it the session's registry status and the clock.
//

import Foundation

nonisolated enum TurnCompletion {
    enum Decision: Equatable, Sendable {
        /// The turn is over: record the completion.
        case confirm
        /// Claude may still be running Stop hooks or continuing.
        case wait
    }

    /// How long to wait for confirmation.
    struct Timing: Equatable, Sendable {
        /// No registry follows this turn: confirm after this quiet period.
        var fallbackDelay: TimeInterval
        /// The registry follows the turn but never reported it idle.
        var registryTimeout: TimeInterval
        /// Registry and hook timestamps come from different writers.
        var clockTolerance: TimeInterval

        static let standard = Timing(
            fallbackDelay: SessionStore.completionFallbackDelay,
            registryTimeout: SessionStore.completionRegistryTimeout,
            clockTolerance: SessionStore.registryClockTolerance
        )
    }

    /// Whether the Stop at `stopAt` ended the turn.
    static func decide(
        stopAt: Date,
        turnStartedAt: Date?,
        registryStatus: String?,
        registryChangedAt: Date?,
        now: Date,
        timing: Timing = .standard
    ) -> Decision {
        if let status = registryStatus, let changedAt = registryChangedAt {
            let isSettled = status == "idle" || status == "shell"
            if isSettled && changedAt >= stopAt.addingTimeInterval(-timing.clockTolerance) {
                // The registry went idle after the Stop: every Stop hook ran
                // and none continued the turn.
                return .confirm
            }
            if followsTurn(turnStartedAt: turnStartedAt, stopAt: stopAt, registryStatus: status, registryChangedAt: changedAt, timing: timing) {
                return now >= stopAt.addingTimeInterval(timing.registryTimeout) ? .confirm : .wait
            }
        }
        return now >= stopAt.addingTimeInterval(timing.fallbackDelay) ? .confirm : .wait
    }

    /// Seconds until `decide` may change its answer by the clock alone.
    static func checkDelay(
        stopAt: Date,
        turnStartedAt: Date?,
        registryStatus: String?,
        registryChangedAt: Date?,
        now: Date,
        timing: Timing = .standard
    ) -> TimeInterval {
        let follows = registryStatus.flatMap { status in
            registryChangedAt.map { followsTurn(turnStartedAt: turnStartedAt, stopAt: stopAt, registryStatus: status, registryChangedAt: $0, timing: timing) }
        } ?? false
        let deadline = stopAt.addingTimeInterval(follows ? timing.registryTimeout : timing.fallbackDelay)
        return max(deadline.timeIntervalSince(now), 0.05)
    }

    /// The registry reported the session busy (or waiting on a dialog) since
    /// this turn began, so its going idle will mark the end.
    static func followsTurn(
        turnStartedAt: Date?,
        stopAt: Date,
        registryStatus: String,
        registryChangedAt: Date,
        timing: Timing = .standard
    ) -> Bool {
        guard registryStatus == "busy" || registryStatus == "waiting" else { return false }
        let turnStart = turnStartedAt ?? stopAt
        return registryChangedAt >= turnStart.addingTimeInterval(-timing.clockTolerance)
    }
}
