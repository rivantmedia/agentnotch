import Foundation
import Testing
@testable import ClaudeControl

/// What Settings and the panel say about Claude Parallel Profiles accounts
/// after the review: the consent card (UX-2, S4), launch guidance (UX-3,
/// PP-C6), names of VS Code workspaces' folders (UX-6), "VS Code or the
/// terminal" (UX-7), the new-account caveat (UX-8), the scope notice (S2).
@MainActor
struct PPFix_SettingsCopyTests {
    let home = "/Users/me"

    private func summary(runDirs: [String], storeDirs: [String], windows: Int, isDefault: Bool) -> ClaudeAccountSummary {
        var summary = ClaudeAccountSummary(id: "uuid:b", ringID: "claude-acct-b", configDir: runDirs.first ?? storeDirs.first ?? "",
                                           label: "Claude Biios", email: "claude@biios.in", isDefault: isDefault,
                                           launchCommand: "claude")
        summary.runDirs = runDirs
        summary.storeDirs = storeDirs
        summary.windowCount = windows
        return summary
    }

    @Test func theConsentCardNoLongerContradictsItself() {
        #expect(ConsentCopy.explanation.contains("each folder Claude Code runs in"))
        #expect(ConsentCopy.takeoverStores.contains("settings.json files it created there are removed"))
        let scope = ConsentScope(includesDefault: true, windowCount: 3, storeCount: 3, parallelProfiles: true).sentence ?? ""
        #expect(scope.contains("never get hooks") && !scope.contains("untouched"))
        #expect(ConsentCopy.scopeMessage(folderCount: 3).contains("3 VS Code workspaces' folders"))
    }

    @Test func consentFilesAreGroupedByAccountWithTheProjectNamed() {
        var paras = summary(runDirs: [home + "/.claude", home + "/.claude-windows/801f9dd51396"], storeDirs: [], windows: 1, isDefault: true)
        paras.id = "uuid:p"
        paras.email = "paras@rivant.in"
        let biios = summary(runDirs: [home + "/.claude-windows/1bf3e8f92b11"], storeDirs: [], windows: 1, isDefault: false)
        let files = [home + "/.claude-windows/1bf3e8f92b11/settings.json", home + "/.claude/settings.json",
                     home + "/.claude-windows/801f9dd51396/settings.json"]
        let lines = ClaudeControlHub.consentFileLines(files: files, accounts: [paras, biios],
                                                      windowNames: [home + "/.claude-windows/801f9dd51396": "superpowered-vibe-notch"],
                                                      home: home)
        #expect(lines == [
            "~/.claude-windows/801f9dd51396/settings.json (superpowered-vibe-notch · paras@rivant.in)",
            "~/.claude/settings.json",
            "~/.claude-windows/1bf3e8f92b11/settings.json (claude@biios.in)",
        ])
    }

    @Test func aVSCodeOnlyAccountGetsGuidanceNotACommand() {
        var biios = summary(runDirs: [home + "/.claude-windows/1bf3e8f92b11"], storeDirs: [home + "/.claude-claude"], windows: 1, isDefault: false)
        biios.hasTerminalLaunch = false
        let item = SettingsPaneItems.item(biios, diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                          reading: nil, parallelProfiles: true, home: home, now: Date())
        #expect(!item.hasTerminalLaunch)
        #expect(item.launchGuidance?.contains("Claude Parallel Profiles status bar item") == true)

        var storeOnly = summary(runDirs: [], storeDirs: [home + "/.claude-claude"], windows: 0, isDefault: false)
        storeOnly.hasTerminalLaunch = false
        #expect(SettingsPaneItems.launchGuidance(storeOnly).contains("Open a VS Code window"))

        // ~/.claude's holder: named for what it is, not "Default".
        var paras = summary(runDirs: [home + "/.claude"], storeDirs: [home + "/.claude-paras"], windows: 0, isDefault: true)
        paras.canForget = true
        let holder = SettingsPaneItems.item(paras, diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                            reading: nil, parallelProfiles: true, home: home, now: Date())
        #expect(holder.defaultCaption == "In ~/.claude now" && holder.canForget && holder.hasTerminalLaunch)
        let plain = SettingsPaneItems.item(paras, diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                           reading: nil, home: home, now: Date())
        #expect(plain.defaultCaption == "Default")
    }

    @Test func workspaceFoldersAreNamedWhereTheyAreListed() {
        let paras = summary(runDirs: [home + "/.claude", home + "/.claude-windows/801f9dd51396", home + "/.claude-windows/b9fbb9ecd7cb"],
                            storeDirs: [home + "/.claude-paras"], windows: 2, isDefault: true)
        let folders = SettingsPaneItems.folders(paras, statuses: [:], hooksEnabled: true,
                                                windowNames: [home + "/.claude-windows/801f9dd51396": "superpowered-vibe-notch"],
                                                home: home)
        #expect(folders[1].title == "VS Code · superpowered-vibe-notch")
        #expect(folders[1].roleName == "~/.claude-windows/801f9dd51396")
        #expect(folders[2].title == "~/.claude-windows/b9fbb9ecd7cb" && folders[2].roleName == "VS Code workspace")

        let notice = HooksChangedNotice.text(changedAccountIds: [home + "/.claude-windows/801f9dd51396"], accounts: [paras],
                                             backups: [:], windowNames: [home + "/.claude-windows/801f9dd51396": "superpowered-vibe-notch"],
                                             home: home)
        #expect(notice?.contains("(VS Code · superpowered-vibe-notch)") == true)
        #expect(UnsignedFoldersRow.caption(["~/.claude-windows/0a1b2c3d4e5f"]).contains("status bar item"))
        #expect(!UnsignedFoldersRow.caption(["~/.claude-new"]).contains("status bar item"))
    }

    @Test func promptsWaitWhereClaudeCodeRuns() {
        var status = AccountHookStatus()
        status.configDirExists = true
        let summary = AccountHookSummary.make(status: status, hooksEnabled: true, installsDisabled: false, isHidden: false)
        #expect(summary.detail?.contains("VS Code or the terminal") == true)
        #expect(HookHealth(accountsWithoutHooks: ["Work"]).consequence.contains("VS Code or the terminal"))
        #expect(HookHealth.controlOffMessage.contains("VS Code or the terminal"))
        #expect(NewAccountCopy.parallelProfiles.contains("reloads it"))
    }
}
