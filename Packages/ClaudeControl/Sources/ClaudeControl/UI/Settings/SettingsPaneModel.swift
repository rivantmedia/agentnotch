//
//  SettingsPaneModel.swift
//  ClaudeControl
//
//  The "Claude Code" settings pane as plain values in and closures out. The
//  live pane (ClaudeSettingsPane) fills these from the engine and the app's
//  settings host; the snapshots fill them from fixtures.
//

import Foundation

nonisolated struct SettingsPaneModel {
    // Setup
    var setup = ClaudeSetupState()
    /// The answer to "Turn on Claude Code control": nil while unanswered.
    var hookConsent: Bool?
    /// The settings.json files turning on would edit.
    var consentFiles: [String] = []
    /// "Installs into ~/.claude and your VS Code workspaces' folders (3 now;
    /// new ones are set up automatically). Claude Parallel Profiles' account
    /// stores never get hooks." With Claude Parallel Profiles only.
    var consentScope: String?
    /// Taking over also cleans Superpowered Vibe Notch's leftovers out of
    /// Claude Parallel Profiles stores or the shared history.
    var takeoverCleansStores = false
    /// The settings.json files that cleans (in stores, ~/.claude-shared).
    var takeoverCleanupFiles: [String] = []
    /// The folders the "now covers your VS Code workspaces" notice names.
    var scopeNoticeFolders: [String] = []

    // Accounts
    var accounts: [AccountSettingsItem] = []
    /// Folders that look like accounts but weren't added by themselves.
    var suggestions: [FolderSuggestionItem] = []
    /// Run folders nobody is signed in to (no ring): `~/.claude-new`.
    var unsignedFolders: [String] = []
    /// Claude Parallel Profiles manages accounts here: "New account…"
    /// explains that accounts are added by signing in inside VS Code.
    var parallelProfiles = false

    // Hooks and status line
    var hooksEnabled = false
    var statusLineIntegration = true
    /// Off in a `--no-install` run: nothing is written.
    var installsAllowed = true
    /// An install or uninstall pass is running.
    var isHookWorkRunning = false
    var socketPath = ""
    var claudeCodeVersion: String?
    /// The `claude` binary chosen in Settings; nil means found automatically.
    var claudeBinaryPath: String?
    /// "Updated settings.json in ~/.claude-work (backup beside it)": the
    /// hook manager's last pass changed these files.
    var hooksChangedNotice: String?
    /// Upstream Vibe Notch is running: its own hooks report every session too.
    var upstreamVibeNotchRunning = false

    // Usage
    /// Minutes between usage checks; 0 is off.
    var probeInterval = ClaudeControlSettings.defaultUsageProbeInterval
    var readsDesktopUsageCache = true
    var isRefreshingUsage = false

    // Cloud
    /// The website: where sync goes, who is signed in, the sync and summary
    /// switches and the last sync (the hub's `cloud`). Signed out, with no
    /// website, until the user sets one.
    var cloud = ClaudeCloudState()

    // Sessions and attention
    var autoOpen: AutoOpenPolicy = .needsInput
    var holdOpen: HoldOpenPolicy = .auto
    var ringBadges = true
    var restingMarks = true
    var dockBadge = true
    var ringClick: RingClickAction = .openPanel
    var sessionClick: SessionClickAction = .smart
    var hotKey: PanelHotKey = .off

    // Notifications
    var notifyNeedsInput = true
    var notifyReadyForReview = true
    /// macOS notifications are turned off for the app in System Settings.
    var notificationsDenied = false

    var now = Date()

    /// Tracked accounts whose hooks are in place.
    var hooksInstalledCount: Int {
        accounts.filter { $0.isTracked && $0.hookState == .installed }.count
    }

    /// Tracked accounts with a folder to hook (one only a Claude Parallel
    /// Profiles store holds right now has none).
    var trackedCount: Int { accounts.filter { $0.isTracked && $0.hookFolderCount > 0 }.count }

    /// Folders of tracked accounts that get hooks, and how many have them.
    var trackedFolderCount: Int { accounts.filter(\.isTracked).map(\.hookFolderCount).reduce(0, +) }
    var hookedFolderCount: Int { accounts.filter(\.isTracked).map(\.hookedFolderCount).reduce(0, +) }

    /// The Hooks switch's detail: what is true, not what was asked for.
    var hooksSummary: String {
        guard hookConsent == true else { return "Turn on Claude Code control first." }
        guard installsAllowed else { return "Installing is off for this run (--no-install)." }
        guard hooksEnabled else { return "Off: no account has this app's hooks." }
        let total = trackedCount
        let folders = trackedFolderCount
        // An account in several folders (VS Code windows): count folders.
        if folders > total {
            switch (hookedFolderCount, folders) {
            case let (installed, folders) where installed == folders:
                return "Installed in all \(folders) folders of \(total == 1 ? "the tracked account" : "\(total) tracked accounts")."
            case let (installed, folders):
                return "Installed in \(installed) of \(folders) folders of \(total == 1 ? "the tracked account" : "\(total) tracked accounts")."
            }
        }
        switch (hooksInstalledCount, total) {
        case (_, 0): return "No tracked accounts."
        case let (installed, total) where installed == total:
            return total == 1 ? "Installed in the tracked account." : "Installed in all \(total) tracked accounts."
        case let (installed, total):
            return "Installed in \(installed) of \(total) tracked accounts."
        }
    }
}

/// One account as the pane lists it.
nonisolated struct AccountSettingsItem: Identifiable, Equatable, Sendable {
    /// Why "Ring in notch" is off and disabled for an untracked account.
    static let ringNeedsTrackingCaption = "Needs Track sessions and hooks"

    enum HookState: Equatable, Sendable {
        case installed
        case notInstalled
        /// Hooks are switched off, or the account isn't tracked.
        case off
        case unreadable
        case missingFolder
    }

    enum LegacyHooks: String, CaseIterable, Equatable, Identifiable, Sendable {
        /// Upstream Vibe Notch's claude-island-state.py.
        case vibeNotch
        /// Superpowered Vibe Notch's superpowered-notch-*.py.
        case superpoweredVibeNotch

        var id: String { rawValue }

        var appName: String {
            switch self {
            case .vibeNotch: return "Vibe Notch"
            case .superpoweredVibeNotch: return "Superpowered Vibe Notch"
            }
        }

        /// Whether "Remove … hooks" can act: after consent, in a run that may
        /// write. Vibe Notch's entries are simply deleted; Superpowered Vibe
        /// Notch's are taken over (its hooks removed, the status line its
        /// wrapper replaced put back), refused while it is running.
        func canRemove(canWrite: Bool, canInstall: Bool) -> Bool {
            canWrite
        }

        /// The button's tooltip: what it does to settings.json.
        func removalHelp(canInstall: Bool) -> String {
            switch self {
            case .vibeNotch:
                return "Deletes only Vibe Notch's entries from settings.json, with a backup. Every other hook stays."
            case .superpoweredVibeNotch:
                return canInstall
                    ? "Replaces Superpowered Vibe Notch's hooks with this app's and puts back the status line it wrapped, with a backup."
                    : "Removes Superpowered Vibe Notch's hooks and puts back the status line it wrapped, with a backup. Quit it first."
            }
        }
    }

    /// One folder of an account, for the expanded list.
    struct Folder: Equatable, Identifiable, Sendable {
        enum Role: Equatable, Sendable {
            /// `~/.claude`.
            case defaultFolder
            /// A VS Code window's working copy (Claude Parallel Profiles).
            case window
            /// A folder of its own (`CLAUDE_CONFIG_DIR=~/.claude-work`).
            case standalone
            /// A Claude Parallel Profiles account store: never written.
            case store
        }

        /// `~/.claude-windows/801f9dd51396`.
        var path: String
        var role: Role
        /// "Hooks installed", "Hooks not installed", "Left untouched", …
        var state: String
        /// A VS Code workspace's folder named after its project
        /// ("superpowered-vibe-notch"), when a session there told.
        var projectName: String? = nil
        /// The user's own profile Claude Parallel Profiles also copies from.
        var isAdopted = false

        var id: String { path }

        /// What the row leads with: "VS Code · superpowered-vibe-notch", else the path.
        var title: String { projectName.map { "VS Code · \($0)" } ?? path }

        var roleName: String {
            switch role {
            case .defaultFolder: return "Default"
            case .window: return projectName == nil ? "VS Code workspace" : path
            case .standalone: return isAdopted ? "Folder · also a Claude Parallel Profiles account" : "Folder"
            case .store: return "Account store"
            }
        }
    }

    let id: String
    let ringID: String
    /// The name shown everywhere: Codenotch's nickname, else the label.
    var name: String
    /// What the name falls back to without a nickname.
    var defaultName: String
    var hasNickname: Bool
    /// "me@work.com · Max 20x", or "Not signed in".
    var identity: String
    /// `~/.claude-work`.
    var folder: String
    var colorIndex: Int
    var isDefault: Bool
    var isTracked: Bool
    var isRingShown: Bool
    var hookState: HookState
    var statusLineInstalled: Bool
    var legacyHooks: [LegacyHooks]
    /// Why hooks aren't in place when they should be, or what went wrong.
    var hookProblem: String?
    var launchCommand: String
    /// `launchCommand` starts it from a terminal; else `launchGuidance` says
    /// how to use it (VS Code windows only, or only a store).
    var hasTerminalLaunch = true
    var launchGuidance: String?
    /// "Forget…" is offered (see `ClaudeAccountSummary.canForget`).
    var canForget = true
    /// Beside the name of the account `~/.claude` runs as: "Default", or
    /// with Claude Parallel Profiles "In ~/.claude now".
    var defaultCaption = "Default"
    var defaultCaptionHelp: String?
    /// "5-hour 34% · weekly 12% · 4m ago", or why there is no reading.
    var usageLine: String
    /// Sessions of this folder ran with more than one CLAUDE_CONFIG_DIR
    /// spelling, which Claude Code treats as separate logins.
    var hasLoginConflict = false
    /// Vibe Island's own hooks are in its settings.json (reported only: it
    /// is an app of its own, not something this app replaces).
    var vibeIslandHooksPresent = false
    /// The folders its hooks go into (run folders), and how many have them.
    var hookFolderCount = 1
    var hookedFolderCount = 0
    /// "Runs in ~/.claude and 2 VS Code workspaces", then (a line of its
    /// own) "Stores (Claude Parallel Profiles): ~/.claude-paras", or just the
    /// folder.
    var folderSummary = ""
    /// Every folder, for the expanded list (empty when there is only one).
    var folders: [Folder] = []

    /// Text for the hook chip and VoiceOver.
    var hookTitle: String {
        if hookFolderCount == 0, isTracked { return "No folder runs it now" }
        if hookFolderCount > 1, hookState == .installed || hookState == .notInstalled {
            return "Hooks in \(hookedFolderCount) of \(hookFolderCount) folders"
        }
        switch hookState {
        case .installed: return "Hooks installed"
        case .notInstalled: return "Hooks not installed"
        case .off: return "Hooks off"
        case .unreadable: return "settings.json unreadable"
        case .missingFolder: return "Folder missing"
        }
    }
}

/// What the pane can do.
struct SettingsPaneActions {
    var turnOn: () -> Void = {}
    var notNow: () -> Void = {}
    var quitVibeNotch: () -> Void = {}
    /// The notice that the yes now covers VS Code workspaces' folders.
    var acknowledgeScope: () -> Void = {}
    var turnOffAfterScopeNotice: () -> Void = {}

    var rename: (_ ringID: String, _ nickname: String?) -> Void = { _, _ in }
    var setRingShown: (_ ringID: String, _ shown: Bool) -> Void = { _, _ in }
    var setTracked: (_ accountId: String, _ tracked: Bool) -> Void = { _, _ in }
    var installHooks: (_ accountId: String) -> Void = { _ in }
    var removeLegacyHooks: (_ accountId: String, _ kind: AccountSettingsItem.LegacyHooks) -> Void = { _, _ in }
    var copyLaunchCommand: (_ command: String) -> Void = { _ in }
    var reveal: (_ accountId: String) -> Void = { _ in }
    var forget: (_ accountId: String) -> Void = { _ in }
    var addExistingFolder: () -> Void = {}
    var acceptSuggestion: (_ configDir: String) -> Void = { _ in }
    var dismissSuggestion: (_ configDir: String) -> Void = { _ in }
    /// Creates `~/.claude-<name>`; returns its launch command, or throws a
    /// user-facing error.
    var createAccount: (_ name: String) throws -> String = { _ in throw AccountRegistry.CreateAccountError.invalidName }

    var chooseClaudeBinary: () -> Void = {}
    var resetClaudeBinary: () -> Void = {}
    var setHooksEnabled: (Bool) -> Void = { _ in }
    var setStatusLineIntegration: (Bool) -> Void = { _ in }

    var setProbeInterval: (Int) -> Void = { _ in }
    var setReadsDesktopUsageCache: (Bool) -> Void = { _ in }
    var refreshUsage: () -> Void = {}

    /// "Sign in with Google": the host runs the browser step.
    var cloudSignIn: () -> Void = {}
    var cloudSignOut: () -> Void = {}
    var setCloudSync: (Bool) -> Void = { _ in }
    var setSessionSummaries: (Bool) -> Void = { _ in }
    var syncCloudNow: () -> Void = {}
    var openCloudDashboard: () -> Void = {}
    /// "Share accounts…": the website's pools page (`<dashboard>/pools`).
    var openCloudPools: () -> Void = {}
    /// The website's settings page (`<site>/settings`): removing summaries,
    /// deleting synced data.
    var openCloudSettings: () -> Void = {}

    var setAutoOpen: (AutoOpenPolicy) -> Void = { _ in }
    var setHoldOpen: (HoldOpenPolicy) -> Void = { _ in }
    var setRingBadges: (Bool) -> Void = { _ in }
    var setRestingMarks: (Bool) -> Void = { _ in }
    var setDockBadge: (Bool) -> Void = { _ in }
    var setRingClick: (RingClickAction) -> Void = { _ in }
    var setSessionClick: (SessionClickAction) -> Void = { _ in }
    var setHotKey: (PanelHotKey) -> Void = { _ in }

    var setNotifyNeedsInput: (Bool) -> Void = { _ in }
    var setNotifyReadyForReview: (Bool) -> Void = { _ in }
    var openSystemNotificationSettings: () -> Void = {}
    var openNotificationsPane: () -> Void = {}

    var openSessionsPanel: () -> Void = {}
    var copyStateDump: () -> Void = {}
    var resetReviewQueue: () -> Void = {}
}

/// A folder offered as an account.
nonisolated struct FolderSuggestionItem: Identifiable, Equatable, Sendable {
    /// The full path (what accepting adds).
    let configDir: String
    /// `~/.claude-old`.
    let folder: String
    /// Why it wasn't added by itself.
    let reason: String

    var id: String { configDir }

    init(configDir: String, folder: String, reason: String) {
        self.configDir = configDir
        self.folder = folder
        self.reason = reason
    }

    init(_ suggestion: AccountFolderSuggestion, home: String) {
        configDir = suggestion.configDir
        folder = AccountPathDisplay.abbreviated(suggestion.configDir, home: home)
        switch suggestion.reason {
        case .found: reason = "Has Claude Code's folders but no login or live sessions."
        case .looksLikeBackup: reason = "Named like a backup copy."
        case .seenAgain: reason = "Forgotten earlier; a session ran there since."
        }
    }
}

nonisolated enum HooksChangedNotice {
    /// "Updated settings.json in ~/.claude-work and ~/.claude. The previous
    /// version is kept beside each file." from the hook manager's last pass.
    static func text(changedAccountIds: [String], accounts: [ClaudeAccountSummary],
                     backups: [String: String], windowNames: [String: String] = [:], home: String) -> String? {
        // The ids are folders; an account lists all of its own. A VS Code
        // workspace's folder is named after its project when known.
        let folders = changedAccountIds.compactMap { id -> String? in
            if let account = accounts.first(where: { $0.id == id }) {
                return AccountPathDisplay.abbreviated(account.configDir, home: home)
            }
            guard accounts.contains(where: { $0.configDirs.contains(id) }) || id.hasPrefix("/") else { return nil }
            let path = AccountPathDisplay.abbreviated(id, home: home)
            return windowNames[id].map { "\(path) (VS Code · \($0))" } ?? path
        }
        guard !folders.isEmpty else { return nil }
        let list = ListFormatter.localizedString(byJoining: folders)
        let backupNote: String
        if folders.count == 1, let id = changedAccountIds.first, let backup = backups[id] {
            backupNote = "The previous version is kept as \(AccountPathDisplay.abbreviated(backup, home: home))."
        } else {
            backupNote = "The previous version is kept beside each file."
        }
        return "Last change: settings.json in \(list). \(backupNote)"
    }
}

// MARK: - Usage line

nonisolated enum SettingsUsageLine {
    /// "5-hour 34% · weekly 12% · 4m ago", or the reading's status.
    static func text(for reading: ClaudeRingReading?, isRingShown: Bool, isTracked: Bool = true, now: Date) -> String {
        // Tracking off takes the ring and the usage checks with it (GUX-7).
        guard isTracked else { return "Not checked while it isn't tracked" }
        guard isRingShown else { return "Not checked while its ring is off" }
        guard let reading else { return "No reading yet" }
        switch reading.status {
        case .waitingForFirstReading:
            return "Waiting for the first reading"
        case .signInNeeded(let message), .unavailable(let message):
            return message
        case .failed(let message) where reading.windows.isEmpty:
            return "Usage check failed: \(message)"
        case .ok, .failed:
            var parts: [String] = []
            if let session = reading.windows.first(where: { $0.id == "session" }) {
                parts.append("5-hour \(UsageFormatter.percent(session.usedFraction * 100))")
            }
            if let weekly = reading.windows.first(where: { $0.id == "weekly_all" }) {
                parts.append("weekly \(UsageFormatter.percent(weekly.usedFraction * 100))")
            }
            if let updated = reading.updatedAt {
                // Older than the engine's threshold for this probe interval
                // (the ring shows it greyed out too).
                let age = UsageFormatter.age(of: updated, now: now)
                parts.append(reading.isStale(now: now) ? "\(age), stale" : age)
            }
            return parts.isEmpty ? "No limits reported" : parts.joined(separator: " · ")
        }
    }
}

// MARK: - Adding a folder

/// Whether a folder the user picked can be added as an account. Pure, so the
/// rules are tested: the home folder and anything above it are never an
/// account (adding one would write settings.json and hooks/ into it), and a
/// folder with nothing of Claude Code's in it is added only when confirmed.
nonisolated enum AddFolderCheck: Equatable, Sendable {
    case add
    case confirm(String)
    case reject(String)

    /// - Parameters:
    ///   - entries: The names inside the folder.
    ///   - knownConfigDirs: Folders already added.
    ///   - resolvedPath: `path` with symbolic links resolved: a link to the
    ///     home folder is the home folder.
    static func evaluate(path: String, home: String, entries: Set<String>, knownConfigDirs: [String],
                         resolvedPath: String? = nil) -> AddFolderCheck {
        let folder = standardized(path)
        let homePath = standardized(home)
        let resolvedHome = standardized(URL(fileURLWithPath: homePath).resolvingSymlinksInPath().path)
        for candidate in [folder, resolvedPath.map(standardized)].compactMap({ $0 }) {
            for home in Set([homePath, resolvedHome])
            where candidate == home || home.hasPrefix(candidate == "/" ? "/" : candidate + "/") {
                return .reject("That's your home folder, or a folder above it. Choose a Claude Code config folder instead, such as ~/.claude-work.")
            }
        }
        if knownConfigDirs.map(standardized).contains(folder) {
            return .reject("\(AccountPathDisplay.abbreviated(folder, home: homePath)) is already an account.")
        }
        // `.claude.json` alone isn't enough: the home folder has one too.
        if entries.contains("projects") || entries.contains("sessions") || entries.contains("settings.json") {
            return .add
        }
        return .confirm("\(AccountPathDisplay.abbreviated(folder, home: homePath)) doesn't look like a Claude Code config folder (no projects, sessions or settings.json). Add it anyway?")
    }

    private static func standardized(_ path: String) -> String {
        var result = (path as NSString).standardizingPath
        while result.count > 1 && result.hasSuffix("/") { result.removeLast() }
        return result
    }
}
