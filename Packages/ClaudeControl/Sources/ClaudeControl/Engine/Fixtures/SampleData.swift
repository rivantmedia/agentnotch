//
//  SampleData.swift
//  ClaudeControl
//
//  Sample accounts and usage for fixtures, snapshots and sealed mode.
//  Deterministic: every date is relative to `SampleData.now`.
//  Ported from Superpowered Vibe Notch's SnapshotRenderer (the renderer
//  itself is dropped; ClaudeControlSnapshots replaces it).
//

import Foundation

enum SampleData {
    /// Fixed clock so the snapshots are reproducible: a Tuesday late morning.
    static let now: Date = {
        var components = DateComponents()
        components.year = 2026
        components.month = 9
        components.day = 22
        components.hour = 11
        components.minute = 47
        return Calendar.current.date(from: components) ?? Date()
    }()

    static var personal: ClaudeAccount { SampleSessions.personal }
    static var work: ClaudeAccount { SampleSessions.work }
    static let side = ClaudeAccount(
        configDir: "~/.claude-side",
        configDirEnv: AccountPaths.normalize("~/.claude-side"),
        customLabel: "Side project",
        email: "hello@side.dev",
        accountUuid: SampleLayout.sideUUID,
        subscriptionType: "pro",
        colorIndex: 2
    )
    /// A config dir nobody has logged in to.
    static let scratch = ClaudeAccount(
        configDir: "~/.claude-scratch",
        configDirEnv: AccountPaths.normalize("~/.claude-scratch"),
        colorIndex: 3
    )

    static let accounts = [personal, work, side]

    static func usage(now: Date) -> [String: AccountUsage] {
        var personalUsage = AccountUsage.sample(accountId: personal.id, session: 34, weekly: 41, now: now)
        personalUsage.scoped.append(
            ScopedUsage(name: "Sonnet", window: window(12, resetsIn: 3 * day + 5 * hour, duration: UsageWindow.weeklyDuration, now: now))
        )
        // The work account has used up its session window: its rate-limited
        // sample session waits for this reset.
        var workUsage = AccountUsage.sample(accountId: work.id, session: 100, weekly: 64, now: now)
        // A later point in both windows than the sample's defaults.
        workUsage.fiveHour?.resetsAt = now.addingTimeInterval(1 * hour + 6 * minute)
        workUsage.sevenDay?.resetsAt = now.addingTimeInterval(1 * day + 21 * hour)
        workUsage.scoped = workUsage.scoped.map { scoped in
            var copy = scoped
            copy.window.resetsAt = workUsage.sevenDay?.resetsAt
            return copy
        }
        workUsage.source = .statusLine
        workUsage.updatedAt = now.addingTimeInterval(-20)
        return [
            personal.id: personalUsage,
            work.id: workUsage,
            side.id: aheadOfPace(accountId: side.id, now: now),
        ]
    }

    /// Session burning faster than the window allows.
    static func aheadOfPace(accountId: String, now: Date) -> AccountUsage {
        var usage = AccountUsage.sample(accountId: accountId, session: 72, weekly: 30, now: now)
        usage.sevenDay?.resetsAt = now.addingTimeInterval(5 * day + 2 * hour)
        usage.scoped = []
        usage.subscriptionType = "pro"
        usage.source = .cache
        usage.updatedAt = now.addingTimeInterval(-4 * 60)
        return usage
    }

    /// Session over its limit, weekly nearly out, extra usage switched on.
    static func limitReached(accountId: String, now: Date) -> AccountUsage {
        AccountUsage(
            accountId: accountId,
            fiveHour: window(110, resetsIn: 47 * minute, duration: UsageWindow.sessionDuration, now: now),
            sevenDay: window(92, resetsIn: 1 * day + 21 * hour, duration: UsageWindow.weeklyDuration, now: now),
            scoped: [
                ScopedUsage(name: "Opus", window: window(96, resetsIn: 1 * day + 21 * hour, duration: UsageWindow.weeklyDuration, now: now)),
                ScopedUsage(name: "Sonnet", window: window(38, resetsIn: 1 * day + 21 * hour, duration: UsageWindow.weeklyDuration, now: now)),
            ],
            extraUsage: ExtraUsage(isEnabled: true, monthlyLimit: 5_000, usedCredits: 1_240, utilization: 24.8, currency: "USD"),
            subscriptionType: "team",
            source: .probe,
            updatedAt: now.addingTimeInterval(-26 * 60)
        )
    }

    /// Old snapshot: shown desaturated with a stale footer.
    static func stale(accountId: String, now: Date) -> AccountUsage {
        var usage = AccountUsage.sample(accountId: accountId, session: 45, weekly: 30, now: now)
        usage.scoped = []
        usage.source = .cache
        usage.updatedAt = now.addingTimeInterval(-2 * hour - 8 * minute)
        return usage
    }

    /// Status line data: session and week only, no reset for the week.
    static func partial(accountId: String, now: Date) -> AccountUsage {
        AccountUsage(
            accountId: accountId,
            fiveHour: window(18, resetsIn: 4 * hour + 20 * minute, duration: UsageWindow.sessionDuration, now: now),
            sevenDay: UsageWindow(utilization: 55, resetsAt: nil, duration: UsageWindow.weeklyDuration),
            source: .statusLine,
            updatedAt: now.addingTimeInterval(-50)
        )
    }

    // MARK: Helpers

    static let minute: TimeInterval = 60
    static let hour: TimeInterval = 60 * 60
    static let day: TimeInterval = 24 * 60 * 60

    static func window(_ utilization: Double, resetsIn: TimeInterval, duration: TimeInterval, now: Date) -> UsageWindow {
        UsageWindow(utilization: utilization, resetsAt: now.addingTimeInterval(resetsIn), duration: duration)
    }
}
