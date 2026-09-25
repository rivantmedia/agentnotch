import Foundation
import Testing
@testable import ClaudeControl

/// Ring readings (design §4.3): window ids, labels, order, money, the five
/// statuses and staleness, plus the limit a rate-limited session waits for.
struct A3_RingReadingTests {
    let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func window(_ utilization: Double, resetsIn: TimeInterval, _ duration: TimeInterval = UsageWindow.weeklyDuration) -> UsageWindow {
        UsageWindow(utilization: utilization, resetsAt: now.addingTimeInterval(resetsIn), duration: duration)
    }

    private var fullUsage: AccountUsage {
        AccountUsage(
            accountId: "a",
            fiveHour: window(34, resetsIn: 3600, UsageWindow.sessionDuration),
            sevenDay: window(41, resetsIn: 3 * 86400),
            scoped: [
                ScopedUsage(name: "Sonnet 4.5", window: window(12, resetsIn: 3 * 86400)),
                ScopedUsage(name: "Opus", window: window(25, resetsIn: 3 * 86400)),
            ],
            extraUsage: ExtraUsage(isEnabled: true, monthlyLimit: 5_000, usedCredits: 1_240, utilization: 24.8, currency: "usd"),
            subscriptionType: "max",
            source: .probe,
            updatedAt: now.addingTimeInterval(-90)
        )
    }

    // MARK: Ids, labels, order, money

    @Test func idsLabelsAndOrderMirrorCodenotch() {
        let reading = ClaudeHostProjections.ringReading(usage: fullUsage, fetchState: .idle, now: now)
        #expect(reading.windows.map(\.id) == ["session", "weekly_all", "extra_usage", "weekly_opus", "weekly_sonnet_4_5"])
        // Codenotch labels the two standard windows itself.
        #expect(reading.windows[0].label == nil && reading.windows[1].label == nil)
        #expect(reading.windows.first { $0.id == "weekly_opus" }?.label == "Opus")
        #expect(reading.windows.first { $0.id == "weekly_sonnet_4_5" }?.label == "Sonnet 4.5")
        #expect(abs(reading.windows[0].usedFraction - 0.34) < 1e-9)
        #expect(reading.windows[0].duration == UsageWindow.sessionDuration)
        #expect(reading.windows[1].resetsAt == now.addingTimeInterval(3 * 86400))
        #expect(reading.plan == "Max")
        #expect(reading.updatedAt == now.addingTimeInterval(-90))
        #expect(reading.status == .ok)
    }

    @Test func extraUsageIsMoneyInMajorUnits() throws {
        let reading = ClaudeHostProjections.ringReading(usage: fullUsage, fetchState: nil, now: now)
        let extra = try #require(reading.windows.first { $0.id == "extra_usage" })
        #expect(extra.label == "Extra usage")
        #expect(abs(extra.usedFraction - 0.248) < 1e-9)
        #expect(extra.money == .init(currency: "USD", spent: 12.40, remaining: 37.60))
        // Switched off: no window.
        var off = fullUsage
        off.extraUsage?.isEnabled = false
        #expect(!ClaudeHostProjections.ringReading(usage: off, fetchState: nil, now: now).windows.contains { $0.id == "extra_usage" })
        // Zero-decimal currencies are not divided.
        let yen = UsageRingWindows.extraUsageWindow(ExtraUsage(isEnabled: true, monthlyLimit: 5000, usedCredits: 1000, utilization: nil, currency: "JPY"))
        #expect(yen?.money == .init(currency: "JPY", spent: 1000, remaining: 4000))
        #expect(yen.map { abs($0.usedFraction - 0.2) < 1e-9 } == true)
    }

    @Test func aResetWindowReadsZero() {
        var usage = fullUsage
        usage.fiveHour = window(88, resetsIn: -60, UsageWindow.sessionDuration)
        let reading = ClaudeHostProjections.ringReading(usage: usage, fetchState: nil, now: now)
        #expect(reading.windows.first { $0.id == "session" }?.usedFraction == 0)
    }

    @Test func labelsForIdsMatchCodenotchWording() {
        #expect(UsageRingWindows.label(forID: "session") == "Current session")
        #expect(UsageRingWindows.label(forID: "weekly_all") == "All models")
        #expect(UsageRingWindows.label(forID: "weekly_opus") == "Opus")
        #expect(UsageRingWindows.label(forID: "weekly_scoped") == "Scoped")
        #expect(UsageRingWindows.label(forID: "weekly_fable") == "Fable")
        #expect(UsageRingWindows.scopedID(forModel: "Sonnet 4.5") == "weekly_sonnet_4_5")
        #expect(UsageRingWindows.scopedID(forModel: "  ") == "weekly_scoped")
    }

    // MARK: Statuses

    @Test func theFiveStatuses() {
        typealias P = ClaudeHostProjections
        #expect(P.ringReading(usage: fullUsage, fetchState: .failed("x"), now: now).status == .ok)
        #expect(P.ringReading(usage: nil, fetchState: nil, now: now).status == .waitingForFirstReading)
        #expect(P.ringReading(usage: nil, fetchState: .fetching, now: now).status == .waitingForFirstReading)
        #expect(P.ringReading(usage: nil, fetchState: .idle, now: now).status == .waitingForFirstReading)
        #expect(P.ringReading(usage: nil, fetchState: UsageStore.notSignedIn, now: now).status == .signInNeeded("Not signed in to Claude"))
        // Signed out wins even over old windows (they belong to the last login).
        #expect(P.ringReading(usage: fullUsage, fetchState: UsageStore.notSignedIn, now: now).status == .signInNeeded("Not signed in to Claude"))
        #expect(P.ringReading(usage: nil, fetchState: .unavailable("Usage probes are off"), now: now).status == .unavailable("Usage probes are off"))
        #expect(P.ringReading(usage: nil, fetchState: .failed("Claude Code not found"), now: now).status == .failed("Claude Code not found"))
        let empty = P.ringReading(usage: nil, fetchState: nil, now: now)
        #expect(empty.windows.isEmpty && empty.updatedAt == nil)
    }

    // MARK: Staleness

    @Test func staleThresholdFollowsTheProbeInterval() {
        #expect(AccountUsage.staleThreshold(probeIntervalMinutes: 5) == 15 * 60)
        #expect(AccountUsage.staleThreshold(probeIntervalMinutes: 10) == 15 * 60)
        #expect(AccountUsage.staleThreshold(probeIntervalMinutes: 15) == 22.5 * 60)
        #expect(AccountUsage.staleThreshold(probeIntervalMinutes: 30) == 45 * 60)
        #expect(AccountUsage.staleThreshold(probeIntervalMinutes: 0) == AccountUsage.staleAfterWithoutProbes)
        #expect(AccountUsage.staleAfterWithoutProbes > 45 * 60)
    }

    @Test func readingsGoStaleAfterTheirThreshold() {
        var usage = fullUsage
        usage.updatedAt = now.addingTimeInterval(-20 * 60)
        var reading = ClaudeHostProjections.ringReading(usage: usage, fetchState: nil, staleThreshold: 15 * 60, now: now)
        #expect(reading.staleThreshold == 15 * 60)
        #expect(reading.isStale(now: now))
        // At the 30-minute setting, 20 minutes is not stale yet.
        reading.staleThreshold = AccountUsage.staleThreshold(probeIntervalMinutes: 30)
        #expect(!reading.isStale(now: now))
        // Only `.ok` readings are ever stale.
        #expect(!ClaudeRingReading(status: .waitingForFirstReading).isStale(now: now))
        #expect(!usage.isStale(now: usage.updatedAt.addingTimeInterval(60)))
        #expect(usage.isStale(now: now, threshold: 15 * 60))
    }

    @Test func aUsedUpWindowNeverGoesStaleBeforeItsReset() throws {
        var usage = fullUsage
        usage.sevenDay = window(100, resetsIn: 2 * 86400)
        usage.updatedAt = now.addingTimeInterval(-3 * 3600)
        let reading = ClaudeHostProjections.ringReading(usage: usage, fetchState: nil, staleThreshold: 15 * 60, now: now)
        #expect(!reading.isStale(now: now))
        #expect(!usage.isStale(now: now))
        let exhausted = try #require(reading.exhaustedWindow(now: now))
        #expect(exhausted.id == "weekly_all")
        // Once it has reset, the old reading is stale like any other.
        let afterReset = now.addingTimeInterval(2 * 86400 + 60)
        #expect(reading.exhaustedWindow(now: afterReset) == nil)
        #expect(reading.isStale(now: afterReset))
    }

    // MARK: The limit a session waits for

    @Test func limitHitPicksTheWindowThatLiftsLast() {
        var usage = fullUsage
        #expect(usage.limitHit(now: now) == nil)
        usage.fiveHour = window(104, resetsIn: 47 * 60, UsageWindow.sessionDuration)
        #expect(usage.limitHit(now: now) == UsageLimitHit(window: .session, resetsAt: now.addingTimeInterval(47 * 60)))
        // The week is used up too: waiting for the session isn't enough.
        usage.sevenDay = window(100, resetsIn: 2 * 86400)
        #expect(usage.limitHit(now: now) == UsageLimitHit(window: .weekly, resetsAt: now.addingTimeInterval(2 * 86400)))
        // A model's week that lifts even later.
        usage.scoped = [ScopedUsage(name: "Opus", window: window(100, resetsIn: 3 * 86400))]
        #expect(usage.limitHit(now: now)?.window == .scoped("Opus"))
        // A window whose reset passed restarted at 0%.
        #expect(usage.limitHit(now: now.addingTimeInterval(4 * 86400)) == nil)

        let reading = ClaudeHostProjections.ringReading(usage: usage, fetchState: nil, now: now)
        #expect(reading.exhaustedWindow(now: now)?.id == "weekly_opus")
    }

    @Test func moneyWindowsNeverBlock() {
        let reading = ClaudeRingReading(windows: [
            .init(id: "extra_usage", usedFraction: 1.2, money: .init(currency: "USD", spent: 60, remaining: 0)),
        ], updatedAt: now, status: .ok)
        #expect(reading.exhaustedWindow(now: now) == nil)
    }

    // MARK: Claude Desktop readings

    @Test func desktopWindowsBecomeAFullSnapshot() throws {
        let observed = now.addingTimeInterval(-30)
        let external = ClaudeExternalUsageReading(windows: [
            .init(id: "session", usedFraction: 0.5, resetsAt: now.addingTimeInterval(3600), duration: 18_000),
            .init(id: "weekly_all", usedFraction: 0.2, resetsAt: now.addingTimeInterval(86400), duration: 604_800),
            .init(id: "weekly_scoped", label: "Fable", usedFraction: 0.1, resetsAt: now.addingTimeInterval(86400)),
            .init(id: "weekly_opus", usedFraction: 0.3, resetsAt: now.addingTimeInterval(86400)),
            .init(id: "extra_usage", usedFraction: 0.25, money: .init(currency: "USD", spent: 12.5, remaining: 37.5)),
            .init(id: "something_new", usedFraction: 0.9),
        ], observedAt: observed)
        let usage = try #require(UsageRingWindows.accountUsage(from: external, accountId: "a"))
        #expect(usage.updatedAt == observed)
        #expect(usage.source == .cache)
        #expect(usage.fiveHour?.utilization == 50)
        #expect(usage.sevenDay?.utilization == 20)
        #expect(usage.scoped.map(\.name) == ["Fable", "Opus"])
        #expect(usage.extraUsage == ExtraUsage(isEnabled: true, monthlyLimit: 5_000, usedCredits: 1_250, utilization: 25, currency: "USD"))
        // And back: the same ring windows.
        let windows = UsageRingWindows.windows(from: usage, now: now)
        #expect(windows.map(\.id) == ["session", "weekly_all", "extra_usage", "weekly_fable", "weekly_opus"])
        #expect(windows.first { $0.id == "extra_usage" }?.money == .init(currency: "USD", spent: 12.5, remaining: 37.5))

        // Nothing to draw a ring from: no snapshot.
        #expect(UsageRingWindows.accountUsage(from: .init(windows: [], observedAt: now), accountId: "a") == nil)
    }
}
