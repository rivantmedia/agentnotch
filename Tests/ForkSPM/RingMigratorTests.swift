import ClaudeControl
import Foundation
import Testing
@testable import Codenotch

/// The user's upgrade: five per-folder Claude rings (`claude`, `claude-shared`,
/// `claude-claude`, `claude-paras`, `claude-paras-rivant-in`) become two
/// account rings, with Codenotch's choices moved over and the rest gone.
@MainActor
@Suite(.serialized) struct RingMigratorTests {
    private let suite = "agentnotch-ring-migrator-\(UUID().uuidString)"

    private func account(_ ring: String, former: [String]) -> ClaudeAccountSummary {
        var summary = ClaudeAccountSummary(id: ring, ringID: ring, configDir: "/h", label: ring, isDefault: false,
                                           launchCommand: "claude")
        summary.formerRingIDs = former
        return summary
    }

    private func snapshot(_ id: String, percent: Double) -> ProviderSnapshot {
        ProviderSnapshot(id: id, displayName: id, glyph: .claude, fidelity: .official, status: .ok,
                         windows: [LimitWindow(id: "session", label: "Current session", usedFraction: percent / 100)],
                         headlineID: "session", weeklyID: nil, tokenUsage: nil, usageDetail: nil)
    }

    @Test func theUsersFiveRingsBecomeTwo() throws {
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = Preferences(defaults: defaults)
        let old = ["claude", "claude-shared", "claude-claude", "claude-paras", "claude-paras-rivant-in"]
        preferences.reconcile(discoveredIDs: old + ["codex"])
        preferences.setConnected(false, for: "claude-claude")
        preferences.setNickname("Main", for: "claude")
        preferences.setNickname("Shared", for: "claude-shared")
        preferences.providerOrder = ["claude", "claude-shared", "claude-claude", "claude-paras", "claude-paras-rivant-in", "codex"]
        let archive = UsageArchive(defaults: defaults)
        archive.save(["claude": (snapshot("claude", percent: 34), Date()),
                      "claude-claude": (snapshot("claude-claude", percent: 12), Date()),
                      "claude-shared": (snapshot("claude-shared", percent: 99), Date())])

        let paras = account("claude-acct-p", former: ["claude", "claude-dir-aaaaaaaa", "claude-paras", "claude-paras-rivant-in"])
        let biios = account("claude-acct-b", former: ["claude-dir-bbbbbbbb", "claude-claude"])
        // At launch, before the store reads the archive.
        ClaudeRingMigrator.migrateArchive(accounts: [paras, biios], retired: ["claude-shared"], defaults: defaults)
        let readings = archive.load()
        #expect(Set(readings.keys) == ["claude-acct-p", "claude-acct-b"])
        #expect(readings["claude-acct-p"]?.snapshot.windows.first?.usedFraction == 0.34)
        #expect(readings["claude-acct-b"]?.snapshot.id == "claude-acct-b")

        // The new rings arrive (AppDelegate reconciles them: new Claude ids are on).
        preferences.reconcile(discoveredIDs: ["claude-acct-p", "claude-acct-b", "codex"])
        #expect(ClaudeRingMigrator.migratePreferences(accounts: [paras, biios], retired: ["claude-shared"],
                                                      preferences: preferences, store: nil, defaults: defaults))
        #expect(preferences.nickname(for: "claude-acct-p") == "Main")
        #expect(preferences.nickname(for: "claude") == nil)
        #expect(preferences.nickname(for: "claude-shared") == nil)
        #expect(!preferences.isConnected("claude-acct-b"))   // claude-claude was off
        #expect(preferences.isConnected("claude-acct-p"))
        #expect(preferences.providerOrder == ["claude-acct-p", "claude-acct-b", "codex"])

        // Once: a later choice on the new ring stands.
        preferences.setConnected(true, for: "claude-acct-b")
        #expect(!ClaudeRingMigrator.migratePreferences(accounts: [paras, biios], retired: ["claude-shared"],
                                                       preferences: preferences, store: nil, defaults: defaults))
        #expect(preferences.isConnected("claude-acct-b"))
    }
}
