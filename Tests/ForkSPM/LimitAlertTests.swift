import ClaudeControl
import Foundation
import Testing
@testable import Codenotch

/// LIM1/LIM2: a Claude account's usage limit is told once. Codenotch's own
/// alerts see a Claude ring like any provider's; these are the rules the
/// bridge lays over them, and the watcher behaviour they rely on.
@MainActor
@Suite struct LimitAlertTests {
    private let ring = "claude-acct-0123456789ab"
    private let now = Date(timeIntervalSince1970: 1_790_254_200)

    private func snapshot(used: Double, resetsAt: Date?) throws -> ProviderSnapshot {
        let reading = ClaudeRingReading(windows: [
            .init(id: "session", usedFraction: used, resetsAt: resetsAt, duration: 5 * 3600),
            .init(id: "weekly_all", usedFraction: 0.4, resetsAt: now.addingTimeInterval(3 * 86_400),
                  duration: 7 * 86_400),
        ], updatedAt: now, status: .ok)
        return try ClaudeUsageProvider.snapshot(ringID: ring, displayName: "Claude Work", reading: reading, now: now)
    }

    /// The threshold banner at 100% duplicates "limit reached": dropped for a
    /// Claude ring only, including the 80% banner of a reading that jumped
    /// straight to 100% ("… is at 100%"). A real 80% crossing stays.
    @Test func theFullThresholdBannerIsTheLimitWatchersForClaudeRings() throws {
        #expect(!ClaudeBridge.forwardsThresholdAlert(providerID: ring, threshold: 100, usedPercent: 100))
        #expect(!ClaudeBridge.forwardsThresholdAlert(providerID: ring, threshold: 80, usedPercent: 100))
        #expect(ClaudeBridge.forwardsThresholdAlert(providerID: ring, threshold: 80, usedPercent: 85))
        #expect(ClaudeBridge.forwardsThresholdAlert(providerID: "codex", threshold: 100, usedPercent: 100))
        #expect(ClaudeBridge.forwardsThresholdAlert(providerID: "codex", threshold: 80, usedPercent: 100))

        func delivered(_ readings: [Double]) throws -> [Int] {
            var thresholds: [Int] = []
            let notifier = ThresholdNotifier(deliver: { alert in
                if ClaudeBridge.forwardsThresholdAlert(providerID: alert.providerID, threshold: alert.threshold,
                                                       usedPercent: alert.usedPercent) {
                    thresholds.append(alert.threshold)
                }
            })
            let resets = now.addingTimeInterval(3 * 3600)
            for used in readings {
                notifier.observe([try snapshot(used: used, resetsAt: resets)])
            }
            return thresholds
        }
        #expect(try delivered([0.5, 0.85, 1.0]) == [80])
        #expect(try delivered([0.5, 1.0]) == [])
    }

    /// Codenotch's limit watcher takes any later reset time for a new window.
    /// The engine keeps one reset time per window
    /// (`UsageRingWindows.keepingResetTimes`), so a ring at 100% reads the
    /// same reset time whichever source reported last, and "limit reached"
    /// comes once.
    @Test func aSteadyResetTimeAnnouncesTheLimitOnce() throws {
        let resets = now.addingTimeInterval(3 * 3600)
        func fired(_ resetTimes: [Date]) throws -> Int {
            var events = 0
            let watcher = UsageLimitWatcher(deliver: { _ in events += 1 })
            watcher.observe([try snapshot(used: 0.6, resetsAt: resets)])
            for time in resetTimes {
                watcher.observe([try snapshot(used: 1.0, resetsAt: time)])
            }
            return events
        }
        #expect(try fired([resets, resets, resets]) == 1)
        // What the rounding between sources did before: each move later re-armed it.
        let rounded = resets.addingTimeInterval(0.257626)
        #expect(try fired([resets, rounded, resets, rounded]) == 3)
    }

    /// Other providers, and resets, are never held back.
    @Test func onlyAClaudeRingsLimitIsGated() {
        let codex = UsageAlertEvent(kind: .sessionLimitReached, providerID: "codex", providerName: "Codex",
                                    windowLabel: "5-hour limit", glyph: .claude, previousFraction: 0.9,
                                    currentFraction: 1, resetsAt: nil)
        #expect(ClaudeBridge.shared.claimsLimitAlert(codex, bringsSound: true))
        let reset = UsageAlertEvent(kind: .reset, providerID: ring, providerName: "Claude Work",
                                    windowLabel: "5-hour limit", glyph: .claude, previousFraction: 1,
                                    currentFraction: 0, resetsAt: nil)
        #expect(ClaudeBridge.shared.claimsLimitAlert(reset, bringsSound: true))
    }
}
