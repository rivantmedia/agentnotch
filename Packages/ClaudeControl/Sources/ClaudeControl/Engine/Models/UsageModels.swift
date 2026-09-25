//
//  UsageModels.swift
//  ClaudeControl
//
//  Plan usage (rate limit) data for one account. The shape follows Claude
//  Code's own `/api/oauth/usage` response, which reaches the app through
//  four read-only routes (no OAuth tokens are ever handled by the app):
//  - `get_usage` asked of Claude Code itself (`claude -p` stream-json control request)
//  - `cachedUsageUtilization` in the account's .claude.json
//  - `rate_limits` in the status line JSON of live terminal sessions
//  - Claude Desktop's HTTP cache, read by the host app
//    (`ClaudeControlConfiguration.externalUsageSource`)
//

import Foundation

/// One rate-limit window, e.g. the 5-hour session or the 7-day week.
nonisolated struct UsageWindow: Codable, Hashable, Sendable {
    /// Percent of the window used, 0...100 (can exceed 100 when over the limit).
    var utilization: Double
    /// When the window resets. Nil if unknown.
    var resetsAt: Date?
    /// Window length in seconds (5h = 18_000, 7d = 604_800).
    var duration: TimeInterval

    static let sessionDuration: TimeInterval = 5 * 60 * 60
    static let weeklyDuration: TimeInterval = 7 * 24 * 60 * 60

    init(utilization: Double, resetsAt: Date?, duration: TimeInterval) {
        self.utilization = utilization
        self.resetsAt = resetsAt
        self.duration = duration
    }

    /// Whether the reset time has passed, meaning the window has restarted at 0%.
    func hasReset(now: Date = Date()) -> Bool {
        guard let resetsAt else { return false }
        return resetsAt <= now
    }

    /// Utilization to display: 0 once the window has reset.
    func effectiveUtilization(now: Date = Date()) -> Double {
        hasReset(now: now) ? 0 : utilization
    }

    /// Fraction used, clamped to 0...1, for drawing rings and bars.
    func fraction(now: Date = Date()) -> Double {
        min(max(effectiveUtilization(now: now) / 100, 0), 1)
    }

    /// How far through the window we are, 0...1. Nil without a reset time.
    func elapsedFraction(now: Date = Date()) -> Double? {
        guard let resetsAt, duration > 0 else { return nil }
        if resetsAt <= now { return 0 }
        let remaining = resetsAt.timeIntervalSince(now)
        return min(max((duration - remaining) / duration, 0), 1)
    }
}

/// A weekly limit scoped to one model family (e.g. Opus, Sonnet, Fable).
nonisolated struct ScopedUsage: Codable, Hashable, Sendable, Identifiable {
    var id: String { name }
    var name: String
    var window: UsageWindow
}

/// Pay-as-you-go "extra usage" credits, when enabled on the account.
nonisolated struct ExtraUsage: Codable, Hashable, Sendable {
    var isEnabled: Bool
    var monthlyLimit: Double?
    var usedCredits: Double?
    /// Percent of the monthly limit used, 0...100.
    var utilization: Double?
    var currency: String?
}

/// Where a usage snapshot came from.
nonisolated enum UsageSource: String, Codable, Sendable {
    /// Asked Claude Code directly (get_usage control request).
    case probe
    /// A live terminal session's status line `rate_limits`.
    case statusLine
    /// A cache another client keeps: `cachedUsageUtilization` in the
    /// account's .claude.json, or Claude Desktop's HTTP cache.
    case cache
}

/// A limit the account has used up: which window, and when it lifts.
nonisolated struct UsageLimitHit: Hashable, Sendable {
    nonisolated enum Window: Hashable, Sendable {
        /// The 5-hour session window.
        case session
        /// The 7-day window across all models.
        case weekly
        /// A 7-day window for one model family ("Opus").
        case scoped(String)
    }

    var window: Window
    /// When the window resets; nil when the source didn't say.
    var resetsAt: Date?
}

/// Latest known usage for one account.
nonisolated struct AccountUsage: Codable, Hashable, Sendable {
    var accountId: String
    /// The 5-hour session window ("Current session").
    var fiveHour: UsageWindow?
    /// The 7-day window across all models ("Current week").
    var sevenDay: UsageWindow?
    /// Weekly windows scoped to a model family.
    var scoped: [ScopedUsage]
    var extraUsage: ExtraUsage?
    /// "max", "pro", ... when the source reports it.
    var subscriptionType: String?
    var source: UsageSource
    /// When the underlying data was fetched by Claude Code (not when the app read it).
    var updatedAt: Date

    init(
        accountId: String,
        fiveHour: UsageWindow? = nil,
        sevenDay: UsageWindow? = nil,
        scoped: [ScopedUsage] = [],
        extraUsage: ExtraUsage? = nil,
        subscriptionType: String? = nil,
        source: UsageSource,
        updatedAt: Date
    ) {
        self.accountId = accountId
        self.fiveHour = fiveHour
        self.sevenDay = sevenDay
        self.scoped = scoped
        self.extraUsage = extraUsage
        self.subscriptionType = subscriptionType
        self.source = source
        self.updatedAt = updatedAt
    }

    /// Data older than this is shown as stale, at the default probe interval.
    static let staleAfter: TimeInterval = 15 * 60
    /// With automatic probes off, only status lines and caches bring news;
    /// an hour without any is when the reading stops being trustworthy.
    static let staleAfterWithoutProbes: TimeInterval = 60 * 60

    /// How old a reading may get before it counts as stale, for a probe
    /// interval in minutes (0 = automatic probes off): never less than 15
    /// minutes, and never less than one and a half probe intervals, so a
    /// reading isn't stale merely because the next probe isn't due yet. Pure.
    static func staleThreshold(probeIntervalMinutes minutes: Int) -> TimeInterval {
        guard minutes > 0 else { return staleAfterWithoutProbes }
        return max(staleAfter, TimeInterval(minutes) * 60 * 1.5)
    }

    /// Whether the reading is too old to trust. A used-up window is never
    /// stale before its reset: it cannot come down until then, however old
    /// the reading is.
    func isStale(now: Date = Date(), threshold: TimeInterval = AccountUsage.staleAfter) -> Bool {
        guard now.timeIntervalSince(updatedAt) > threshold else { return false }
        return limitHit(now: now) == nil
    }

    /// The used-up window the account is waiting on, if any: of the windows
    /// at or over 100% that haven't reset yet, the one that resets last (a
    /// session limit hit during a used-up week lifts before the week does).
    /// A window at 100% with no known reset still counts. Pure.
    func limitHit(now: Date = Date()) -> UsageLimitHit? {
        var candidates: [UsageLimitHit] = []
        func consider(_ window: UsageWindow?, as kind: UsageLimitHit.Window) {
            guard let window, window.effectiveUtilization(now: now) >= 100 else { return }
            candidates.append(UsageLimitHit(window: kind, resetsAt: window.resetsAt))
        }
        consider(fiveHour, as: .session)
        consider(sevenDay, as: .weekly)
        for scoped in scoped {
            consider(scoped.window, as: .scoped(scoped.name))
        }
        return candidates.max { lhs, rhs in
            (lhs.resetsAt ?? .distantFuture) < (rhs.resetsAt ?? .distantFuture)
        }
    }
}

/// Fetch status for one account, shown next to its usage.
nonisolated enum UsageFetchState: Equatable, Sendable {
    case idle
    case fetching
    /// The last attempt failed; the message is short and user-facing.
    case failed(String)
    /// Usage can't be fetched for this account (e.g. API-key login, not claude.ai).
    case unavailable(String)
}

extension AccountUsage {
    /// Sample data for previews and UI snapshots.
    static func sample(accountId: String, session: Double, weekly: Double, now: Date = Date()) -> AccountUsage {
        AccountUsage(
            accountId: accountId,
            fiveHour: UsageWindow(
                utilization: session,
                resetsAt: now.addingTimeInterval(2 * 60 * 60 + 13 * 60),
                duration: UsageWindow.sessionDuration
            ),
            sevenDay: UsageWindow(
                utilization: weekly,
                resetsAt: now.addingTimeInterval(3 * 24 * 60 * 60 + 5 * 60 * 60),
                duration: UsageWindow.weeklyDuration
            ),
            scoped: [
                ScopedUsage(
                    name: "Opus",
                    window: UsageWindow(
                        utilization: weekly * 0.6,
                        resetsAt: now.addingTimeInterval(3 * 24 * 60 * 60 + 5 * 60 * 60),
                        duration: UsageWindow.weeklyDuration
                    )
                ),
            ],
            extraUsage: ExtraUsage(isEnabled: false, monthlyLimit: nil, usedCredits: nil, utilization: nil, currency: nil),
            subscriptionType: "max",
            source: .probe,
            updatedAt: now.addingTimeInterval(-90)
        )
    }
}
