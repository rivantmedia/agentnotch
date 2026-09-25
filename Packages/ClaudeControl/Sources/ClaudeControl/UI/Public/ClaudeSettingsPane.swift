//
//  ClaudeSettingsPane.swift
//  ClaudeControl
//
//  The "Claude Code" pane of Codenotch's settings window, wired to the
//  engine. What Codenotch owns (nicknames, which rings are shown, its
//  Notifications pane, opening the panel) is reached through
//  `ClaudeSettingsHost`, which the app implements over its Preferences.
//

import AppKit
import Combine
import SwiftUI

/// What the pane needs from the app.
@MainActor
public protocol ClaudeSettingsHost: AnyObject {
    /// Codenotch's nickname for the ring, if any.
    func nickname(ringID: String) -> String?
    func setNickname(_ nickname: String?, ringID: String)
    /// Codenotch's connected state for the ring ("Ring in notch").
    func isRingShown(_ ringID: String) -> Bool
    func setRingShown(_ on: Bool, ringID: String)
    /// Show Codenotch's Notifications pane (sounds, peek).
    func openNotificationsSettings()
    /// Open the sessions panel.
    func openSessionsPanel()
}

public struct ClaudeSettingsPane: View {
    @ObservedObject private var hub: ClaudeControlHub
    private let host: ClaudeSettingsHost
    @ObservedObject private var hookManager = AccountHookManager.shared
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var notifications = NotificationService.shared
    @ObservedObject private var registry = AccountRegistry.shared

    /// Bumped after a change the pane can't observe (a setting, a nickname).
    @State private var revision = 0

    public init(hub: ClaudeControlHub, host: ClaudeSettingsHost) {
        self.hub = hub
        self.host = host
    }

    public var body: some View {
        SettingsPaneContent(model: model, actions: actions)
            .onAppear { notifications.refreshAuthorizationStatus() }
    }

    // MARK: Model

    private var model: SettingsPaneModel {
        _ = revision
        let home = AccountPaths.homeDirectory
        let now = Date()
        var model = SettingsPaneModel()
        model.setup = hub.setup
        model.hookConsent = ClaudeControlSettings.hookConsent
        model.consentFiles = hub.consentFileLines(home: home)
        model.consentScope = hub.consentScope.sentence
        model.takeoverCleanupFiles = hub.takeoverCleanupFiles(home: home)
        model.takeoverCleansStores = !model.takeoverCleanupFiles.isEmpty
        model.scopeNoticeFolders = hub.setup.newInstallFolders.map { folder in
            WindowFolderNames.label(folder, display: AccountPathDisplay.abbreviated(folder, home: home), names: hub.windowFolderNames)
        }
        model.parallelProfiles = hub.parallelProfilesDetected
        model.unsignedFolders = hub.unsignedFolders.map { folder in
            WindowFolderNames.label(folder, display: AccountPathDisplay.abbreviated(folder, home: home), names: hub.windowFolderNames)
        }
        model.hooksEnabled = ClaudeControlSettings.hooksEnabled
        model.statusLineIntegration = ClaudeControlSettings.statusLineIntegration
        model.installsAllowed = !DevFlags.installsDisabled
        model.isHookWorkRunning = hookManager.isWorking
        model.socketPath = AppIdentity.socketPath
        model.claudeCodeVersion = hookManager.detectedVersion?.description
        model.claudeBinaryPath = ClaudeControlSettings.claudeBinaryPath
        model.upstreamVibeNotchRunning = !SealedMode.isOn && hookManager.upstreamVibeNotchRunning
        model.hooksChangedNotice = HooksChangedNotice.text(
            changedAccountIds: hookManager.lastChangedAccounts, accounts: hub.accounts,
            backups: hookManager.status.compactMapValues(\.newestBackupPath), windowNames: hub.windowFolderNames,
            home: home)
        model.suggestions = hub.folderSuggestions.map { FolderSuggestionItem($0, home: home) }
        model.probeInterval = ClaudeControlSettings.usageProbeIntervalMinutes
        model.readsDesktopUsageCache = ClaudeControlSettings.readsDesktopUsageCache
        model.isRefreshingUsage = usageStore.isFetching
        model.autoOpen = ClaudeControlSettings.autoOpen
        model.holdOpen = ClaudeControlSettings.holdOpenWhileNeedsYou
        model.ringBadges = ClaudeControlSettings.ringBadges
        model.restingMarks = ClaudeControlSettings.restingMarks
        model.dockBadge = ClaudeControlSettings.dockBadge
        model.ringClick = ClaudeControlSettings.ringClick
        model.sessionClick = ClaudeControlSettings.sessionClick
        model.hotKey = ClaudeControlSettings.hotKey
        model.notifyNeedsInput = ClaudeControlSettings.notifyNeedsInput
        model.notifyReadyForReview = ClaudeControlSettings.notifyReadyForReview
        model.notificationsDenied = notifications.isAuthorizationDenied
        model.now = now
        model.accounts = hub.accounts.map { summary in
            let statuses = hookManager.status
            var item = SettingsPaneItems.item(
                summary,
                diskStatus: SettingsPaneItems.aggregate(summary.runDirs.map { statuses[$0] }),
                nickname: host.nickname(ringID: summary.ringID),
                isRingShown: host.isRingShown(summary.ringID),
                hooksEnabled: model.hooksEnabled,
                installsDisabled: !model.installsAllowed,
                reading: hub.ringReadings[summary.ringID],
                parallelProfiles: hub.parallelProfilesDetected,
                home: home,
                now: now
            )
            item.hasLoginConflict = summary.runDirs.contains { registry.account(id: $0).map(registry.hasLoginConflict) ?? false }
            item.vibeIslandHooksPresent = summary.configDirs.contains { statuses[$0]?.vibeIslandHooksPresent == true }
            item.folders = SettingsPaneItems.folders(summary, statuses: statuses, hooksEnabled: model.hooksEnabled,
                                                     windowNames: hub.windowFolderNames, home: home)
            return item
        }
        return model
    }

    // MARK: Actions

    private var actions: SettingsPaneActions {
        let hub = self.hub
        let host = self.host
        let changed: @MainActor () -> Void = { revision &+= 1 }
        let setting: @MainActor (() -> Void) -> Void = { apply in
            apply()
            changed()
        }
        var actions = SettingsPaneActions()
        // Every hook and account action goes through the hub, which writes
        // only on these clicks and does nothing at all in a sealed run.
        actions.turnOn = { setting { _ = hub.grantHookConsent() } }
        actions.notNow = { setting { hub.declineHookConsent() } }
        actions.quitVibeNotch = { hub.quitVibeNotch() }
        actions.acknowledgeScope = { setting { hub.acknowledgeInstallScope() } }
        actions.turnOffAfterScopeNotice = { setting { hub.turnOffAfterScopeNotice() } }
        actions.rename = { ringID, nickname in
            setting { host.setNickname(nickname, ringID: ringID) }
        }
        actions.setRingShown = { ringID, shown in
            setting { host.setRingShown(shown, ringID: ringID) }
        }
        // Off also takes our hooks out of the account.
        actions.setTracked = { accountId, tracked in hub.setTracked(accountId: accountId, tracked) }
        actions.installHooks = { hub.installHooks(accountId: $0) }
        actions.removeLegacyHooks = { accountId, kind in
            switch kind {
            case .vibeNotch:
                hub.removeLegacyHooks(accountId: accountId, kind: .vibeNotch)
            case .superpoweredVibeNotch:
                // A takeover of that account: its hooks go, the status line
                // its wrapper replaced comes back, then ours go in where
                // hooks are on. Refused while it runs; the hub's setup then
                // says it is running and the consent card offers to quit it.
                hub.takeOverFromVibeNotch(accountIds: [accountId])
            }
        }
        actions.copyLaunchCommand = { command in
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(command, forType: .string)
        }
        actions.reveal = { accountId in
            guard let account = hub.accounts.first(where: { $0.id == accountId }) else { return }
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: account.configDir, isDirectory: true)])
        }
        // Our hooks come out first (in the hook manager's queue), then the
        // account is forgotten; the folder stays.
        actions.forget = { hub.forgetAccount(accountId: $0) }
        actions.addExistingFolder = { AddAccountFolderPanel.run(hub: hub) }
        actions.acceptSuggestion = { configDir in
            do { try hub.acceptSuggestion(configDir) } catch { AddAccountFolderPanel.show(error) }
            changed()
        }
        actions.dismissSuggestion = { configDir in
            hub.dismissSuggestion(configDir)
            changed()
        }
        actions.createAccount = { name in try hub.createAccount(name: name).launchCommand }
        actions.chooseClaudeBinary = {
            guard let path = ClaudeBinaryPanel.run() else { return }
            setting { hub.setClaudeBinaryPath(path) }
        }
        actions.resetClaudeBinary = { setting { hub.setClaudeBinaryPath(nil) } }
        actions.setHooksEnabled = { on in setting { hub.setHooksEnabled(on) } }
        actions.setStatusLineIntegration = { on in setting { hub.setStatusLineIntegration(on) } }
        actions.setProbeInterval = { minutes in setting { UsageStore.shared.setProbeInterval(minutes: minutes) } }
        actions.setReadsDesktopUsageCache = { on in setting { ClaudeControlSettings.readsDesktopUsageCache = on } }
        actions.refreshUsage = {
            for account in hub.accounts where account.isTracked {
                Task { await hub.refreshUsage(ringID: account.ringID, reason: .forced) }
            }
        }
        actions.setAutoOpen = { value in setting { ClaudeControlSettings.autoOpen = value } }
        actions.setHoldOpen = { value in setting { ClaudeControlSettings.holdOpenWhileNeedsYou = value } }
        actions.setRingBadges = { value in setting { ClaudeControlSettings.ringBadges = value } }
        actions.setRestingMarks = { value in setting { ClaudeControlSettings.restingMarks = value } }
        actions.setDockBadge = { value in setting { ClaudeControlSettings.dockBadge = value } }
        actions.setRingClick = { value in setting { ClaudeControlSettings.ringClick = value } }
        actions.setSessionClick = { value in setting { ClaudeControlSettings.sessionClick = value } }
        actions.setHotKey = { value in setting { ClaudeControlSettings.hotKey = value } }
        actions.setNotifyNeedsInput = { value in setting { ClaudeControlSettings.notifyNeedsInput = value } }
        actions.setNotifyReadyForReview = { value in setting { ClaudeControlSettings.notifyReadyForReview = value } }
        actions.openSystemNotificationSettings = { NotificationService.shared.openSystemSettings() }
        actions.openNotificationsPane = { host.openNotificationsSettings() }
        actions.openSessionsPanel = { host.openSessionsPanel() }
        actions.copyStateDump = {
            let dump = ClaudeSessionMonitor.shared.instances.map(SessionStateDump.summary).joined(separator: "\n")
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(dump.isEmpty ? "No sessions" : dump, forType: .string)
        }
        actions.resetReviewQueue = { ClaudeSessionMonitor.shared.markAllReviewed() }
        return actions
    }
}

// MARK: - Items

nonisolated enum SettingsPaneItems {
    /// One account's row, from its summary, what its settings.json says and
    /// Codenotch's name and switch for its ring.
    static func item(
        _ summary: ClaudeAccountSummary,
        diskStatus: AccountHookStatus?,
        nickname: String?,
        isRingShown: Bool,
        hooksEnabled: Bool,
        installsDisabled: Bool = false,
        reading: ClaudeRingReading?,
        parallelProfiles: Bool = false,
        home: String,
        now: Date
    ) -> AccountSettingsItem {
        let folder = AccountPathDisplay.abbreviated(summary.configDir, home: home)
        let defaultName = summary.email ?? AccountLabels.folderName(summary.configDir, home: home)
        let hookFolderCount = summary.runDirs.isEmpty && summary.storeDirs.isEmpty ? 1 : summary.runDirs.count
        let nickname = nickname.flatMap { $0.isEmpty ? nil : $0 }
        var legacy: [AccountSettingsItem.LegacyHooks] = []
        if summary.hooks.vibeNotchHooksPresent || diskStatus?.vibeNotchHooksPresent == true { legacy.append(.vibeNotch) }
        if summary.hooks.superpoweredVibeNotchHooksPresent || diskStatus?.superpoweredVibeNotchHooksPresent == true {
            legacy.append(.superpoweredVibeNotch)
        }
        let hooks = hookSummary(summary, diskStatus: diskStatus, hooksEnabled: hooksEnabled, installsDisabled: installsDisabled)
        // Only its store holds it now: nothing to hook, nothing wrong.
        let runsNowhere = summary.runDirs.isEmpty && !summary.storeDirs.isEmpty
        let state = runsNowhere ? .off : hookState(hooks.kind, hooksEnabled: hooksEnabled, installsDisabled: installsDisabled)
        var item = AccountSettingsItem(
            id: summary.id,
            ringID: summary.ringID,
            name: nickname ?? summary.label,
            defaultName: defaultName,
            hasNickname: nickname != nil,
            identity: [summary.email ?? "Not signed in", summary.planName].compactMap { $0 }.joined(separator: " · "),
            folder: folder,
            colorIndex: summary.colorIndex,
            isDefault: summary.isDefault,
            isTracked: summary.isTracked,
            isRingShown: isRingShown,
            hookState: state,
            statusLineInstalled: summary.hooks.statusLineInstalled,
            legacyHooks: legacy,
            // Off by choice needs no explanation beside the switch.
            hookProblem: state == .off || state == .installed ? nil : hooks.detail,
            launchCommand: summary.launchCommand,
            usageLine: SettingsUsageLine.text(for: reading, isRingShown: isRingShown,
                                              isTracked: summary.isTracked, now: now),
            hookFolderCount: hookFolderCount,
            hookedFolderCount: hookFolderCount <= 1
                ? (state == .installed ? hookFolderCount : 0)
                : min(summary.hooks.installedFolderCount, hookFolderCount),
            folderSummary: folderSummary(summary, home: home)
        )
        item.hasTerminalLaunch = summary.hasTerminalLaunch
        item.launchGuidance = summary.hasTerminalLaunch ? nil : launchGuidance(summary)
        item.canForget = summary.canForget
        if parallelProfiles {
            item.defaultCaption = "In ~/.claude now"
            item.defaultCaptionHelp = "Terminals outside VS Code run as this account now: Claude Parallel Profiles copies the focused VS Code window's account into ~/.claude."
        }
        return item
    }

    /// What replaces "Copy launch command" for an account that runs only in
    /// VS Code windows (or that only a store holds): a window's folder is
    /// that window's, and a store is never run.
    static func launchGuidance(_ summary: ClaudeAccountSummary) -> String {
        if summary.windowCount > 0 {
            return "Runs in VS Code: pick it for a window from the Claude Parallel Profiles status bar item; that window's terminals run as it. For another terminal, focus one of its windows (the extension then copies it into ~/.claude) and run claude."
        }
        return "Open a VS Code window on it with the Claude Parallel Profiles status bar item (the extension then copies it into ~/.claude), then run claude."
    }

    /// Where the account lives: "Runs in ~/.claude and 2 VS Code
    /// workspaces", and on a line of its own "Stores (Claude Parallel
    /// Profiles): ~/.claude-paras, ~/.claude-paras-rivant-in"; just the
    /// folder when it has one. A workspace's folder outlives its window, so
    /// they are counted as workspaces.
    static func folderSummary(_ summary: ClaudeAccountSummary, home: String) -> String {
        let run = summary.runDirs.isEmpty && summary.storeDirs.isEmpty ? [summary.configDir] : summary.runDirs
        let stores = summary.storeDirs.map { AccountPathDisplay.abbreviated($0, home: home) }
        if stores.isEmpty, run.count == 1, summary.windowCount == 0 {
            let only = AccountPathDisplay.abbreviated(run[0], home: home)
            return summary.adoptedDirs.contains(run[0]) ? "\(only) (also a \(ParallelProfiles.displayName) account)" : only
        }
        let windows = summary.windowCount
        let others = run.filter { !isWindowFolder($0, home: home) }.map { dir -> String in
            let shown = AccountPathDisplay.abbreviated(dir, home: home)
            return summary.adoptedDirs.contains(dir) ? "\(shown) (also a \(ParallelProfiles.displayName) account)" : shown
        }
        var places = others
        if windows > 0 { places.append(windows == 1 ? "1 VS Code workspace" : "\(windows) VS Code workspaces") }
        var parts: [String] = []
        parts.append(places.isEmpty ? "Runs nowhere now" : "Runs in " + ListFormatter.localizedString(byJoining: places))
        // One store per line: a path never wraps in the middle.
        if stores.count == 1 {
            parts.append("Store (\(ParallelProfiles.displayName)): \(stores[0])")
        } else if !stores.isEmpty {
            parts.append("Stores (\(ParallelProfiles.displayName)):\n" + stores.map { "  " + $0 }.joined(separator: "\n"))
        }
        return parts.joined(separator: "\n")
    }

    private static func isWindowFolder(_ path: String, home: String) -> Bool {
        ParallelProfiles.isWindowDir(path, home: home)
    }

    /// Every folder of an account with what its hooks are, when it has more
    /// than one.
    static func folders(_ summary: ClaudeAccountSummary, statuses: [String: AccountHookStatus],
                        hooksEnabled: Bool, windowNames: [String: String] = [:],
                        home: String) -> [AccountSettingsItem.Folder] {
        guard summary.configDirs.count > 1 else { return [] }
        let defaultDir = (AccountPaths.normalize(home) as NSString).appendingPathComponent(".claude")
        var folders: [AccountSettingsItem.Folder] = summary.runDirs.map { dir in
            let role: AccountSettingsItem.Folder.Role = dir == defaultDir ? .defaultFolder
                : isWindowFolder(dir, home: home) ? .window : .standalone
            let state: String
            if !summary.isTracked {
                state = "Not tracked"
            } else if let status = statuses[dir] {
                if !status.settingsReadable {
                    state = "settings.json unreadable"
                } else if status.hooksInstalled {
                    state = status.statusLineInstalled ? "Hooks and live status line" : "Hooks installed"
                } else {
                    state = hooksEnabled ? "Hooks not installed" : "Hooks off"
                }
            } else {
                state = "Checking…"
            }
            return AccountSettingsItem.Folder(path: AccountPathDisplay.abbreviated(dir, home: home), role: role, state: state,
                                              projectName: role == .window ? windowNames[dir] : nil,
                                              isAdopted: summary.adoptedDirs.contains(dir))
        }
        folders += summary.storeDirs.map { dir in
            AccountSettingsItem.Folder(path: AccountPathDisplay.abbreviated(dir, home: home), role: .store,
                                       state: "Read only, never changed")
        }
        return folders
    }

    /// One status over an account's run folders: hooks in place only when
    /// in every one, unreadable when any is, another app's hooks when in any.
    /// Nil while any is still unread.
    static func aggregate(_ statuses: [AccountHookStatus?]) -> AccountHookStatus? {
        guard !statuses.isEmpty else { return nil }
        let known = statuses.compactMap { $0 }
        guard known.count == statuses.count else { return nil }
        guard known.count > 1, let first = known.first else { return known.first }
        var status = first
        status.configDirExists = known.contains(where: \.configDirExists)
        status.settingsReadable = known.allSatisfy(\.settingsReadable)
        status.hooksInstalled = known.allSatisfy(\.hooksInstalled)
        status.hooksRegistered = known.contains(where: \.hooksRegistered)
        status.statusLineInstalled = known.allSatisfy(\.statusLineInstalled)
        status.vibeNotchHooksPresent = known.contains(where: \.vibeNotchHooksPresent)
        status.superpoweredVibeNotchHooksPresent = known.contains(where: \.superpoweredVibeNotchHooksPresent)
        status.vibeIslandHooksPresent = known.contains(where: \.vibeIslandHooksPresent)
        status.lastError = known.compactMap(\.lastError).first
        status.newestBackupPath = known.compactMap(\.newestBackupPath).first
        return status
    }

    /// The account's hooks, from its settings.json when that was read, else
    /// from the summary the engine publishes.
    static func hookSummary(_ summary: ClaudeAccountSummary, diskStatus: AccountHookStatus?,
                            hooksEnabled: Bool, installsDisabled: Bool = false) -> AccountHookSummary {
        var status = diskStatus ?? AccountHookStatus()
        if diskStatus == nil {
            status.configDirExists = true
            status.hooksInstalled = summary.hooks.hooksInstalled
            status.statusLineInstalled = summary.hooks.statusLineInstalled
        }
        if status.lastError == nil { status.lastError = summary.hooks.lastError }
        return AccountHookSummary.make(status: status, hooksEnabled: hooksEnabled, installsDisabled: installsDisabled,
                                       isHidden: !summary.isTracked)
    }

    static func hookState(_ summary: ClaudeAccountSummary, diskStatus: AccountHookStatus?,
                          hooksEnabled: Bool) -> AccountSettingsItem.HookState {
        hookState(hookSummary(summary, diskStatus: diskStatus, hooksEnabled: hooksEnabled).kind,
                  hooksEnabled: hooksEnabled, installsDisabled: false)
    }

    private static func hookState(_ kind: AccountHookSummary.Kind, hooksEnabled: Bool,
                                  installsDisabled: Bool) -> AccountSettingsItem.HookState {
        switch kind {
        case .installed: return .installed
        case .unreadable: return .unreadable
        case .missingFolder: return .missingFolder
        case .hidden: return .off
        case .notInstalled, .unknown: return hooksEnabled && !installsDisabled ? .notInstalled : .off
        }
    }
}

// MARK: - Add existing folder

/// "Add existing folder…": pick a folder (hidden ones shown), refuse the
/// home folder and anything above it, ask before adding one that doesn't
/// look like Claude Code's, then add it and install hooks if they are on.
@MainActor
enum AddAccountFolderPanel {
    static func run(hub: ClaudeControlHub) {
        // A sealed run shows fixture accounts only: it never reads or
        // writes a real Claude Code folder.
        guard !SealedMode.isOn else {
            alert("This is a sealed run: it shows fixture accounts only and never adds a real folder.", confirm: nil)
            return
        }
        let home = AccountPaths.homeDirectory
        let panel = NSOpenPanel()
        panel.title = "Add a Claude Code account"
        panel.message = "Choose the folder a Claude Code account keeps its settings in, such as ~/.claude-work."
        panel.prompt = "Add account"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        // Home is where ~/.claude-* folders live; picking home itself is
        // refused below with an explanation.
        panel.directoryURL = URL(fileURLWithPath: home, isDirectory: true)
        guard panel.runModal() == .OK, let url = panel.url else { return }

        let path = url.standardizedFileURL.path
        let entries = Set((try? FileManager.default.contentsOfDirectory(atPath: path)) ?? [])
        // The pane's own rules first (home, links to it, already added),
        // then the engine's (inside another account, not a folder, …).
        switch AddFolderCheck.evaluate(path: path, home: home, entries: entries,
                                       knownConfigDirs: AccountRegistry.shared.accounts.map(\.configDir),
                                       resolvedPath: url.resolvingSymlinksInPath().path) {
        case .reject(let message):
            alert(message, confirm: nil)
        case .confirm(let message):
            let markers: AccountFolderMarkers
            do { markers = try hub.checkFolder(path) } catch { return show(error) }
            if markers.isClearlyConfigDir || alert(message, confirm: "Add anyway") { add(path, hub: hub) }
        case .add:
            add(path, hub: hub)
        }
    }

    /// Hooks follow on the hook manager's next pass (after consent).
    private static func add(_ path: String, hub: ClaudeControlHub) {
        do { try hub.addExistingFolder(path) } catch { show(error) }
    }

    static func show(_ error: Error) {
        alert(error.localizedDescription, confirm: nil)
    }

    /// True when the user confirmed.
    @discardableResult
    private static func alert(_ message: String, confirm: String?) -> Bool {
        let alert = NSAlert()
        alert.messageText = confirm == nil ? "Can't add this folder" : "Add this folder?"
        alert.informativeText = message
        if let confirm {
            alert.addButton(withTitle: confirm)
            alert.addButton(withTitle: "Cancel")
        } else {
            alert.addButton(withTitle: "OK")
        }
        return alert.runModal() == .alertFirstButtonReturn && confirm != nil
    }
}

// MARK: - Choose the claude binary

/// "Choose…" beside the Claude Code version: pick the `claude` executable
/// the hooks are written for (when the one found isn't the one you run).
@MainActor
enum ClaudeBinaryPanel {
    /// The chosen executable's path, or nil when cancelled or not runnable.
    static func run() -> String? {
        guard !SealedMode.isOn else { return nil }
        let panel = NSOpenPanel()
        panel.title = "Choose the claude binary"
        panel.message = "Choose the claude executable you run, for example ~/.local/bin/claude."
        panel.prompt = "Use this binary"
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        panel.treatsFilePackagesAsDirectories = true
        guard panel.runModal() == .OK, let url = panel.url else { return nil }
        let path = url.standardizedFileURL.path
        guard FileManager.default.isExecutableFile(atPath: path) else {
            let alert = NSAlert()
            alert.messageText = "Can't use this file"
            alert.informativeText = "\((path as NSString).lastPathComponent) isn't an executable."
            alert.addButton(withTitle: "OK")
            alert.runModal()
            return nil
        }
        return path
    }
}
