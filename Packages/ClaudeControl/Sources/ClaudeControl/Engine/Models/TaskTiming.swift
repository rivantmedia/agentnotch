//
//  TaskTiming.swift
//  ClaudeControl
//
//  How far a session's task list is and how long the rest should take,
//  measured from the list's own times (SessionTaskList records when each
//  task was created, started and completed):
//
//  - pace: the wall-clock time each completed task took, from its start (or
//    the previous completion, when it never went in progress, or was started
//    before that completion) to its completion, averaged. Tasks completed
//    together count as quick ones, so the average stays the list's
//    throughput. With three or more timed tasks, one far longer than the
//    rest (a turn left waiting overnight) counts as at most four times their
//    median.
//  - progress: completed tasks, plus the one in progress credited for the
//    time it has run against the pace (at most 90% of it, so the bar never
//    reads done before Claude says so).
//  - remaining: the pace for every task not completed, less what the one in
//    progress has already run.
//
//  Nothing is invented: until a completed task has been timed there is no
//  pace, so no estimate and no credit for the task in progress.
//

import Foundation

nonisolated struct TaskTiming: Equatable, Sendable {
    var completed: Int
    var total: Int
    /// Wall-clock seconds a task has taken on average; nil until a completed
    /// task has been timed.
    var secondsPerTask: TimeInterval?
    /// When the task in progress started; nil when none is, or its start
    /// wasn't seen.
    var activeSince: Date?

    /// The task in progress is credited at most this share of a task.
    static let maxActiveCredit = 0.9
    /// From this many timed tasks on, outliers are capped.
    static let outlierSampleCount = 3
    /// An outlier counts as at most this many times the median.
    static let outlierCap = 4.0

    /// Done so far, 0...1: completed tasks plus credit for the one in
    /// progress. 1 only when every task is completed.
    func fraction(now: Date) -> Double {
        guard total > 0 else { return 0 }
        guard completed < total else { return 1 }
        var done = Double(completed)
        if let secondsPerTask, let activeSince {
            let ran = max(0, now.timeIntervalSince(activeSince))
            done += min(ran / secondsPerTask, Self.maxActiveCredit)
        }
        return min(done / Double(total), 1)
    }

    /// Whole percent done, rounded down (so 100 means every task is completed).
    func percent(now: Date) -> Int {
        // The nudge keeps 0.95 × 100 (94.99999…) from reading 94.
        Int((fraction(now: now) * 100 + 1e-9).rounded(.down))
    }

    /// Seconds the tasks not completed should still take; nil when every
    /// task is completed (or there are none), or there is no pace yet.
    func remaining(now: Date) -> TimeInterval? {
        let left = total - completed
        guard left > 0, let secondsPerTask else { return nil }
        guard let activeSince else { return Double(left) * secondsPerTask }
        let ran = max(0, now.timeIntervalSince(activeSince))
        // A task running over its share has nothing left to estimate; the
        // tasks after it still have theirs.
        return max(secondsPerTask - ran, 0) + Double(left - 1) * secondsPerTask
    }

    /// What a progress bar shows at `now` (see TaskEstimate).
    func estimate(now: Date) -> TaskEstimate {
        let remaining = remaining(now: now)
        var activeCredit = 0.0
        if completed < total, let secondsPerTask, let activeSince {
            activeCredit = min(max(0, now.timeIntervalSince(activeSince)) / secondsPerTask, Self.maxActiveCredit)
        }
        return TaskEstimate(
            percent: percent(now: now),
            activePercent: Int((activeCredit * 100 + 1e-9).rounded(.down)),
            remaining: Self.remainingLabel(remaining),
            remainingShort: Self.remainingShort(remaining),
            spokenRemaining: Self.spokenRemaining(remaining)
        )
    }

    // MARK: - Pace

    /// Average wall-clock seconds per completed task (see the file comment);
    /// nil when no completed task can be timed, or all took no time at all.
    static func secondsPerTask(_ items: [SessionTaskItem]) -> TimeInterval? {
        let completions = items
            .filter { $0.status == .completed }
            .compactMap { item in item.completedAt.map { (item: item, at: $0) } }
            .sorted { $0.at < $1.at }
        var samples: [TimeInterval] = []
        var previousCompletion: Date?
        for (item, completedAt) in completions {
            defer { previousCompletion = completedAt }
            // Its own start, unless an earlier task finished after it: that
            // stretch was already counted for the earlier one.
            let starts = [item.startedAt, previousCompletion].compactMap { $0 }
            guard let start = starts.max() ?? item.createdAt else { continue }
            samples.append(max(0, completedAt.timeIntervalSince(start)))
        }
        guard !samples.isEmpty else { return nil }

        let timed = samples.filter { $0 > 0 }.sorted()
        if timed.count >= outlierSampleCount {
            let cap = timed[timed.count / 2] * outlierCap
            samples = samples.map { min($0, cap) }
        }
        let average = samples.reduce(0, +) / Double(samples.count)
        return average > 0 ? average : nil
    }

    // MARK: - Labels

    /// "~4m left", "~1h 25m left", "almost done"; nil without an estimate.
    static func remainingLabel(_ seconds: TimeInterval?) -> String? {
        guard let short = remainingShort(seconds) else { return nil }
        return short == almostDone ? short : "\(short) left"
    }

    /// "~4m", "~1h 25m", "almost done": the label without "left", for tight spots.
    static func remainingShort(_ seconds: TimeInterval?) -> String? {
        guard let minutes = roundedMinutes(seconds) else { return nil }
        guard minutes > 0 else { return almostDone }
        if minutes < 60 { return "~\(minutes)m" }
        let hours = minutes / 60
        let rest = minutes % 60
        return rest == 0 ? "~\(hours)h" : "~\(hours)h \(rest)m"
    }

    /// "about 4 minutes left", "about 1 hour 25 minutes left", "almost done".
    static func spokenRemaining(_ seconds: TimeInterval?) -> String? {
        guard let minutes = roundedMinutes(seconds) else { return nil }
        guard minutes > 0 else { return almostDone }
        func unit(_ count: Int, _ name: String) -> String { "\(count) \(name)\(count == 1 ? "" : "s")" }
        let hours = minutes / 60
        let rest = minutes % 60
        let parts = [hours > 0 ? unit(hours, "hour") : nil, rest > 0 ? unit(rest, "minute") : nil].compactMap { $0 }
        return "about \(parts.joined(separator: " ")) left"
    }

    static let almostDone = "almost done"
    /// Under this many seconds, the estimate reads "almost done".
    static let almostDoneBelow: TimeInterval = 45

    /// Minutes to show: 0 for "almost done", nearest minute up to an hour,
    /// then the nearest 5 (a "1h 23m" estimate is more precise than it is),
    /// then the nearest hour from 10 hours on. Nil without an estimate.
    static func roundedMinutes(_ seconds: TimeInterval?) -> Int? {
        guard let seconds, seconds.isFinite, seconds >= 0 else { return nil }
        if seconds < almostDoneBelow { return 0 }
        // A week is already more than any session's estimate means.
        let minutes = min(seconds, 7 * 86_400) / 60
        if minutes < 59.5 { return max(1, Int(minutes.rounded())) }
        if minutes < 600 { return Int((minutes / 5).rounded()) * 5 }
        return Int((minutes / 60).rounded()) * 60
    }
}

/// A task list's progress as a row draws it at one moment: whole numbers and
/// labels, so a row compares equal (and doesn't redraw) until one of them
/// changes.
nonisolated struct TaskEstimate: Equatable, Sendable {
    /// Done so far, crediting the task in progress, 0...100.
    var percent: Int
    /// How much of the task in progress is credited, 0...90.
    var activePercent: Int
    /// "~4m left", "almost done"; nil without a pace (or with nothing left).
    var remaining: String?
    /// "~4m", "almost done".
    var remainingShort: String?
    /// "about 4 minutes left", for VoiceOver.
    var spokenRemaining: String?
}
