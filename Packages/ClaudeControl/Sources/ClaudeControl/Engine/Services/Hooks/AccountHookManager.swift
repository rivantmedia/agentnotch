//
//  AccountHookManager.swift
//  ClaudeControl
//
//  Keeps the app's hook script and status line wrapper installed in every
//  tracked account's run folders' settings.json, and only there — never in a
//  Claude Parallel Profiles store or the shared history (`ConfigDirKind`),
//  which are only read (and cleaned of Superpowered Vibe Notch's leftovers
//  when the user takes over from it). A VS Code window's new working copy
//  is a run folder of its account, so it gets the hooks on the pass after
//  the registry finds it. At launch, whenever the
//  set of accounts changes, when a Claude Code older than the one the hooks
//  were written for shows up, and every 10 minutes (a moved folder or a
//  rewritten settings.json is put right; nothing is written where nothing
//  differs). Untracking or forgetting an account
//  takes our hooks out of it. Publishes what is on disk per account for the
//  settings UI, and the setup state for the consent card.
//
//  Nothing is written:
//  - before the user answers "Turn on Claude Code control" with yes
//    (`hookConsent`), or after they turned hooks off (`hooksEnabled`);
//  - while Superpowered Vibe Notch is running (both apps would keep
//    rewriting the same settings.json with their own hooks);
//  - into an account that still has Superpowered Vibe Notch's hooks, until
//    the user takes them over (two blocking PermissionRequest hooks would
//    race over one prompt);
//  - when installs are disabled for the run (`--no-install`).
//  Statuses are still read back so the UI can show them.
//
//  All file work runs off the main actor, one operation at a time.
//

import AppKit
import Combine
import Foundation
import os.log

@MainActor
final class AccountHookManager: ObservableObject {
    static let shared = AccountHookManager()

    nonisolated private static var logger: Logger { EngineLog.logger("Hooks") }

    /// What the manager needs from outside, so tests can stand in for it.
    nonisolated struct Environment {
        /// Installs are disabled for this run (`--no-install`).
        var installsDisabled: @Sendable () -> Bool
        /// Is this bundle id running?
        var isRunning: @MainActor (String) -> Bool
        /// The interpreter for the scripts (resolved once per launch).
        var python: @Sendable () -> String
        /// Versions of every `claude` binary that can be found.
        var binaryVersions: @Sendable ([String]) -> [ClaudeCodeVersion]
        /// Forget where `claude` was found (a retry after moving it).
        var forgetBinary: @Sendable () -> Void
        /// The scripts to write, socket path filled in.
        var hookScript: @Sendable () -> String?
        var statusLineScript: @Sendable () -> String?
        /// Follow app launches and quits (Superpowered Vibe Notch).
        var observesWorkspace: Bool

        static var live: Environment {
            Environment(
                installsDisabled: { DevFlags.installsDisabled },
                isRunning: { !NSRunningApplication.runningApplications(withBundleIdentifier: $0).isEmpty },
                python: { HookInstaller.detectPython() },
                binaryVersions: { ClaudeBinaryLocator.detectVersions(configDirs: $0) },
                forgetBinary: { ClaudeBinaryLocator.forget() },
                hookScript: { EmbeddedScripts.hook(socketPath: AppIdentity.socketPath) },
                statusLineScript: { EmbeddedScripts.statusLine(socketPath: AppIdentity.socketPath) },
                observesWorkspace: true
            )
        }
    }

    /// What each account's settings.json registers, keyed by account ID.
    @Published private(set) var status: [String: AccountHookStatus] = [:]
    /// An install, uninstall or status pass is running.
    @Published private(set) var isWorking = false
    /// The lowest Claude Code version among the binaries found (nil: none found).
    @Published private(set) var detectedVersion: ClaudeCodeVersion?
    /// The user's answer to the consent card (mirrors `hookConsent`).
    @Published private(set) var hookConsent: Bool?
    /// Superpowered Vibe Notch is running: nothing is installed meanwhile.
    @Published private(set) var vibeNotchRunning = false
    /// Upstream Vibe Notch is running; it re-adds its hooks at every launch.
    @Published private(set) var upstreamVibeNotchRunning = false
    /// Accounts whose settings.json the last pass changed, for a notice
    /// saying what was changed and where the backups are.
    @Published private(set) var lastChangedAccounts: [String] = []

    private let registry: AccountRegistry
    private let settings: ClaudeControlSettings.Store
    private let environment: Environment
    private var started = false
    private var cancellables = Set<AnyCancellable>()
    private var recheckTask: Task<Void, Never>?

    /// How often every account is checked again (a folder moved, a script
    /// deleted, settings.json rewritten by a dotfiles sync). Writes happen
    /// only where something differs.
    static let recheckInterval: TimeInterval = 10 * 60
    /// Tail of the serial operation chain.
    private var lastOperation: Task<Void, Never>?
    private var pendingOperations = 0
    /// The lowest version each account's status line reported this run.
    private var observedVersions: [String: ClaudeCodeVersion] = [:]
    /// The version each account's hooks were last written for.
    private var installedVersions: [String: ClaudeCodeVersion?] = [:]
    /// Folders whose `hooks/` holds Superpowered Vibe Notch's scripts.
    @Published private(set) var leftoverScripts: Set<String> = []

    init(
        registry: AccountRegistry? = nil,
        settings: ClaudeControlSettings.Store = ClaudeControlSettings.store,
        environment: Environment = .live
    ) {
        self.registry = registry ?? .shared
        self.settings = settings
        self.environment = environment
        hookConsent = settings.hookConsent
    }

    // MARK: - State

    /// Installs are disabled for this run (`--no-install` / `AGENTNOTCH_NO_INSTALL=1`).
    var installsDisabled: Bool { environment.installsDisabled() }

    /// The user wants hooks installed (consented, and the Hooks switch is on).
    var hooksEnabled: Bool { settings.hooksEnabled }

    /// Whether the status line wrapper is on.
    var statusLineIntegration: Bool { settings.statusLineIntegration }

    func status(for accountId: String) -> AccountHookStatus? {
        status[accountId]
    }

    /// Folders (of any account, stores included) that still have
    /// Superpowered Vibe Notch's hooks.
    var accountsToTakeOver: [ClaudeAccount] {
        registry.accounts.filter { status[$0.id]?.superpoweredVibeNotchHooksPresent == true }
    }

    /// Every folder with Superpowered Vibe Notch's leftovers (entries in its
    /// settings.json, or its scripts in `hooks/`): run folders, stores and
    /// the shared history alike. What "Take over" cleans.
    var vibeNotchCleanupDirs: [String] {
        (registry.accounts.map(\.configDir) + registry.infrastructureDirs).filter { dir in
            status[AccountPaths.accountId(forConfigDir: dir)]?.superpoweredVibeNotchHooksPresent == true
                || leftoverScripts.contains(AccountPaths.accountId(forConfigDir: dir))
        }
    }

    /// The folders "Turn on" installs into: every tracked run folder
    /// (`~/.claude`, VS Code windows, standalone folders). Never a store.
    var installTargets: [ClaudeAccount] {
        registry.accounts.filter { isTracked($0) }
    }

    /// Whether a folder gets our hooks: a run folder of a tracked account.
    /// `~/.claude`, while Claude Parallel Profiles mirrors accounts into it,
    /// is not any one account's: whoever the focused VS Code window runs as
    /// is copied in, so tracking one account would add and remove our hooks
    /// on every focus switch. It keeps them while any account is tracked
    /// (sessions of an untracked one are hidden, and their permission
    /// requests left to their terminal, see `HookSocketServer`).
    func isTracked(_ folder: ClaudeAccount) -> Bool {
        guard folder.kind == .run else { return false }
        if registry.mirrorsDefault, registry.isDefault(folder) {
            return registry.identities.contains { !$0.isHidden }
        }
        return !folder.isHidden
    }

    /// The settings.json files "Turn on" will edit (tracked run folders).
    var settingsFilesToEdit: [String] {
        installTargets.map { HookInstaller.settingsFile(configDir: $0.configDir).path }
    }

    /// What "Turn on" covers now: `~/.claude`, standalone folders, and every
    /// VS Code window's working copy (new ones as they appear). Earlier
    /// builds' yes (0) covered each account's own folder; VS Code windows'
    /// folders weren't run folders of an account then.
    static let installScope = 2

    /// VS Code windows' folders that get the hooks under a yes given before
    /// they were covered: said once (`acknowledgeInstallScope`).
    var foldersBeyondConsent: [String] {
        guard settings.hookConsent == true, settings.hooksEnabled, settings.hookConsentScope < Self.installScope else { return [] }
        let home = registry.homePath
        return installTargets.filter { ParallelProfiles.isWindowDir($0.configDir, home: home) }.map(\.configDir)
    }

    /// The user saw what the yes covers now.
    func acknowledgeInstallScope() {
        settings.hookConsentScope = Self.installScope
        objectWillChange.send()
    }

    /// What the consent card and banners need (see `ClaudeSetupState`).
    func setupState(isSealed: Bool, socketError: String?) -> ClaudeSetupState {
        guard !isSealed else { return ClaudeSetupState(socketError: socketError) }
        let visible = registry.visibleAccounts
        let vibeNotchCleanup = !vibeNotchCleanupDirs.isEmpty
        return ClaudeSetupState(
            needsHookConsent: settings.hookConsent == nil,
            vibeNotchRunning: vibeNotchRunning,
            legacyHooksFound: visible.contains { status[$0.id]?.legacyHooksPresent == true } || vibeNotchCleanup,
            socketError: socketError,
            superpoweredVibeNotchHooksFound: vibeNotchCleanup,
            vibeNotchHooksFound: visible.contains { status[$0.id]?.vibeNotchHooksPresent == true },
            newInstallFolders: foldersBeyondConsent
        )
    }

    // MARK: - Lifecycle

    /// Read every account now and install where allowed; again whenever the
    /// set of accounts changes, a lower Claude Code version reports in, or
    /// Superpowered Vibe Notch quits. Idempotent.
    func start() {
        guard !started else { return }
        started = true
        refreshRunningApps()

        Publishers.CombineLatest(registry.$accounts, registry.$identities)
            .map { [weak self] accounts, _ in
                accounts.map { "\(self?.isTracked($0) == true ? "+" : "-")\($0.kind.rawValue):\($0.configDir)" }.sorted()
            }
            .removeDuplicates()
            .dropFirst()
            .debounce(for: .seconds(1), scheduler: DispatchQueue.main)
            .sink { [weak self] _ in
                self?.installAll()
            }
            .store(in: &cancellables)

        AppEventBus.shared.statusLineUpdates
            .receive(on: DispatchQueue.main)
            .sink { [weak self] update in
                guard let accountId = update.accountId,
                      let version = update.claudeCodeVersion.flatMap(ClaudeCodeVersion.parse) else { return }
                self?.noteVersion(version, accountId: accountId)
            }
            .store(in: &cancellables)

        if environment.observesWorkspace {
            let center = NSWorkspace.shared.notificationCenter
            for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
                center.publisher(for: name)
                    .compactMap { ($0.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.bundleIdentifier }
                    .filter { [AppIdentity.vibeNotchBundleIdentifier, AppIdentity.upstreamVibeNotchBundleIdentifier].contains($0) }
                    .receive(on: DispatchQueue.main)
                    .sink { [weak self] _ in self?.runningAppsChanged() }
                    .store(in: &cancellables)
            }
        }

        installAll()

        recheckTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(Self.recheckInterval))
                guard !Task.isCancelled, let self else { return }
                self.installAll()
            }
        }
    }

    func stop() {
        cancellables.removeAll()
        recheckTask?.cancel()
        recheckTask = nil
        started = false
    }

    // MARK: - Consent

    /// "Turn on Claude Code control": remember the yes, take over from
    /// Superpowered Vibe Notch where its hooks are (if asked), and install
    /// into every tracked account. Refused (false) while Superpowered Vibe
    /// Notch is running.
    @discardableResult
    func grantConsent(takeOverFromVibeNotch: Bool = true) -> Bool {
        refreshRunningApps()
        guard !vibeNotchRunning else { return false }
        settings.hookConsent = true
        settings.hooksEnabled = true
        settings.hookConsentScope = Self.installScope
        hookConsent = true
        environment.forgetBinary()
        if takeOverFromVibeNotch {
            takeOver(accountIds: nil)
        } else {
            installAll()
        }
        return true
    }

    /// "Not now": remember the no. Nothing is written.
    func declineConsent() {
        settings.hookConsent = false
        hookConsent = false
    }

    // MARK: - Operations

    /// Install into every tracked account where allowed (and take ours out
    /// of untracked ones); otherwise just read back what is there.
    ///
    /// - Parameter relocateClaude: forget the remembered `claude` path first,
    ///   so a Claude Code installed somewhere new is found.
    func installAll(relocateClaude: Bool = false) {
        enqueue { manager in
            if relocateClaude {
                let forget = manager.environment.forgetBinary
                await Task.detached { forget() }.value
            }
            await manager.runPass()
        }
    }

    /// Install into one account (e.g. right after the user adds it). Runs the
    /// whole pass, which writes nothing where hooks are already current, so an
    /// account sharing its settings.json with another is handled the same way
    /// as at launch.
    func install(accountId: String) {
        guard !registry.folders(for: accountId).isEmpty else { return }
        installAll()
    }

    /// The Hooks switch on (an explicit yes, so it also answers the consent
    /// card): remember it and install everywhere.
    func enableHooks() {
        settings.hookConsent = true
        settings.hooksEnabled = true
        settings.hookConsentScope = Self.installScope
        hookConsent = true
        installAll(relocateClaude: true)
    }

    /// Turn hooks off: remember the choice and uninstall from every account
    /// (untracked ones too), restoring each previous status line.
    func disableHooks() {
        settings.hooksEnabled = false
        enqueue { manager in
            await manager.runUninstall(accounts: manager.registry.accounts.filter { manager.status[$0.id]?.hooksRegistered == true || manager.status[$0.id]?.statusLineInstalled == true || $0.kind == .run })
        }
    }

    /// Remove our hooks and status line wrapper from one account.
    func uninstall(accountId: String) {
        enqueue { manager in
            let folders = manager.registry.folders(for: accountId)
            guard !folders.isEmpty else { return }
            await manager.runUninstall(accounts: folders)
        }
    }

    /// "Track sessions and hooks" on or off. Off takes our hooks out of the
    /// account's settings.json (unless a tracked account shares that file).
    func setTracked(accountId: String, _ tracked: Bool) {
        registry.setHidden(id: accountId, !tracked)
        installAll()
    }

    /// Forget an account: take our hooks out of it first (while its folder
    /// is still known), then drop it from the registry.
    func forget(accountId: String) {
        enqueue { manager in
            // An identity: every run folder of it (stores are never ours).
            // A mirrored ~/.claude stays hooked while another account is
            // tracked (see `isTracked`).
            let registry = manager.registry
            let othersTracked = registry.identities.contains { $0.id != accountId && !$0.isHidden }
            let folders = registry.folders(for: accountId).filter { folder in
                folder.kind == .run && !(registry.mirrorsDefault && registry.isDefault(folder) && othersTracked)
            }
            guard !folders.isEmpty else {
                registry.remove(id: accountId)
                manager.dropForgottenStatuses()
                return
            }
            let ids = Set(folders.map(\.id))
            let others = registry.visibleAccounts.filter { !ids.contains($0.id) }
            let removable = folders.filter { folder in
                !others.contains {
                    Self.settingsFileIdentity(configDir: $0.configDir) == Self.settingsFileIdentity(configDir: folder.configDir)
                }
            }
            if !manager.installsDisabled, !removable.isEmpty {
                await manager.runUninstall(accounts: removable)
            }
            manager.registry.remove(id: accountId)
            manager.dropForgottenStatuses()
        }
    }

    /// Take over from Superpowered Vibe Notch: remove its hooks, put back the
    /// status line its wrapper replaced (from its own saved copy), then
    /// install ours where hooks are on. All tracked accounts when `accountIds`
    /// is nil. Refused (false) while it is running.
    ///
    /// Taking over from all of it (`accountIds` nil) cleans every folder it
    /// wrote to, including those nothing of ours goes into: Claude Parallel
    /// Profiles stores and the shared history. Its entries come out (its
    /// status line put back), its scripts go once nothing runs them, and a
    /// settings.json it created there that is now `{}` goes too (see
    /// `VibeNotchLeftovers`).
    @discardableResult
    func takeOver(accountIds: [String]?) -> Bool {
        refreshRunningApps()
        guard !vibeNotchRunning else { return false }
        enqueue { manager in
            let targets: [(String, String, ConfigDirKind)]
            if let accountIds {
                var seen = Set<String>()
                targets = accountIds.flatMap { manager.registry.folders(for: $0) }
                    .filter { seen.insert($0.id).inserted }
                    .map { ($0.id, $0.configDir, $0.kind) }
            } else {
                let kinds = Dictionary(manager.registry.accounts.map { ($0.id, $0.kind) }, uniquingKeysWith: { first, _ in first })
                targets = manager.vibeNotchCleanupDirs.map { dir in
                    let id = AccountPaths.accountId(forConfigDir: dir)
                    return (id, dir, kinds[id] ?? .infrastructure)
                }
            }
            await manager.removeLegacy(targets: targets.map { ($0.0, $0.1) }, kinds: [.superpoweredVibeNotch])
            await manager.cleanVibeNotchLeftovers(targets)
            await manager.runPass()
        }
        return true
    }

    /// After Superpowered Vibe Notch's entries came out: its scripts where
    /// nothing runs them any more, and `{}` settings it created in stores
    /// and the shared history.
    private func cleanVibeNotchLeftovers(_ targets: [(String, String, ConfigDirKind)]) async {
        guard !installsDisabled, !targets.isEmpty else { return }
        let known = registry.accounts.map(\.configDir) + registry.infrastructureDirs
        let home = AccountPaths.homeDirectory
        await Task.detached(priority: .userInitiated) {
            // An unreadable settings.json may still run its scripts: none go
            // until it can be read (a later pass tries again).
            guard let referenced = VibeNotchLeftovers.referencedScriptsIfAllReadable(configDirs: known, home: home) else {
                Self.logger.notice("Leaving Superpowered Vibe Notch's scripts for now: a settings.json can't be read")
                return
            }
            for (_, configDir, kind) in targets {
                VibeNotchLeftovers.cleanUp(configDir: configDir, referenced: referenced,
                                           removesBlankSettings: kind != .run, home: home)
            }
        }.value
        await readStatuses(of: [], extraDirs: targets.map(\.1), outcome: nil)
    }

    /// Remove another app's hook entries from one account. Explicit user
    /// action only; nothing else is touched. Superpowered Vibe Notch's are
    /// a takeover of that account, refused (false) while it runs, as
    /// `takeOver` is.
    @discardableResult
    func removeLegacyHooks(accountId: String, kind: LegacyHookKind) -> Bool {
        if kind == .superpoweredVibeNotch {
            refreshRunningApps()
            guard !vibeNotchRunning else { return false }
        }
        enqueue { manager in
            let folders = manager.registry.folders(for: accountId)
            guard !folders.isEmpty else { return }
            await manager.removeLegacy(targets: folders.map { ($0.id, $0.configDir) }, kinds: [kind])
            if kind == .superpoweredVibeNotch {
                await manager.cleanVibeNotchLeftovers(folders.map { ($0.id, $0.configDir, $0.kind) })
                // The account can have ours now.
                await manager.runPass()
            }
        }
        return true
    }

    /// Turn the status line wrapper on or off everywhere. Off restores each
    /// account's previous status line exactly.
    func setStatusLineIntegration(_ enabled: Bool) {
        guard settings.statusLineIntegration != enabled else { return }
        settings.statusLineIntegration = enabled
        installAll()
    }

    /// Re-read every account's settings.json without writing anything.
    func refreshStatus() {
        enqueue { manager in
            await manager.readAllStatuses()
        }
    }

    /// Quit Superpowered Vibe Notch (the user's click on "Quit it"; never
    /// done on our own). Only that exact bundle id.
    func quitVibeNotch() {
        for app in NSRunningApplication.runningApplications(withBundleIdentifier: AppIdentity.vibeNotchBundleIdentifier) {
            app.terminate()
        }
    }

    // MARK: - Versions

    /// A status line reported the Claude Code version a session runs. If it
    /// is older than what the account's hooks were written for, rewrite them
    /// for it (an older Claude Code may ignore a settings.json that names
    /// events it doesn't know).
    func noteVersion(_ version: ClaudeCodeVersion, accountId: String) {
        if let known = observedVersions[accountId], known <= version { return }
        observedVersions[accountId] = version
        // Only hooks written for a newer version need rewriting (never
        // written, or the baseline set: nothing to take back).
        guard let installed = installedVersions[accountId], let written = installed, version < written else { return }
        Self.logger.notice("Claude Code \(version.description, privacy: .public) seen in \(accountId, privacy: .public); rewriting its hooks for it")
        installAll()
    }

    /// The version to write an account's hooks for: the lowest of every
    /// binary found, every version its sessions reported, and every live
    /// session in its registry. Nil (the baseline events) when none is known.
    nonisolated static func effectiveVersion(
        binaries: [ClaudeCodeVersion],
        observed: ClaudeCodeVersion?,
        sessions: [ClaudeCodeVersion]
    ) -> ClaudeCodeVersion? {
        (binaries + sessions + [observed].compactMap { $0 }).min()
    }

    /// Versions of the live sessions in `<configDir>/sessions/<pid>.json`.
    /// Only `.json` files are read, and only those whose process is alive
    /// (a crashed session's file would pin the account to its version).
    nonisolated static func sessionVersions(
        configDir: String,
        isAlive: (Int32) -> Bool = { pid in kill(pid, 0) == 0 || errno == EPERM }
    ) -> [ClaudeCodeVersion] {
        let directory = (AccountPaths.normalize(configDir) as NSString).appendingPathComponent("sessions")
        let names = ((try? FileManager.default.contentsOfDirectory(atPath: directory)) ?? [])
            .filter { $0.hasSuffix(".json") }
            .prefix(64)
        return names.compactMap { name in
            guard let pid = Int32(name.dropLast(".json".count)), pid > 0, isAlive(pid),
                  let data = try? Data(contentsOf: URL(fileURLWithPath: (directory as NSString).appendingPathComponent(name))),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let text = json["version"] as? String else { return nil }
            return ClaudeCodeVersion.parse(text)
        }
    }

    // MARK: - Running apps

    private func refreshRunningApps() {
        let running = environment.isRunning(AppIdentity.vibeNotchBundleIdentifier)
        if vibeNotchRunning != running { vibeNotchRunning = running }
        let upstream = environment.isRunning(AppIdentity.upstreamVibeNotchBundleIdentifier)
        if upstreamVibeNotchRunning != upstream { upstreamVibeNotchRunning = upstream }
    }

    private func runningAppsChanged() {
        let wasBlocked = vibeNotchRunning
        refreshRunningApps()
        if wasBlocked && !vibeNotchRunning {
            // It quit: whatever it rewrote gets checked (and ours put back).
            installAll()
        } else {
            refreshStatus()
        }
    }

    // MARK: - Passes

    private struct Target: Sendable {
        let id: String
        let configDir: String
        let tracked: Bool
        /// Only run folders get hooks; stores and the shared history are
        /// read (and have ours taken out, should an older build have left any).
        var kind: ConfigDirKind = .run
    }

    private struct PassResult: Sendable {
        let id: String
        let outcome: HookInstallOutcome?
        let status: AccountHookStatus
        /// Set when hooks were written (or checked) for a version.
        let version: ClaudeCodeVersion??
    }

    private func runPass() async {
        refreshRunningApps()
        let accounts = registry.accounts
        let infrastructure = registry.infrastructureDirs
        guard !installsDisabled else {
            await readStatuses(of: accounts, extraDirs: infrastructure, outcome: .disabled)
            return
        }

        // Nothing is installed before the folders were classified: a store
        // must never be taken for a run folder.
        let canInstall = settings.hooksEnabled && !vibeNotchRunning && registry.hasClassified
        // After the yes (which takes over from Superpowered Vibe Notch), what
        // it left behind is tidied on every pass: its entries in stores and
        // the shared history, its scripts nothing runs any more, and a `{}`
        // settings.json it (or an earlier build of this app) left in a store.
        let tidies = settings.hookConsent == true && !vibeNotchRunning && registry.hasClassified
        let home = registry.homePath
        let knownDirs = accounts.map(\.configDir) + infrastructure
        let statusLineIntegration = settings.statusLineIntegration
        let targets = accounts.map { Target(id: $0.id, configDir: $0.configDir, tracked: isTracked($0), kind: $0.kind) }
            + infrastructure.map { Target(id: AccountPaths.accountId(forConfigDir: $0), configDir: $0, tracked: false, kind: .infrastructure) }
        let observed = observedVersions
        let detectPython = environment.python
        let detectBinaryVersions = environment.binaryVersions
        let hookScript = environment.hookScript
        let statusLineScript = environment.statusLineScript

        let (results, binaryVersions, scripts): ([PassResult], [ClaudeCodeVersion]?, Set<String>) = await Task.detached(priority: .utility) {
            let current = targets.map { HookInstaller.readStatus(configDir: $0.configDir) }
            var scripts = Set(targets.filter { VibeNotchLeftovers.hasScripts(configDir: $0.configDir) }.map(\.id))
            // One write per settings.json file. Accounts sharing one (a
            // symlinked settings.json, or a config dir linked to another)
            // would otherwise take turns rewriting it with their own script
            // paths on every pass; they report the first account's outcome.
            // A file a tracked account uses keeps our hooks.
            let files = targets.map { Self.settingsFileIdentity(configDir: $0.configDir) }
            let trackedFiles = Set(zip(targets, files).filter { $0.0.tracked }.map(\.1))
            var outcomeByFile: [String: HookInstallOutcome] = [:]
            var python: String?
            var binaryVersions: [ClaudeCodeVersion]?
            var results: [PassResult] = []

            for (index, target) in targets.enumerated() {
                let file = files[index]
                var outcome: HookInstallOutcome?
                var version: ClaudeCodeVersion??
                if target.tracked {
                    if canInstall, !current[index].superpoweredVibeNotchHooksPresent {
                        if let shared = outcomeByFile[file] {
                            outcome = shared
                        } else {
                            let resolvedPython = python ?? detectPython()
                            python = resolvedPython
                            let binaries = binaryVersions ?? detectBinaryVersions(targets.filter { $0.kind == .run }.map(\.configDir))
                            binaryVersions = binaries
                            let effective = Self.effectiveVersion(
                                binaries: binaries,
                                observed: observed[target.id],
                                sessions: Self.sessionVersions(configDir: target.configDir)
                            )
                            version = .some(effective)
                            outcome = HookInstaller.install(configDir: target.configDir, configuration: HookInstaller.Configuration(
                                python: resolvedPython,
                                version: effective,
                                statusLineIntegration: statusLineIntegration,
                                hookScript: hookScript(),
                                statusLineScript: statusLineScript()
                            ))
                            outcomeByFile[file] = outcome
                        }
                    }
                } else if current[index].hooksRegistered || current[index].statusLineInstalled, !trackedFiles.contains(file) {
                    // Not tracked any more (or a store): ours come out.
                    let removed = outcomeByFile[file] ?? HookInstaller.uninstall(configDir: target.configDir)
                    outcomeByFile[file] = removed
                    outcome = removed
                }
                let status = outcome == nil ? current[index] : HookInstaller.readStatus(configDir: target.configDir)
                results.append(PassResult(id: target.id, outcome: outcome, status: status, version: version))
            }
            if tidies {
                results = Self.tidyVibeNotchLeftovers(targets: targets, results: results, knownDirs: knownDirs, home: home)
                scripts = Set(targets.filter { VibeNotchLeftovers.hasScripts(configDir: $0.configDir) }.map(\.id))
            }
            return (results, binaryVersions, scripts)
        }.value
        if leftoverScripts != scripts { leftoverScripts = scripts }

        if let binaryVersions {
            detectedVersion = binaryVersions.min()
        }
        var changed: [String] = []
        for result in results {
            apply(result.status, outcome: result.outcome, to: result.id)
            if let version = result.version { installedVersions[result.id] = version }
            if result.outcome?.wroteSettings == true { changed.append(result.id) }
        }
        if !changed.isEmpty { lastChangedAccounts = changed }
        dropForgottenStatuses()
    }

    /// After consent: Superpowered Vibe Notch's entries out of stores and
    /// the shared history (nothing of ours goes there, so nothing else
    /// would), its unreferenced scripts out of every folder, and a `{}`
    /// settings.json left in a store or the shared history removed when no
    /// backup beside it holds anything of the user's (see
    /// `VibeNotchLeftovers`), with those backups and an empty `hooks/`.
    /// Returns the results with fresh statuses where something changed.
    nonisolated private static func tidyVibeNotchLeftovers(targets: [Target], results: [PassResult],
                                                           knownDirs: [String], home: String) -> [PassResult] {
        var results = results
        var touched = false
        for (index, target) in targets.enumerated() where target.kind != .run {
            let status = results[index].status
            guard status.settingsReadable, status.superpoweredVibeNotchHooksPresent else { continue }
            let outcome = HookInstaller.removeLegacyHooks(configDir: target.configDir, kinds: [.superpoweredVibeNotch])
            results[index] = PassResult(id: target.id, outcome: outcome, status: HookInstaller.readStatus(configDir: target.configDir),
                                        version: results[index].version)
            touched = true
        }
        let needsTidy = targets.indices.filter { index in
            let target = targets[index]
            let status = results[index].status
            guard status.settingsReadable, !status.superpoweredVibeNotchHooksPresent else { return false }
            if VibeNotchLeftovers.hasScripts(configDir: target.configDir) { return true }
            return target.kind != .run && !status.hooksRegistered && !status.statusLineInstalled
                && VibeNotchLeftovers.hasEmptyObjectSettings(configDir: target.configDir)
        }
        guard !needsTidy.isEmpty else { return results }
        guard let referenced = VibeNotchLeftovers.referencedScriptsIfAllReadable(configDirs: knownDirs, home: home) else {
            logger.notice("Leaving Superpowered Vibe Notch's leftovers for now: a settings.json can't be read")
            return results
        }
        for index in needsTidy {
            let target = targets[index]
            let removed = VibeNotchLeftovers.cleanUp(configDir: target.configDir, referenced: referenced,
                                                     removesBlankSettings: target.kind != .run, home: home)
            if !removed.removedFiles.isEmpty || touched {
                results[index] = PassResult(id: target.id, outcome: results[index].outcome,
                                            status: HookInstaller.readStatus(configDir: target.configDir),
                                            version: results[index].version)
            }
        }
        return results
    }

    /// The real file behind an account's settings.json, with every symlink
    /// (of the file or of the config dir) resolved.
    nonisolated static func settingsFileIdentity(configDir: String) -> String {
        URL(fileURLWithPath: AccountPaths.normalize(configDir), isDirectory: true)
            .resolvingSymlinksInPath()
            .appendingPathComponent("settings.json")
            .resolvingSymlinksInPath()
            .path
    }

    private func runUninstall(accounts: [ClaudeAccount]) async {
        guard !installsDisabled else {
            await readStatuses(of: accounts, outcome: .disabled)
            return
        }
        let targets = accounts.map { ($0.id, $0.configDir) }
        let results = await Task.detached(priority: .userInitiated) {
            targets.map { id, configDir in
                (id, HookInstaller.uninstall(configDir: configDir), HookInstaller.readStatus(configDir: configDir))
            }
        }.value
        for (id, outcome, diskStatus) in results {
            apply(diskStatus, outcome: outcome, to: id)
            installedVersions.removeValue(forKey: id)
        }
    }

    private func removeLegacy(targets: [(String, String)], kinds: Set<LegacyHookKind>) async {
        guard !installsDisabled else {
            await readStatuses(of: [], extraDirs: targets.map(\.1), outcome: .disabled)
            return
        }
        let results = await Task.detached(priority: .userInitiated) {
            targets.map { id, configDir in
                (id, HookInstaller.removeLegacyHooks(configDir: configDir, kinds: kinds), HookInstaller.readStatus(configDir: configDir))
            }
        }.value
        for (id, outcome, diskStatus) in results {
            apply(diskStatus, outcome: outcome, to: id)
        }
    }

    private func readAllStatuses(outcome: HookInstallOutcome? = nil) async {
        await readStatuses(of: registry.accounts, extraDirs: registry.infrastructureDirs, outcome: outcome)
    }

    private func readStatuses(of accounts: [ClaudeAccount], extraDirs: [String] = [], outcome: HookInstallOutcome?) async {
        let targets = accounts.map { ($0.id, $0.configDir) }
            + extraDirs.map { (AccountPaths.accountId(forConfigDir: $0), $0) }
        let results = await Task.detached(priority: .utility) {
            targets.map { id, configDir in
                (id, HookInstaller.readStatus(configDir: configDir), VibeNotchLeftovers.hasScripts(configDir: configDir))
            }
        }.value
        var scripts = leftoverScripts
        for (id, diskStatus, hasScripts) in results {
            apply(diskStatus, outcome: outcome, to: id)
            if hasScripts { scripts.insert(id) } else { scripts.remove(id) }
        }
        if leftoverScripts != scripts { leftoverScripts = scripts }
        dropForgottenStatuses()
    }

    /// Drop folders the registry no longer knows (infrastructure stays).
    fileprivate func dropForgottenStatuses() {
        let known = Set(registry.accounts.map(\.id) + registry.infrastructureDirs.map(AccountPaths.accountId(forConfigDir:)))
        for id in status.keys where !known.contains(id) {
            status.removeValue(forKey: id)
        }
        let scripts = leftoverScripts.filter(known.contains)
        if scripts != leftoverScripts { leftoverScripts = scripts }
    }

    private func apply(_ diskStatus: AccountHookStatus, outcome: HookInstallOutcome?, to accountId: String) {
        var updated = diskStatus
        let previous = status[accountId]
        updated.lastOutcome = outcome ?? previous?.lastOutcome
        updated.lastError = outcome.map { $0.errorMessage } ?? previous?.lastError
        if let error = outcome?.errorMessage {
            Self.logger.error("Hooks for \(accountId, privacy: .public): \(error, privacy: .public)")
        }
        if status[accountId] != updated {
            status[accountId] = updated
            Self.logger.info("Hooks for \(accountId, privacy: .public): installed=\(updated.hooksInstalled) statusLine=\(updated.statusLineInstalled) legacy=\(updated.legacyHooksPresent) readable=\(updated.settingsReadable)")
        }
    }

    /// Run operations one after another, in the order they were requested.
    private func enqueue(_ operation: @escaping @MainActor (AccountHookManager) async -> Void) {
        let previous = lastOperation
        pendingOperations += 1
        isWorking = true
        lastOperation = Task { [weak self] in
            await previous?.value
            guard let self else { return }
            await operation(self)
            self.pendingOperations -= 1
            if self.pendingOperations == 0 {
                self.isWorking = false
            }
        }
    }

    /// Wait for every queued operation (tests).
    func waitUntilIdle() async {
        while let operation = lastOperation {
            await operation.value
            if operation == lastOperation { break }
        }
    }
}
