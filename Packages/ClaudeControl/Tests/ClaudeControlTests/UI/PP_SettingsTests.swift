import Foundation
import Testing
@testable import ClaudeControl

/// Settings › Claude Code › Accounts: one row per account, where it runs,
/// its stores, and its hooks over every run folder.
@MainActor
struct PP_SettingsTests {
    let home = "/Users/me"

    private func summary(runDirs: [String], storeDirs: [String], windows: Int, installed: Int = 0) -> ClaudeAccountSummary {
        var hooks = ClaudeHookStatus(hooksInstalled: installed == runDirs.count && !runDirs.isEmpty, statusLineInstalled: true)
        hooks.folderCount = runDirs.count
        hooks.installedFolderCount = installed
        var summary = ClaudeAccountSummary(id: "uuid:p", ringID: "claude-acct-p", configDir: runDirs.first ?? storeDirs.first ?? "",
                                           label: "Claude Rivant", email: "paras@rivant.in", planName: "Max 20x",
                                           isDefault: runDirs.contains(home + "/.claude"), hooks: hooks, launchCommand: "claude")
        summary.runDirs = runDirs
        summary.storeDirs = storeDirs
        summary.windowCount = windows
        return summary
    }

    @Test func aRowSaysWhereTheAccountRunsAndWhereItIsStored() {
        let paras = summary(runDirs: [home + "/.claude", home + "/.claude-windows/801f9dd51396", home + "/.claude-windows/b9fbb9ecd7cb"],
                            storeDirs: [home + "/.claude-paras", home + "/.claude-paras-rivant-in"], windows: 2, installed: 3)
        // A workspace's folder outlives its window: they are workspaces (UX-5),
        // and the stores go on a line of their own (UX-11).
        #expect(SettingsPaneItems.folderSummary(paras, home: home)
                == "Runs in ~/.claude and 2 VS Code workspaces\nStores (Claude Parallel Profiles):\n  ~/.claude-paras\n  ~/.claude-paras-rivant-in")
        let biios = summary(runDirs: [home + "/.claude-windows/1bf3e8f92b11"], storeDirs: [home + "/.claude-claude"], windows: 1)
        #expect(SettingsPaneItems.folderSummary(biios, home: home)
                == "Runs in 1 VS Code workspace\nStore (Claude Parallel Profiles): ~/.claude-claude")
        let storeOnly = summary(runDirs: [], storeDirs: [home + "/.claude-claude"], windows: 0)
        #expect(SettingsPaneItems.folderSummary(storeOnly, home: home).hasPrefix("Runs nowhere now\nStore (Claude Parallel Profiles): ~/.claude-claude"))
        // A plain folder is just its folder.
        let plain = summary(runDirs: [home + "/.claude-work"], storeDirs: [], windows: 0)
        #expect(SettingsPaneItems.folderSummary(plain, home: home) == "~/.claude-work")

        let item = SettingsPaneItems.item(paras, diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                          reading: nil, home: home, now: Date())
        #expect(item.hookFolderCount == 3 && item.hookedFolderCount == 3)
        #expect(item.hookTitle == "Hooks in 3 of 3 folders")
        #expect(item.identity == "paras@rivant.in · Max 20x")

        var statuses: [String: AccountHookStatus] = [:]
        var done = AccountHookStatus()
        done.configDirExists = true
        done.hooksInstalled = true
        statuses[home + "/.claude"] = done
        let folders = SettingsPaneItems.folders(paras, statuses: statuses, hooksEnabled: true, home: home)
        #expect(folders.map(\.path) == ["~/.claude", "~/.claude-windows/801f9dd51396", "~/.claude-windows/b9fbb9ecd7cb",
                                        "~/.claude-paras", "~/.claude-paras-rivant-in"])
        #expect(folders.map(\.role) == [.defaultFolder, .window, .window, .store, .store])
        #expect(folders[0].state == "Hooks installed" && folders[1].state == "Checking…")
        #expect(folders[3].state == "Read only, never changed")
    }

    @Test func theAccountsHooksAreInPlaceOnlyInEveryRunFolder() {
        var a = AccountHookStatus()
        a.configDirExists = true
        a.hooksInstalled = true
        a.statusLineInstalled = true
        var b = a
        b.hooksInstalled = false
        b.lastError = "boom"
        let aggregate = SettingsPaneItems.aggregate([a, b])
        #expect(aggregate?.hooksInstalled == false)
        #expect(aggregate?.lastError == "boom")
        #expect(SettingsPaneItems.aggregate([a, nil]) == nil)
        #expect(SettingsPaneItems.aggregate([a])?.hooksInstalled == true)
        var unreadable = a
        unreadable.settingsReadable = false
        #expect(SettingsPaneItems.aggregate([a, unreadable])?.settingsReadable == false)

        var model = SettingsPaneModel()
        model.hookConsent = true
        model.hooksEnabled = true
        var paras = AccountSettingsItem(id: "p", ringID: "r", name: "P", defaultName: "P", hasNickname: false, identity: "",
                                        folder: "", colorIndex: 0, isDefault: true, isTracked: true, isRingShown: true,
                                        hookState: .notInstalled, statusLineInstalled: false, legacyHooks: [], hookProblem: nil,
                                        launchCommand: "", usageLine: "")
        paras.hookFolderCount = 3
        paras.hookedFolderCount = 2
        var biios = paras
        biios.hookFolderCount = 1
        biios.hookedFolderCount = 1
        biios.hookState = .installed
        model.accounts = [paras, biios]
        #expect(paras.hookTitle == "Hooks in 2 of 3 folders")
        #expect(model.hooksSummary == "Installed in 3 of 4 folders of 2 tracked accounts.")
    }

    /// Only its store holds it now (no window, and ~/.claude runs as the
    /// other account): nothing to hook, and nothing is wrong.
    @Test func anAccountThatRunsNowhereHasNoHookWarning() {
        let biios = summary(runDirs: [], storeDirs: [home + "/.claude-claude"], windows: 0)
        let item = SettingsPaneItems.item(biios, diskStatus: nil, nickname: nil, isRingShown: true, hooksEnabled: true,
                                          reading: nil, home: home, now: Date())
        #expect(item.hookState == .off)
        #expect(item.hookTitle == "No folder runs it now")
        #expect(item.hookProblem == nil)
        var withoutHooks = biios
        withoutHooks.hooks.folderCount = 0
        #expect(HookHealth.make(accounts: [withoutHooks], labels: [:], hooksEnabled: true).isHealthy)
        var model = SettingsPaneModel()
        model.hookConsent = true
        model.hooksEnabled = true
        model.accounts = [item]
        #expect(model.trackedCount == 0)
    }

    @Test func newAccountExplainsClaudeParallelProfiles() {
        #expect(NewAccountCopy.parallelProfiles.contains("VS Code window"))
        #expect(NewAccountCopy.parallelProfiles.contains("/login"))
        #expect(NewAccountCopy.terminalAlternative.contains("~/.claude-<name>"))
        #expect(ConsentCopy.takeoverStores.contains("~/.claude-shared"))
    }
}
