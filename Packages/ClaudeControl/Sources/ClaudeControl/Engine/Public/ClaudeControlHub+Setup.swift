//
//  ClaudeControlHub+Setup.swift
//  ClaudeControl
//
//  The hub's setup side: the consent card ("Turn on Claude Code control"),
//  taking over from Superpowered Vibe Notch, the Hooks switches, and the
//  account actions of the settings pane (track, forget, add a folder, new
//  account, suggestions). Everything here writes only after the user's
//  explicit click, and nothing at all when sealed.
//
//  The settings pane (same module) calls these; the bridge needs none of
//  them. `setup` is republished from `currentSetupState()`.
//

import Combine
import Foundation

extension ClaudeControlHub {
    // MARK: - Setup state

    /// What the consent card and banners show: consent unanswered,
    /// Superpowered Vibe Notch running, other apps' hooks in a tracked
    /// account. The socket error comes from the socket server.
    func currentSetupState(socketError: String? = nil) -> ClaudeSetupState {
        AccountHookManager.shared.setupState(isSealed: isSealed,
                                             socketError: socketError ?? ClaudeSessionMonitor.shared.socketError)
    }

    /// Republish `setup` now (after a consent answer, which no service
    /// publishes by itself).
    func publishSetup() {
        let state = currentSetupState()
        if setup != state { setup = state }
    }

    /// Keep `setup` current as the answer, the Superpowered Vibe Notch guard
    /// and the accounts' hooks change. The hub keeps the subscription while
    /// started (none when sealed).
    func followSetupChanges() -> AnyCancellable? {
        guard !isSealed else { return nil }
        let manager = AccountHookManager.shared
        return Publishers.CombineLatest4(manager.$hookConsent, manager.$vibeNotchRunning, manager.$status,
                                         ClaudeSessionMonitor.shared.$socketError)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _, _, _, _ in self?.publishSetup() }
    }

    // MARK: - Consent

    /// The settings.json files "Turn on" will edit: every tracked run
    /// folder's (`~/.claude`, VS Code windows, standalone folders), never a
    /// Claude Parallel Profiles store's.
    var settingsFilesToEdit: [String] {
        isSealed ? [] : AccountHookManager.shared.settingsFilesToEdit
    }

    /// The consent card's file list: each settings.json "Turn on" edits,
    /// grouped by account, a VS Code workspace's folder with its project
    /// and account after it: "~/.claude-windows/801f9dd51396/settings.json
    /// (superpowered-vibe-notch · me@x.dev)".
    func consentFileLines(home: String) -> [String] {
        Self.consentFileLines(files: settingsFilesToEdit, accounts: accounts, windowNames: windowFolderNames, home: home)
    }

    /// Pure: see `consentFileLines(home:)`.
    nonisolated static func consentFileLines(files: [String], accounts: [ClaudeAccountSummary],
                                             windowNames: [String: String], home: String) -> [String] {
        func owner(_ folder: String) -> Int? { accounts.firstIndex { $0.configDirs.contains(folder) || $0.configDir == folder } }
        let folders = files.map { ($0 as NSString).deletingLastPathComponent }
        let ordered = zip(files, folders).sorted { lhs, rhs in
            (owner(lhs.1) ?? Int.max, lhs.0) < (owner(rhs.1) ?? Int.max, rhs.0)
        }
        return ordered.map { file, folder in
            let path = AccountPathDisplayName.abbreviated(file, home: home)
            guard ParallelProfiles.isWindowDir(folder, home: home) else { return path }
            let who = owner(folder).flatMap { accounts[$0].email ?? accounts[$0].label }
            let notes = [windowNames[folder], who].compactMap { $0 }
            return notes.isEmpty ? path : "\(path) (\(notes.joined(separator: " · ")))"
        }
    }

    /// The settings.json files the takeover cleans where nothing of ours
    /// goes (stores, the shared history), for the consent card.
    func takeoverCleanupFiles(home: String) -> [String] {
        let layout = AccountRegistry.shared.layout
        return vibeNotchCleanupDirs
            .filter { layout.kind(of: $0) == .store || layout.kind(of: $0) == .infrastructure }
            .map { AccountPathDisplayName.abbreviated(($0 as NSString).appendingPathComponent("settings.json"), home: home) }
    }

    /// Accounts with a folder (stores included) that still has Superpowered
    /// Vibe Notch's hooks.
    var accountsToTakeOver: [ClaudeAccountSummary] {
        guard !isSealed else { return [] }
        var seen = Set<String>()
        return AccountHookManager.shared.accountsToTakeOver.compactMap { folder in
            let summary = account(forFolderId: folder.id) ?? summary(of: folder)
            return seen.insert(summary.id).inserted ? summary : nil
        }
    }

    /// Every folder "Take over" cleans of Superpowered Vibe Notch's
    /// leftovers: run folders, stores and the shared history.
    var vibeNotchCleanupDirs: [String] {
        isSealed ? [] : AccountHookManager.shared.vibeNotchCleanupDirs
    }

    /// What "Turn on" installs into, in words (see `ConsentScope`).
    var consentScope: ConsentScope {
        guard !isSealed else { return ConsentScope() }
        let registry = AccountRegistry.shared
        let targets = AccountHookManager.shared.installTargets
        let home = registry.homePath
        return ConsentScope(
            includesDefault: targets.contains { AccountRegistry.isDefault($0, home: home) },
            windowCount: targets.filter { ParallelProfiles.isWindowDir($0.configDir, home: home) }.count,
            standaloneFolders: targets.filter {
                !AccountRegistry.isDefault($0, home: home) && !ParallelProfiles.isWindowDir($0.configDir, home: home)
            }.map { AccountPathDisplayName.abbreviated($0.configDir, home: home) },
            storeCount: registry.accounts.filter { $0.kind == .store }.count,
            parallelProfiles: registry.layout.extensionDetected
        )
    }

    /// [Turn on]: install into every tracked account, first taking over from
    /// Superpowered Vibe Notch where its hooks are (unless told not to).
    /// False while Superpowered Vibe Notch is running: nothing happens then.
    @discardableResult
    func grantHookConsent(takeOverFromVibeNotch: Bool = true) -> Bool {
        guard !isSealed else { return false }
        let granted = AccountHookManager.shared.grantConsent(takeOverFromVibeNotch: takeOverFromVibeNotch)
        publishSetup()
        return granted
    }

    /// [OK] on the notice that "Turn on" now also covers the VS Code
    /// workspaces' folders (a yes given to an earlier build).
    func acknowledgeInstallScope() {
        guard !isSealed else { return }
        AccountHookManager.shared.acknowledgeInstallScope()
        publishSetup()
    }

    /// [Turn off] on that notice: the Hooks switch off (every folder's
    /// hooks come out, status lines restored), the notice gone.
    func turnOffAfterScopeNotice() {
        guard !isSealed else { return }
        AccountHookManager.shared.acknowledgeInstallScope()
        setHooksEnabled(false)
    }

    /// [Not now]: remember it. Nothing is written.
    func declineHookConsent() {
        guard !isSealed else { return }
        AccountHookManager.shared.declineConsent()
        publishSetup()
    }

    /// "Take over from Superpowered Vibe Notch" (all tracked accounts, or
    /// the ones given). False while it is running.
    @discardableResult
    func takeOverFromVibeNotch(accountIds: [String]? = nil) -> Bool {
        guard !isSealed else { return false }
        return AccountHookManager.shared.takeOver(accountIds: accountIds)
    }

    /// [Quit it] on the "Superpowered Vibe Notch is running" banner.
    func quitVibeNotch() {
        guard !isSealed else { return }
        AccountHookManager.shared.quitVibeNotch()
    }

    // MARK: - Hooks

    /// The Hooks switch. On is also a yes to the consent card.
    func setHooksEnabled(_ enabled: Bool) {
        guard !isSealed else { return }
        if enabled {
            AccountHookManager.shared.enableHooks()
        } else {
            AccountHookManager.shared.disableHooks()
            // Running sessions finish from their transcripts from now on (GUX-9).
            ClaudeSessionMonitor.shared.hooksTurnedOff()
        }
        publishSetup()
    }

    /// "Live status line data": wrap each account's status line, or restore it exactly.
    func setStatusLineIntegration(_ enabled: Bool) {
        guard !isSealed else { return }
        AccountHookManager.shared.setStatusLineIntegration(enabled)
    }

    /// Install/Reinstall hooks (a pass over every account; writes only where needed).
    func installHooks(accountId: String) {
        guard !isSealed else { return }
        AccountHookManager.shared.install(accountId: accountId)
    }

    /// Remove old hooks of one kind from one account. False (nothing done)
    /// for Superpowered Vibe Notch's while it runs.
    @discardableResult
    func removeLegacyHooks(accountId: String, kind: LegacyHookKind) -> Bool {
        guard !isSealed else { return false }
        return AccountHookManager.shared.removeLegacyHooks(accountId: accountId, kind: kind)
    }

    /// The `claude` binary chosen in Settings (nil: find it). Rewrites the
    /// hooks for its version.
    func setClaudeBinaryPath(_ path: String?) {
        guard !isSealed else { return }
        ClaudeControlSettings.claudeBinaryPath = path
        AccountHookManager.shared.installAll(relocateClaude: true)
    }

    // MARK: - Accounts

    /// "Track sessions and hooks". Off also takes our hooks out of the account.
    func setTracked(accountId: String, _ tracked: Bool) {
        guard !isSealed else { return }
        AccountHookManager.shared.setTracked(accountId: accountId, tracked)
    }

    /// Forget an account (not the default one): our hooks come out first,
    /// the folder stays.
    func forgetAccount(accountId: String) {
        guard !isSealed else { return }
        let registry = AccountRegistry.shared
        if let identity = registry.identity(id: accountId) {
            // Not the account `~/.claude` runs as, unless Claude Parallel
            // Profiles keeps it (a store or a VS Code window): then which one
            // ~/.claude holds only says which window was focused last.
            guard identity.canBeForgotten else { return }
        } else {
            guard registry.account(id: accountId)?.isDefault == false else { return }
        }
        let folders = registry.folders(for: accountId).map(\.id)
        AccountHookManager.shared.forget(accountId: accountId)
        // Its sessions go with it, rather than moving to the default ring and
        // freezing there once the hooks are gone (BHV-3).
        for folder in folders {
            ClaudeSessionMonitor.shared.dropSessions(ofAccount: folder)
        }
    }

    /// Check a folder the user picked before adding it: throws
    /// `AccountFolderError` for home, a folder holding it, or one inside
    /// another account; otherwise says what it holds (ask "doesn't look like
    /// a Claude Code folder, add it anyway?" unless `isClearlyConfigDir`).
    func checkFolder(_ path: String) throws -> AccountFolderMarkers {
        guard !isSealed else { throw AccountFolderError.unavailableWhenSealed }
        return try AccountRegistry.shared.checkFolder(path)
    }

    /// "Add existing folder…". The hooks follow on the next pass (after consent).
    @discardableResult
    func addExistingFolder(_ path: String) throws -> ClaudeAccountSummary {
        guard !isSealed else { throw AccountFolderError.unavailableWhenSealed }
        let account = try AccountRegistry.shared.addAccount(configDir: path)
        return summary(of: account)
    }

    /// "New account…": creates `~/.claude-<name>`. Copy its `launchCommand`,
    /// run it, then `/login`.
    @discardableResult
    func createAccount(name: String) throws -> ClaudeAccountSummary {
        guard !isSealed else { throw AccountFolderError.unavailableWhenSealed }
        let account = try AccountRegistry.shared.createAccount(name: name)
        return summary(of: account)
    }

    /// Folders that look like accounts but weren't added by themselves.
    var folderSuggestions: [AccountFolderSuggestion] {
        isSealed ? [] : AccountRegistry.shared.suggestions
    }

    @discardableResult
    func acceptSuggestion(_ configDir: String) throws -> ClaudeAccountSummary {
        guard !isSealed else { throw AccountFolderError.unavailableWhenSealed }
        return summary(of: try AccountRegistry.shared.acceptSuggestion(configDir))
    }

    func dismissSuggestion(_ configDir: String) {
        guard !isSealed else { return }
        AccountRegistry.shared.dismissSuggestion(configDir)
    }

    /// The account a folder belongs to (its identity's summary), else the
    /// folder on its own.
    private func summary(of account: ClaudeAccount) -> ClaudeAccountSummary {
        let registry = AccountRegistry.shared
        if let identity = registry.identity(forFolderId: account.id) {
            return ClaudeHostProjections.account(identity: identity, hookStatuses: AccountHookManager.shared.status,
                                                 defaultRing: Self.defaultRingOwner(registry),
                                                 adopted: registry.layout.adoptedByExtension, home: registry.homePath)
        }
        return ClaudeHostProjections.account(account, hookStatus: AccountHookManager.shared.status[account.id],
                                             home: AppIdentity.homeDirectory)
    }
}

/// Where "Turn on" puts the hooks, for the consent card: `~/.claude`, the VS
/// Code workspaces' folders (new ones are set up as they appear) and
/// standalone folders; Claude Parallel Profiles' stores never get hooks.
nonisolated struct ConsentScope: Equatable, Sendable {
    var includesDefault = false
    var windowCount = 0
    var standaloneFolders: [String] = []
    var storeCount = 0
    var parallelProfiles = false

    /// Folders it installs into now.
    var folderCount: Int { (includesDefault ? 1 : 0) + windowCount + standaloneFolders.count }

    /// "Installs into ~/.claude and your VS Code workspaces' folders (3 now;
    /// new ones are set up automatically). Claude Parallel Profiles' account
    /// stores never get hooks." Nil without Claude Parallel Profiles (the
    /// file list says it). A workspace's folder stays after its window
    /// closes, so the count is of workspaces, not open windows.
    var sentence: String? {
        guard parallelProfiles else { return nil }
        var places: [String] = []
        if includesDefault { places.append("~/.claude") }
        places.append("your VS Code workspaces' folders (\(windowCount) now; new ones are set up automatically)")
        places += standaloneFolders
        let list = ListFormatter.localizedString(byJoining: places)
        return "Installs into \(list). Claude Parallel Profiles' account stores never get hooks."
    }
}
