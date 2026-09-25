import ClaudeControl
import Foundation
import Testing
@testable import Codenotch

/// The bridge's side of the Parallel Profiles review: the menu bar names
/// account rings (PP-C9), Codenotch's Accounts pane says where an account
/// runs (UX-5, UX-10), and muted alerts and the menu-bar choice move with
/// the migration (PP-C8).
@MainActor
@Suite(.serialized) struct PPFixBridgeTests {
    private let suite = "agentnotch-ppfix-bridge-\(UUID().uuidString)"

    @Test func theMenuBarNamesAnAccountRingNotItsHash() {
        #expect(ClaudeBridge.shortMenuBarLabel("Claude Rivant") == "Rivant")
        #expect(ClaudeBridge.shortMenuBarLabel("Work") == "Work")
        #expect(ClaudeBridge.shortMenuBarLabel("Claude") == "Claude")
        // Other rings keep upstream's rule.
        #expect(ClaudeBridge.menuBarLabel(providerID: "claude-work") == nil)
        #expect(ClaudeBridge.menuBarLabel(providerID: "codex") == nil)
        #expect(ClaudeBridge.menuBarLabel(providerID: "claude-acct-d6235e7fc7d7")?.hasPrefix("acct-") == false)
    }

    @Test func theAccountsPaneSaysWhereAnAccountRuns() {
        var account = ClaudeAccountSummary(id: "uuid:p", ringID: "claude-acct-p", configDir: "/h/.claude", label: "Claude Rivant",
                                           isDefault: true, launchCommand: "claude")
        account.runDirs = ["/h/.claude", "/h/.claude-windows/801f9dd51396", "/h/.claude-windows/b9fbb9ecd7cb"]
        account.storeDirs = ["/h/.claude-paras"]
        account.windowCount = 2
        #expect(ClaudeRingNames.source(for: account).hasSuffix("2 VS Code workspaces"))
        // Only its store holds it now: it runs nowhere, and says so.
        account.runDirs = []
        account.windowCount = 0
        account.isDefault = false
        account.configDir = "/h/.claude-paras"
        #expect(ClaudeRingNames.source(for: account) == "Claude Parallel Profiles account (no window open)")
    }

    @Test func mutedAlertsAndTheMenuBarChoiceMoveToTheAccountRing() throws {
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let preferences = Preferences(defaults: defaults)
        let old = ["claude", "claude-shared", "claude-claude", "claude-paras"]
        preferences.reconcile(discoveredIDs: old + ["codex"])
        preferences.setAlertsMuted(true, for: "claude")
        preferences.setAlertsMuted(true, for: "claude-shared")
        preferences.setInMenuBar(true, for: "claude-claude", among: old + ["codex"])
        preferences.setInMenuBar(false, for: "claude", among: old + ["codex"])
        preferences.setInMenuBar(false, for: "claude-paras", among: old + ["codex"])

        func account(_ ring: String, former: [String]) -> ClaudeAccountSummary {
            var summary = ClaudeAccountSummary(id: ring, ringID: ring, configDir: "/h", label: ring, isDefault: false,
                                               launchCommand: "claude")
            summary.formerRingIDs = former
            return summary
        }
        let paras = account("claude-acct-p", former: ["claude", "claude-paras"])
        let biios = account("claude-acct-b", former: ["claude-claude"])
        preferences.reconcile(discoveredIDs: ["claude-acct-p", "claude-acct-b", "codex"])
        #expect(ClaudeRingMigrator.migratePreferences(accounts: [paras, biios], retired: ["claude-shared"],
                                                      preferences: preferences, store: nil, defaults: defaults))
        #expect(preferences.isMutedAlerts(for: "claude-acct-p"))
        #expect(!preferences.isMutedAlerts(for: "claude-acct-b"))
        #expect(preferences.mutedAlertProviders.isDisjoint(with: old))
        #expect(preferences.isInMenuBar("claude-acct-b"))
        #expect(!preferences.isInMenuBar("claude-acct-p"))
        #expect(preferences.menuBarLimits.chosen?.isDisjoint(with: old) == true)
    }
}
