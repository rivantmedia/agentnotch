//
//  Integration_SettingsTests.swift
//  ClaudeControlTests
//
//  The settings pane's pieces added when B's pane was wired to A2's hub
//  actions: folder suggestions, the "last change" note and the per-kind
//  legacy hook chips.
//

import Foundation
import Testing
@testable import ClaudeControl

struct Integration_SettingsTests {
    let home = "/Users/me"

    @Test func suggestionsSayWhyTheyWerentAdded() {
        let backup = FolderSuggestionItem(AccountFolderSuggestion(configDir: "/Users/me/.claude-old", reason: .looksLikeBackup),
                                          home: home)
        #expect(backup.folder == "~/.claude-old")
        #expect(backup.reason.contains("backup"))
        let again = FolderSuggestionItem(AccountFolderSuggestion(configDir: "/Users/me/.claude-x", reason: .seenAgain), home: home)
        #expect(again.reason.contains("Forgotten"))
    }

    @Test func theLastChangeNamesTheFilesAndTheBackup() {
        let work = ClaudeAccountSummary(id: "/Users/me/.claude-work", ringID: "claude-work", configDir: "/Users/me/.claude-work",
                                        label: "Work", isDefault: false, isTracked: true, colorIndex: 1,
                                        hooks: ClaudeHookStatus(), launchCommand: "")
        let personal = ClaudeAccountSummary(id: "/Users/me/.claude", ringID: "claude", configDir: "/Users/me/.claude",
                                            label: "Me", isDefault: true, isTracked: true, colorIndex: 0,
                                            hooks: ClaudeHookStatus(), launchCommand: "")
        #expect(HooksChangedNotice.text(changedAccountIds: [], accounts: [work], backups: [:], home: home) == nil)
        let one = HooksChangedNotice.text(changedAccountIds: [work.id], accounts: [work, personal],
                                          backups: [work.id: "/Users/me/.claude-work/settings.json.bak-1"], home: home)
        #expect(one?.contains("~/.claude-work") == true)
        #expect(one?.contains("~/.claude-work/settings.json.bak-1") == true)
        let both = HooksChangedNotice.text(changedAccountIds: [work.id, personal.id], accounts: [work, personal],
                                           backups: [:], home: home)
        #expect(both?.contains("beside each file") == true)
    }

    @Test func legacyHookChipsFollowEachKind() {
        let summary = ClaudeAccountSummary(id: "/Users/me/.claude", ringID: "claude", configDir: "/Users/me/.claude",
                                           label: "Me", isDefault: true, isTracked: true, colorIndex: 0,
                                           hooks: ClaudeHookStatus(), launchCommand: "")
        var disk = AccountHookStatus()
        disk.configDirExists = true
        disk.superpoweredVibeNotchHooksPresent = true
        let item = SettingsPaneItems.item(summary, diskStatus: disk, nickname: nil, isRingShown: true,
                                          hooksEnabled: true, reading: nil, home: home, now: Date())
        // Only Superpowered Vibe Notch's hooks: no Vibe Notch chip.
        #expect(item.legacyHooks == [.superpoweredVibeNotch])
    }
}
