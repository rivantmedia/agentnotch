//
//  HookInstaller.swift
//  ClaudeControl
//
//  Installs the hook script and the status line wrapper into one Claude Code
//  config dir (one account): writes the embedded scripts (socket path filled
//  in) into `<configDir>/hooks/` and registers them in
//  `<configDir>/settings.json`. Also removes other notch apps' entries when
//  the user asks (upstream Vibe Notch, Superpowered Vibe Notch, Vibe Island),
//  restoring the status line they wrapped. AccountHookManager runs it for
//  every account.
//
//  settings.json is the user's file and the one thing this app writes that can
//  destroy something they cannot get back, so every write here:
//  - refuses outright when the file can't be parsed as a JSON object, when
//    its `hooks` isn't an object, or when it is a symlink to nothing;
//  - is skipped when nothing would change (compared as JSON);
//  - changes only the `hooks` and `statusLine` values, in the file's own
//    layout: every other byte stays as it was (`SettingsDocument`);
//  - backs up the current bytes first (owner-only, newest 5 kept, plus the
//    very first one kept for good);
//  - is staged in a temporary file and renamed into place right after a
//    last check that the file is still the one planned from (inode, time,
//    size and bytes), so a concurrent save by Claude Code is never lost;
//  - goes through a symlinked settings.json to its target, with the
//    target's permissions;
//  - touches only entries that run a known script (see `HookCommands`).
//
//  The commands it writes fail open: a missing script exits 0, never the
//  exit status 2 that Claude Code treats as "block".
//
//  All functions are nonisolated and block on file IO (and, for version and
//  interpreter detection, a subprocess), so call them off the main actor.
//

import Darwin
import Foundation
import os.log

/// Result of an install, uninstall or legacy-removal attempt on one account.
nonisolated enum HookInstallOutcome: Equatable, Sendable {
    /// settings.json was rewritten with our entries.
    case installed
    /// settings.json was rewritten without our (or the legacy) entries.
    case removed
    /// settings.json already said exactly what we wanted; nothing written.
    case alreadyCurrent
    /// settings.json exists but could not be read as a JSON object. Left untouched.
    case settingsUnreadable
    /// settings.json's `hooks` is not an object. Left untouched.
    case settingsHooksMalformed
    /// settings.json is a symlink whose target doesn't exist (a dotfiles repo
    /// not cloned yet?). Left untouched rather than replaced by a file.
    case settingsLinkBroken(String)
    /// A write failed. settings.json is unchanged (writes are atomic).
    case writeFailed(String)
    /// The account's config dir does not exist (yet).
    case configDirMissing
    /// Installation is disabled for this run (`--no-install` / `SPCN_NO_INSTALL=1`),
    /// or the folder is the real one of the user running tests. Nothing was
    /// read or written.
    case disabled
    /// A Claude Parallel Profiles store or the shared history: Claude Code
    /// never runs there, and nothing of ours is ever installed there.
    case notAnInstallTarget

    /// Short, user-facing description of a failure; nil for success outcomes.
    var errorMessage: String? {
        switch self {
        case .installed, .removed, .alreadyCurrent, .disabled:
            return nil
        case .settingsUnreadable:
            return "settings.json isn't valid JSON, so it was left alone. Fix it and try again."
        case .settingsHooksMalformed:
            return "settings.json has a \"hooks\" value that isn't an object, so it was left alone."
        case .settingsLinkBroken(let target):
            return "settings.json links to \(target), which doesn't exist, so it was left alone."
        case .writeFailed(let message):
            return message
        case .configDirMissing:
            return "The config folder doesn't exist."
        case .notAnInstallTarget:
            return "This folder is an account store of Claude Parallel Profiles (or the shared history); nothing is installed there."
        }
    }

    /// settings.json was changed.
    var wroteSettings: Bool { self == .installed || self == .removed }
}

/// Another notch app's hooks, which we never remove on our own.
nonisolated enum LegacyHookKind: String, CaseIterable, Hashable, Sendable {
    /// Upstream Vibe Notch: `claude-island-state.py`.
    case vibeNotch
    /// Superpowered Vibe Notch: `superpowered-notch-hook.py` and its status
    /// line wrapper. Taken over rather than stacked on.
    case superpoweredVibeNotch
    /// Vibe Island's bridge (`~/.vibe-island/bin/`), and its status line.
    case vibeIsland

    var appName: String {
        switch self {
        case .vibeNotch: return "Vibe Notch"
        case .superpoweredVibeNotch: return "Superpowered Vibe Notch"
        case .vibeIsland: return "Vibe Island"
        }
    }
}

/// What is on disk for one account, read back from its settings.json.
nonisolated struct AccountHookStatus: Equatable, Sendable {
    /// The config dir exists.
    var configDirExists: Bool = false
    /// False when settings.json exists but isn't a readable JSON object (or
    /// its `hooks` isn't an object, or it links to nothing).
    var settingsReadable: Bool = true
    /// Our hook command is registered and the script is in place.
    var hooksInstalled: Bool = false
    /// Our hook command is registered at all (even if its script is gone).
    var hooksRegistered: Bool = false
    /// The account's `statusLine` runs our wrapper.
    var statusLineInstalled: Bool = false
    /// Upstream Vibe Notch's `claude-island-state.py` hooks are registered.
    var vibeNotchHooksPresent: Bool = false
    /// Superpowered Vibe Notch's hooks or status line wrapper are registered.
    var superpoweredVibeNotchHooksPresent: Bool = false
    /// Vibe Island's bridge hooks or status line are registered.
    var vibeIslandHooksPresent: Bool = false
    /// Our hook command as Claude Code will see it.
    var hookCommand: String?
    /// Newest backup of settings.json this app made, if any.
    var newestBackupPath: String?
    /// Result of the last install/uninstall this run (set by AccountHookManager).
    var lastOutcome: HookInstallOutcome?
    /// User-facing message for the last failure, if any.
    var lastError: String?

    /// Hooks another notch app left behind that we take over or remove on
    /// request (Vibe Notch and Superpowered Vibe Notch; Vibe Island is an
    /// app of its own and only reported).
    var legacyHooksPresent: Bool { vibeNotchHooksPresent || superpoweredVibeNotchHooksPresent }

    func has(_ kind: LegacyHookKind) -> Bool {
        switch kind {
        case .vibeNotch: return vibeNotchHooksPresent
        case .superpoweredVibeNotch: return superpoweredVibeNotchHooksPresent
        case .vibeIsland: return vibeIslandHooksPresent
        }
    }
}

nonisolated enum HookInstaller {
    private static var logger: Logger { EngineLog.logger("Hooks") }

    /// Kept so existing call sites and tests can keep spelling it this way.
    typealias ClaudeCodeVersion = ClaudeControl.ClaudeCodeVersion

    /// Where the previous `statusLine` object is kept while ours wraps it.
    /// Lives next to the wrapper script, which reads it to chain the command.
    static let previousStatusLineFileName = "superpowered-codenotch-statusline.previous.json"

    // MARK: - Configuration

    /// Everything an install needs besides the config dir. Resolved once per
    /// pass by AccountHookManager (version detection runs a subprocess).
    struct Configuration: Sendable {
        /// Interpreter for the scripts: an absolute path, or a name on PATH.
        var python: String
        /// The lowest Claude Code version known to use this account; nil
        /// registers the baseline events only.
        var version: ClaudeCodeVersion?
        /// Wrap the account's statusLine with ours (else restore it).
        var statusLineIntegration: Bool
        /// Hook script to write, socket path filled in (see `EmbeddedScripts`).
        var hookScript: String?
        /// Status line wrapper to write, socket path filled in.
        var statusLineScript: String?

        init(
            python: String,
            version: ClaudeCodeVersion?,
            statusLineIntegration: Bool,
            hookScript: String? = EmbeddedScripts.hook(socketPath: AppIdentity.socketPath),
            statusLineScript: String? = EmbeddedScripts.statusLine(socketPath: AppIdentity.socketPath)
        ) {
            self.python = python
            self.version = version
            self.statusLineIntegration = statusLineIntegration
            self.hookScript = hookScript
            self.statusLineScript = statusLineScript
        }
    }

    // MARK: - Paths

    static func hooksDir(configDir: String) -> URL {
        URL(fileURLWithPath: AccountPaths.normalize(configDir), isDirectory: true).appendingPathComponent("hooks")
    }

    static func settingsFile(configDir: String) -> URL {
        URL(fileURLWithPath: AccountPaths.normalize(configDir), isDirectory: true).appendingPathComponent("settings.json")
    }

    static func hookScriptURL(configDir: String) -> URL {
        hooksDir(configDir: configDir).appendingPathComponent(AppIdentity.hookScriptName)
    }

    static func statusLineScriptURL(configDir: String) -> URL {
        hooksDir(configDir: configDir).appendingPathComponent(AppIdentity.statusLineScriptName)
    }

    static func previousStatusLineURL(configDir: String) -> URL {
        hooksDir(configDir: configDir).appendingPathComponent(previousStatusLineFileName)
    }

    /// The fail-open command running this account's hook script.
    static func hookCommand(configDir: String, python: String) -> String {
        HookCommands.command(runningScript: hookScriptURL(configDir: configDir).path, python: python)
    }

    /// The fail-open command running this account's status line wrapper.
    static func statusLineCommand(configDir: String, python: String) -> String {
        HookCommands.command(runningScript: statusLineScriptURL(configDir: configDir).path, python: python)
    }

    // MARK: - Guard

    /// Tests run before any `bootstrap`, with the real home folder. Whatever
    /// a test does, the installer never writes into that user's real Claude
    /// folders then: `~`, `~/.claude*`, `~/.config/claude*` of the account
    /// running the process (its password-file home, not `$HOME`).
    static func isProtectedBeforeBootstrap(configDir: String) -> Bool {
        guard !AppIdentity.isFrozen, let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir else { return false }
        let realHome = (String(cString: dir) as NSString).standardizingPath
        let path = (AccountPaths.normalize(configDir) as NSString).standardizingPath
        let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
        return [path, resolved].contains { candidate in
            if candidate == realHome { return true }
            let parent = (candidate as NSString).deletingLastPathComponent
            let name = (candidate as NSString).lastPathComponent
            if parent == realHome, name.hasPrefix(".claude") { return true }
            if parent == realHome + "/.config", name.hasPrefix("claude") { return true }
            return candidate.hasPrefix(realHome + "/.claude") || candidate.hasPrefix(realHome + "/.config/claude")
        }
    }

    /// A folder nothing of ours is installed into (and Claude Code is never
    /// run in): a store Claude Parallel Profiles created (its marker, or
    /// listed in the manifest's `created`), the shared history, or the
    /// windows folder itself. A profile the extension only adopted (in its
    /// `stores`, not `created`) is the user's run folder and is not refused.
    /// While the manifest can't be read (caught mid-write), a standalone
    /// `~/.claude-*` folder is refused until it can. Read-only checks.
    static func isNeverInstallTarget(configDir: String, home: String = AccountPaths.homeDirectory) -> Bool {
        let path = AccountPaths.normalize(configDir)
        if path == ParallelProfiles.sharedStore(home: home) || path == ParallelProfiles.windowsRoot(home: home) { return true }
        if AccountPaths.isInfrastructureDir(path) { return true }
        if FileManager.default.fileExists(atPath: (path as NSString).appendingPathComponent(ParallelProfiles.storeMarkerName)) {
            return true
        }
        // The manifest only speaks for folders of its own home (and the real
        // home is not read before bootstrap).
        let homePath = AccountPaths.normalize(home)
        guard path.hasPrefix(homePath + "/"), !AccountPaths.isRealHomeBeforeBootstrap(homePath) else { return false }
        // ~/.claude and the windows' working copies are never stores.
        if path == AccountRegistry.defaultConfigDir(home: homePath) || ParallelProfiles.isWindowDir(path, home: homePath) {
            return false
        }
        if let manifest = ParallelProfilesManifest.read(home: homePath) {
            return manifest.isCreatedStore(path)
        }
        return ParallelProfilesManifest.isUnreadable(home: homePath)
    }

    private static func writesAllowed(_ configDir: String) -> Bool {
        if DevFlags.installsDisabled {
            logger.notice("Not touching \(configDir, privacy: .public): installs are disabled for this run")
            return false
        }
        if isProtectedBeforeBootstrap(configDir: configDir) {
            logger.error("Not touching \(configDir, privacy: .public): the engine isn't bootstrapped and this is a real Claude folder")
            return false
        }
        return true
    }

    // MARK: - Install

    /// Copy our scripts into the account's hooks dir and register them in its
    /// settings.json. Idempotent: a second run with nothing changed writes nothing.
    @discardableResult
    static func install(configDir: String, configuration: Configuration) -> HookInstallOutcome {
        guard writesAllowed(configDir) else { return .disabled }
        // Last line of defence, whatever the caller thinks the folder is.
        guard !isNeverInstallTarget(configDir: configDir) else {
            logger.error("Refusing to install into \(configDir, privacy: .public): a Claude Parallel Profiles store or the shared history")
            return .notAnInstallTarget
        }
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: AccountPaths.normalize(configDir), isDirectory: &isDirectory), isDirectory.boolValue else {
            return .configDirMissing
        }

        // Don't leave scripts behind for a settings.json we'd refuse anyway.
        // (applyChange checks again right before it writes.)
        if let refusal = preflight(configDir: configDir) {
            return refusal
        }

        let hooksDir = hooksDir(configDir: configDir)
        do {
            try fm.createDirectory(at: hooksDir, withIntermediateDirectories: true)
        } catch {
            return .writeFailed("Couldn't create the hooks folder: \(error.localizedDescription)")
        }

        // Scripts first: settings.json must never point at a script that isn't there.
        if let failure = installScript(configuration.hookScript, to: hookScriptURL(configDir: configDir)) {
            return failure
        }
        var statusLineIntent = StatusLineIntent.unwrap
        if configuration.statusLineIntegration {
            if let failure = installScript(configuration.statusLineScript, to: statusLineScriptURL(configDir: configDir)) {
                // Hooks still work without the wrapper; leave the status line alone.
                logger.error("Status line wrapper not installed in \(configDir, privacy: .public): \(failure.errorMessage ?? "", privacy: .public)")
                statusLineIntent = .leave
            } else {
                statusLineIntent = .wrap(command: statusLineCommand(configDir: configDir, python: configuration.python))
            }
        }

        let command = hookCommand(configDir: configDir, python: configuration.python)
        let outcome = applyChange(configDir: configDir, writtenOutcome: .installed) { context in
            planInstall(
                existingData: context.data,
                hookCommand: command,
                version: configuration.version,
                statusLine: statusLineIntent,
                savedPreviousStatusLine: context.savedPreviousStatusLine,
                backupStatusLine: context.backupStatusLine()
            )
        }

        // With the integration off and our wrapper no longer in settings.json,
        // the wrapper script has nothing left to do.
        if !configuration.statusLineIntegration,
           outcome == .installed || outcome == .alreadyCurrent,
           !readStatus(configDir: configDir).statusLineInstalled {
            try? fm.removeItem(at: statusLineScriptURL(configDir: configDir))
            try? fm.removeItem(at: previousStatusLineURL(configDir: configDir))
        }

        switch outcome {
        case .installed:
            logger.info("Installed hooks into \(configDir, privacy: .public)")
        case .alreadyCurrent:
            logger.debug("Hooks already current in \(configDir, privacy: .public)")
        default:
            logger.error("Hook install in \(configDir, privacy: .public) failed: \(String(describing: outcome), privacy: .public)")
        }
        return outcome
    }

    /// Remove our hooks, restore the previous status line exactly, then delete
    /// our scripts. Other tools' entries are never touched.
    @discardableResult
    static func uninstall(configDir: String) -> HookInstallOutcome {
        guard writesAllowed(configDir) else { return .disabled }
        guard FileManager.default.fileExists(atPath: AccountPaths.normalize(configDir)) else {
            return .configDirMissing
        }

        let outcome = applyChange(configDir: configDir, writtenOutcome: .removed) { context in
            planUninstall(
                existingData: context.data,
                savedPreviousStatusLine: context.savedPreviousStatusLine,
                backupStatusLine: context.backupStatusLine()
            )
        }

        // Only once settings.json no longer references them.
        if outcome == .removed || outcome == .alreadyCurrent {
            let fm = FileManager.default
            try? fm.removeItem(at: hookScriptURL(configDir: configDir))
            try? fm.removeItem(at: statusLineScriptURL(configDir: configDir))
            try? fm.removeItem(at: previousStatusLineURL(configDir: configDir))
            // The folder our scripts went into, when nothing else is in it.
            let hooks = hooksDir(configDir: configDir)
            if (try? fm.destinationOfSymbolicLink(atPath: hooks.path)) == nil,
               (try? fm.contentsOfDirectory(atPath: hooks.path))?.isEmpty == true {
                try? fm.removeItem(at: hooks)
            }
            logger.info("Uninstalled hooks from \(configDir, privacy: .public)")
        }
        return outcome
    }

    /// Remove another notch app's hooks from one account, restoring the
    /// status line its wrapper replaced (Superpowered Vibe Notch's from its
    /// own saved copy). An explicit user action: the installer never removes
    /// another app's hooks on its own. The other app's files are left alone.
    @discardableResult
    static func removeLegacyHooks(configDir: String, kinds: Set<LegacyHookKind>) -> HookInstallOutcome {
        guard writesAllowed(configDir) else { return .disabled }
        guard FileManager.default.fileExists(atPath: AccountPaths.normalize(configDir)) else {
            return .configDirMissing
        }
        let home = AccountPaths.homeDirectory
        let outcome = applyChange(configDir: configDir, writtenOutcome: .removed) { context in
            planLegacyRemoval(
                existingData: context.data,
                kinds: kinds,
                vibeNotchPreviousStatusLine: context.vibeNotchPreviousStatusLine(),
                vibeNotchBackupStatusLine: context.vibeNotchBackupStatusLine(),
                savedPreviousStatusLine: context.savedPreviousStatusLine,
                backupStatusLine: context.backupStatusLine(),
                home: home
            )
        }
        if outcome == .removed {
            let names = kinds.map(\.appName).sorted().joined(separator: ", ")
            logger.info("Removed \(names, privacy: .public) hooks from \(configDir, privacy: .public)")
        }
        return outcome
    }

    // MARK: - Status

    /// Read back what one account's settings.json registers. Read-only; safe
    /// even when installs are disabled.
    static func readStatus(configDir: String) -> AccountHookStatus {
        let fm = FileManager.default
        var status = AccountHookStatus()
        var isDirectory: ObjCBool = false
        status.configDirExists = fm.fileExists(atPath: AccountPaths.normalize(configDir), isDirectory: &isDirectory)
            && isDirectory.boolValue
        guard status.configDirExists else { return status }
        status.newestBackupPath = newestBackup(besides: settingsFile(configDir: configDir))

        let settingsURL = settingsFile(configDir: configDir)
        if case .broken = linkState(settingsURL) {
            status.settingsReadable = false
            return status
        }
        guard fm.fileExists(atPath: settingsURL.path) else { return status }
        guard let data = try? Data(contentsOf: settingsURL) else {
            status.settingsReadable = false
            return status
        }
        guard let document = SettingsDocument(data: data) else {
            status.settingsReadable = false
            return status
        }
        if let hooks = document["hooks"], !hooks.isObject {
            status.settingsReadable = false
        }
        let home = AccountPaths.homeDirectory
        let json = document.value

        // The scripts the entries actually run: normally in this account's
        // hooks dir, but a settings.json shared with another account (a
        // symlink, or a copy) runs that account's copies, which work as well.
        status.hookCommand = firstHookCommand(in: json) { isOurHook($0, home: home) }
        if let command = status.hookCommand {
            status.hooksRegistered = true
            let script = HookCommands.scriptPath(in: command, named: AppIdentity.hookScriptName, home: home)
                ?? hookScriptURL(configDir: configDir).path
            status.hooksInstalled = fm.fileExists(atPath: script)
        }
        let statusLine = json["statusLine"]
        if isOurStatusLine(statusLine, home: home) {
            status.statusLineInstalled = fm.fileExists(atPath: statusLineScriptPath(statusLine, configDir: configDir, home: home))
        }
        status.vibeNotchHooksPresent = firstHookCommand(in: json) { isLegacyHook($0, kind: .vibeNotch, home: home) } != nil
        status.superpoweredVibeNotchHooksPresent =
            firstHookCommand(in: json) { isLegacyHook($0, kind: .superpoweredVibeNotch, home: home) } != nil
            || isLegacyStatusLine(statusLine, kind: .superpoweredVibeNotch, home: home)
        status.vibeIslandHooksPresent =
            firstHookCommand(in: json) { isLegacyHook($0, kind: .vibeIsland, home: home) } != nil
            || isLegacyStatusLine(statusLine, kind: .vibeIsland, home: home)
        return status
    }

    /// The wrapper script an `isOurStatusLine` status line runs; this
    /// account's own copy when the command can't be read.
    private static func statusLineScriptPath(_ statusLine: OrderedJSON?, configDir: String, home: String) -> String {
        let command = statusLine?["command"]?.stringValue ?? ""
        return HookCommands.scriptPath(in: command, named: AppIdentity.statusLineScriptName, home: home)
            ?? statusLineScriptURL(configDir: configDir).path
    }

    /// Where the status line our wrapper chains to is saved: next to the
    /// wrapper the current `statusLine` runs, which is this account's unless
    /// the settings.json came from (or is shared with) another account. That
    /// is the file the running wrapper reads, so it is the one to restore from.
    static func savedStatusLineURL(existing: OrderedJSON?, configDir: String, home: String) -> URL {
        let own = previousStatusLineURL(configDir: configDir)
        guard let statusLine = existing?["statusLine"], isOurStatusLine(statusLine, home: home) else { return own }
        let script = statusLineScriptPath(statusLine, configDir: configDir, home: home)
        return URL(fileURLWithPath: script).deletingLastPathComponent().appendingPathComponent(previousStatusLineFileName)
    }

    /// Where Superpowered Vibe Notch saved the status line its wrapper
    /// chains to: beside the wrapper the current `statusLine` runs.
    static func vibeNotchSavedStatusLineURL(existing: OrderedJSON?, configDir: String, home: String) -> URL {
        let own = hooksDir(configDir: configDir).appendingPathComponent(AppIdentity.vibeNotchPreviousStatusLineFileName)
        guard let command = existing?["statusLine"]?["command"]?.stringValue,
              let script = HookCommands.scriptPath(in: command, named: AppIdentity.vibeNotchStatusLineScriptName, home: home)
        else { return own }
        return URL(fileURLWithPath: script).deletingLastPathComponent()
            .appendingPathComponent(AppIdentity.vibeNotchPreviousStatusLineFileName)
    }

    // MARK: - Planning (pure)

    /// What to do with the account's `statusLine` during an install.
    enum StatusLineIntent: Equatable, Sendable {
        /// Make ours the status line, saving whatever was there to chain to.
        case wrap(command: String)
        /// If ours is the status line, put the saved one back.
        case unwrap
        /// Don't touch the status line.
        case leave
    }

    /// A change to the saved previous-status-line file that goes with a plan.
    enum PreviousStatusLineChange: Equatable, Sendable {
        /// Write these bytes (the statusLine object we're taking over) before settings.json.
        case save(Data)
        /// Write these bytes where the saved file was lost (deleted with the
        /// hooks folder, say): what the backups say ours chains to. Applies
        /// even when settings.json doesn't change, or the running wrapper
        /// would keep chaining nothing.
        case recover(Data)
        /// Write the empty object: ours chains to nothing. Unlike a missing
        /// file (a lost one, which the backups stand in for), this says the
        /// user had no status line, so an older one in a backup never
        /// comes back. Applies even when settings.json doesn't change.
        case chainNothing
        /// Delete the file once settings.json is written (the status line was
        /// given back, so there is nothing left to chain).
        case remove
    }

    /// What `chainNothing` writes.
    static let chainsNothing = Data("{}\n".utf8)

    /// Why a plan refuses to touch settings.json.
    enum Refusal: Equatable, Sendable {
        /// Not a JSON object (truncated, an array, binary garbage, …).
        case unreadable
        /// `hooks` exists but isn't an object.
        case hooksNotAnObject

        var outcome: HookInstallOutcome {
            switch self {
            case .unreadable: return .settingsUnreadable
            case .hooksNotAnObject: return .settingsHooksMalformed
            }
        }
    }

    /// What a settings.json change decided, given the bytes currently on disk.
    /// Kept separate from the file IO so it can be exercised without a real
    /// config dir.
    enum SettingsPlan: Equatable, Sendable {
        case write(Data)
        case alreadyCurrent
        case refuse(Refusal)
    }

    struct InstallPlan: Equatable, Sendable {
        var settings: SettingsPlan
        var previousStatusLine: PreviousStatusLineChange?

        init(settings: SettingsPlan, previousStatusLine: PreviousStatusLineChange? = nil) {
            self.settings = settings
            self.previousStatusLine = previousStatusLine
        }
    }

    /// Build the settings.json we want for an install, or refuse.
    ///
    /// - Parameters:
    ///   - existingData: current file contents, or nil when the file does not
    ///     exist yet (the only case where starting from `{}` is correct).
    ///   - savedPreviousStatusLine: the statusLine object saved by an earlier
    ///     install, if any (only consulted while ours is the status line).
    ///   - backupStatusLine: the newest non-wrapper status line in our
    ///     backups, for when the saved one is gone.
    static func planInstall(
        existingData: Data?,
        hookCommand: String,
        version: ClaudeCodeVersion?,
        statusLine: StatusLineIntent,
        savedPreviousStatusLine: OrderedJSON?,
        backupStatusLine: @autoclosure () -> OrderedJSON? = nil,
        home: String = AccountPaths.homeDirectory
    ) -> InstallPlan {
        guard var document = SettingsDocument(data: existingData) else {
            return InstallPlan(settings: .refuse(.unreadable))
        }
        let original = document.value
        guard var hooks = hooksObject(in: document) else {
            return InstallPlan(settings: .refuse(.hooksNotAnObject))
        }

        // Strip our own existing hooks from ALL event types first — even events
        // we no longer register, so stale keys from an older Claude Code never
        // linger (upstream issue #85), and entries in an older command form
        // are replaced by the fail-open one. Only entries running our script
        // are touched; other tools' entries are left exactly as they are.
        let before = hooks
        hooks = removingHooks(from: hooks) { isOurHook($0, home: home) }

        for (event, config) in hookEventConfigs(command: hookCommand, version: version) {
            // An event holding something other than a list of matcher groups is
            // malformed; leave it for the user rather than overwrite it.
            if let value = hooks[event], value.items == nil { continue }
            hooks.set(event, .array((hooks[event]?.items ?? []) + config))
        }
        replaceHooks(before, with: hooks, in: &document)

        let change = applyStatusLineIntent(
            statusLine, to: &document, savedPrevious: savedPreviousStatusLine, backup: backupStatusLine, home: home)
        return finish(document, original: original, existingData: existingData, change: change)
    }

    /// Build the settings.json without our hooks and with the previous status
    /// line restored.
    static func planUninstall(
        existingData: Data?,
        savedPreviousStatusLine: OrderedJSON?,
        backupStatusLine: @autoclosure () -> OrderedJSON? = nil,
        home: String = AccountPaths.homeDirectory
    ) -> InstallPlan {
        guard existingData != nil else { return InstallPlan(settings: .alreadyCurrent) }
        guard var document = SettingsDocument(data: existingData) else {
            return InstallPlan(settings: .refuse(.unreadable))
        }
        let original = document.value
        guard let hooks = hooksObject(in: document) else {
            return InstallPlan(settings: .refuse(.hooksNotAnObject))
        }
        replaceHooks(hooks, with: removingHooks(from: hooks) { isOurHook($0, home: home) }, in: &document)

        let change = applyStatusLineIntent(
            .unwrap, to: &document, savedPrevious: savedPreviousStatusLine, backup: backupStatusLine, home: home)
        return finish(document, original: original, existingData: existingData, change: change)
    }

    /// Build the settings.json without another app's entries. A status line
    /// that app's wrapper holds goes back to what it wrapped: Superpowered
    /// Vibe Notch's from its saved copy (else its newest backup, else none);
    /// when that was our own wrapper (it ran again after we took over, and
    /// wrapped ours), to what ours chains. Vibe Island's is removed (it
    /// prints nothing). If ours chains to Vibe Island's, the saved copy is
    /// dropped so ours chains to nothing.
    static func planLegacyRemoval(
        existingData: Data?,
        kinds: Set<LegacyHookKind>,
        vibeNotchPreviousStatusLine: OrderedJSON? = nil,
        vibeNotchBackupStatusLine: @autoclosure () -> OrderedJSON? = nil,
        savedPreviousStatusLine: OrderedJSON? = nil,
        backupStatusLine: @autoclosure () -> OrderedJSON? = nil,
        home: String = AccountPaths.homeDirectory
    ) -> InstallPlan {
        guard existingData != nil else { return InstallPlan(settings: .alreadyCurrent) }
        guard var document = SettingsDocument(data: existingData) else {
            return InstallPlan(settings: .refuse(.unreadable))
        }
        let original = document.value
        guard let hooks = hooksObject(in: document) else {
            return InstallPlan(settings: .refuse(.hooksNotAnObject))
        }
        let cleaned = removingHooks(from: hooks) { command in
            kinds.contains { isLegacyHook(command, kind: $0, home: home) }
        }
        replaceHooks(hooks, with: cleaned, in: &document)

        var change: PreviousStatusLineChange?
        let statusLine = document["statusLine"]
        if kinds.contains(.superpoweredVibeNotch), isLegacyStatusLine(statusLine, kind: .superpoweredVibeNotch, home: home) {
            func replaced(_ value: OrderedJSON?) -> OrderedJSON? {
                guard let value, !(value.members ?? []).isEmpty,
                      !isLegacyStatusLine(value, kind: .superpoweredVibeNotch, home: home) else { return nil }
                return value
            }
            var previous = replaced(vibeNotchPreviousStatusLine) ?? replaced(vibeNotchBackupStatusLine())
            if let wrapped = previous, isOurStatusLine(wrapped, home: home) {
                previous = chainTarget(saved: savedPreviousStatusLine, backup: backupStatusLine, home: home)
            }
            document.set("statusLine", previous.map { restored(from: $0, wrapper: statusLine) })
        }
        if kinds.contains(.vibeIsland) {
            if isLegacyStatusLine(document["statusLine"], kind: .vibeIsland, home: home) {
                document.set("statusLine", nil)
            } else if isOurStatusLine(document["statusLine"], home: home),
                      isLegacyStatusLine(savedPreviousStatusLine, kind: .vibeIsland, home: home) {
                change = .chainNothing
            }
        }
        return finish(document, original: original, existingData: existingData, change: change)
    }

    /// Put `new` in place of `old` as the document's `hooks`, only if it
    /// differs (so the value keeps its bytes when nothing changed), and
    /// without an empty object the file didn't have.
    private static func replaceHooks(_ old: OrderedJSON, with new: OrderedJSON, in document: inout SettingsDocument) {
        guard !new.isEquivalent(to: old) else { return }
        document.set("hooks", new.members?.isEmpty ?? true ? nil : new)
    }

    /// `hooks` as an object: `{}` when absent, nil when it is something else.
    private static func hooksObject(in document: SettingsDocument) -> OrderedJSON? {
        guard let hooks = document["hooks"] else { return .object([]) }
        return hooks.isObject ? hooks : nil
    }

    private static func finish(
        _ document: SettingsDocument,
        original: OrderedJSON,
        existingData: Data?,
        change: PreviousStatusLineChange?
    ) -> InstallPlan {
        // Compare as JSON, not byte for byte: Claude Code and other tools
        // rewrite settings.json in their own formatting, and reformatting the
        // user's file on every launch would be a pointless write (and backup).
        if existingData != nil, document.value.isEquivalent(to: original) {
            // A `.save` without a statusLine change only re-saves what the
            // saved file already says, so nothing is lost by dropping it
            // here; `.recover`, `.remove` and `.chainNothing` still apply.
            let pending: PreviousStatusLineChange?
            switch change {
            case .save, nil: pending = nil
            case .recover, .remove, .chainNothing: pending = change
            }
            return InstallPlan(settings: .alreadyCurrent, previousStatusLine: pending)
        }
        return InstallPlan(settings: .write(document.data()), previousStatusLine: change)
    }

    // MARK: - Status Line Planning

    /// Apply a status line intent to the document. Returns the matching
    /// change to the saved previous-status-line file.
    private static func applyStatusLineIntent(
        _ intent: StatusLineIntent,
        to document: inout SettingsDocument,
        savedPrevious: OrderedJSON?,
        backup: () -> OrderedJSON?,
        home: String
    ) -> PreviousStatusLineChange? {
        let current = document["statusLine"]
        let currentIsOurs = isOurStatusLine(current, home: home)
        var chained: OrderedJSON? { chainTarget(saved: savedPrevious, backup: backup, home: home) }

        switch intent {
        case .leave:
            return nil

        case .unwrap:
            guard currentIsOurs else { return nil }
            document.set("statusLine", chained.map { restored(from: $0, wrapper: current) })
            return .remove

        case .wrap(let command):
            // Something we don't understand (statusLine is always an object),
            // or another notch app's wrapper we must not stack on: leave it.
            if let current, !currentIsOurs,
               !current.isObject || isLegacyStatusLine(current, kind: .superpoweredVibeNotch, home: home) {
                return nil
            }

            if let current, currentIsOurs {
                // Keep what the user set on the wrapper entry (padding, …);
                // only the command is ours to update. Keep chaining to what
                // the running wrapper chains to. That may be saved next to
                // another account's wrapper (a settings.json copied from it),
                // so save it beside ours as well; it is only written when
                // settings.json changes too.
                // (Set only when the command changes, so a hooks-only write
                // leaves the entry's bytes as they are.)
                if current["command"]?.stringValue != command {
                    var ours = current
                    ours.set("command", .string(command))
                    document.set("statusLine", ours)
                }
                guard let chain = chained else { return nil }
                let bytes = Data((chain.serialized() + "\n").utf8)
                // A lost saved copy is put back now, not at the next change
                // to settings.json: until then the wrapper chains nothing.
                return savedPrevious == nil ? .recover(bytes) : .save(bytes)
            }

            guard let current else {
                // No status line before us: nothing to chain to, and any stale
                // saved file from an earlier install must not be run (nor an
                // older status line from a backup brought back).
                document.set("statusLine", .object(["type": .string("command"), "command": .string(command)]))
                return .chainNothing
            }
            // Wrap: the same entry (every key, in its order) running ours.
            var ours = current
            ours.set("type", .string("command"))
            ours.set("command", .string(command))
            document.set("statusLine", ours)
            return .save(Data((current.serialized() + "\n").utf8))
        }
    }

    /// What our wrapper chains to: the saved copy when there is one (the
    /// empty object says "nothing"), else what the backups say it was (a
    /// lost previous.json must not cost the user their status line). The
    /// backups are only read when the saved copy is missing.
    static func chainTarget(saved: OrderedJSON?, backup: () -> OrderedJSON?, home: String) -> OrderedJSON? {
        func chainable(_ value: OrderedJSON?) -> OrderedJSON? {
            guard let value, let members = value.members, !members.isEmpty, !isAnyWrapper(value, home: home) else { return nil }
            return value
        }
        return saved != nil ? chainable(saved) : chainable(backup())
    }

    /// The status line to put back: the saved one, with any setting the user
    /// changed on the wrapper entry while it was wrapped (keys both have),
    /// and any they added. A `padding: 0` only the wrapper has is what older
    /// installers added, and is the default anyway, so it isn't carried over.
    static func restored(from previous: OrderedJSON, wrapper: OrderedJSON?) -> OrderedJSON {
        guard var restored = previous.isObject ? previous : nil else { return previous }
        for member in wrapper?.members ?? [] where member.key != "type" && member.key != "command" {
            if previous[member.key] != nil {
                restored.set(member.key, member.value)
            } else if member.key == "padding", OrderedJSON.equivalent(member.value, .int(0)) {
                continue
            } else {
                restored.set(member.key, member.value)
            }
        }
        return restored
    }

    static func isOurStatusLine(_ value: OrderedJSON?, home: String = AccountPaths.homeDirectory) -> Bool {
        guard let command = value?["command"]?.stringValue else { return false }
        return HookCommands.runs(command, script: AppIdentity.statusLineScriptName, home: home)
    }

    static func isLegacyStatusLine(_ value: OrderedJSON?, kind: LegacyHookKind, home: String = AccountPaths.homeDirectory) -> Bool {
        guard let command = value?["command"]?.stringValue else { return false }
        switch kind {
        case .vibeNotch: return false
        case .superpoweredVibeNotch:
            return HookCommands.runs(command, script: AppIdentity.vibeNotchStatusLineScriptName, home: home)
        case .vibeIsland:
            return HookCommands.runsVibeIsland(command) || command.contains("/.vibe-island/")
        }
    }

    /// Ours or Superpowered Vibe Notch's wrapper: never something to chain to.
    private static func isAnyWrapper(_ value: OrderedJSON, home: String) -> Bool {
        isOurStatusLine(value, home: home) || isLegacyStatusLine(value, kind: .superpoweredVibeNotch, home: home)
    }

    // MARK: - Hook Events

    /// The ordered (event, config) pairs to register, filtered to the events
    /// the given Claude Code version knows about. Before 2.1.101 an unknown
    /// hook event made Claude Code ignore the whole settings file, so
    /// without a version we stick to the baseline, and the caller passes the
    /// lowest version known to use the account (#85).
    static func hookEventConfigs(command: String, version: ClaudeCodeVersion?) -> [(String, [OrderedJSON])] {
        let hookEntry: OrderedJSON = .array([.object(["type": .string("command"), "command": .string(command)])])
        let hookEntryWithTimeout: OrderedJSON = .array([
            .object(["type": .string("command"), "command": .string(command), "timeout": .int(86400)]),
        ])
        let withMatcher: [OrderedJSON] = [.object(["matcher": .string("*"), "hooks": hookEntry])]
        let withMatcherAndTimeout: [OrderedJSON] = [.object(["matcher": .string("*"), "hooks": hookEntryWithTimeout])]
        let withoutMatcher: [OrderedJSON] = [.object(["hooks": hookEntry])]
        let compactConfig: [OrderedJSON] = [
            .object(["matcher": .string("auto"), "hooks": hookEntry]),
            .object(["matcher": .string("manual"), "hooks": hookEntry]),
        ]

        // Baseline — present in every Claude Code version that supports hooks.
        var events: [(String, [OrderedJSON])] = [
            ("UserPromptSubmit", withoutMatcher),
            ("PreToolUse", withMatcher),
            ("PostToolUse", withMatcher),
            ("PermissionRequest", withMatcherAndTimeout),
            ("Notification", withMatcher),
            ("Stop", withoutMatcher),
            ("SubagentStop", withoutMatcher),
            ("SessionStart", withoutMatcher),
            ("SessionEnd", withoutMatcher),
            ("PreCompact", compactConfig),
        ]

        guard let version else { return events }

        // Versions below are from the official Claude Code changelog.
        // v2.0.x — PostToolUseFailure shipped alongside the PostToolUse redesign.
        if version >= ClaudeCodeVersion(major: 2, minor: 0, patch: 0) {
            events.append(("PostToolUseFailure", withMatcher))
        }
        // v2.0.43 — SubagentStart, pairs with SubagentStop.
        if version >= ClaudeCodeVersion(major: 2, minor: 0, patch: 43) {
            events.append(("SubagentStart", withoutMatcher))
        }
        // v2.1.33 — TaskCompleted ("Added TeammateIdle and TaskCompleted hook events").
        if version >= ClaudeCodeVersion(major: 2, minor: 1, patch: 33) {
            events.append(("TaskCompleted", withoutMatcher))
        }
        // v2.1.76 — PostCompact, pairs with PreCompact.
        if version >= ClaudeCodeVersion(major: 2, minor: 1, patch: 76) {
            events.append(("PostCompact", compactConfig))
        }
        // v2.1.78 — StopFailure on API errors (rate limit, auth, billing).
        if version >= ClaudeCodeVersion(major: 2, minor: 1, patch: 78) {
            events.append(("StopFailure", withoutMatcher))
        }
        // v2.1.84 — TaskCreated ("fires when a task is created via TaskCreate").
        if version >= ClaudeCodeVersion(major: 2, minor: 1, patch: 84) {
            events.append(("TaskCreated", withoutMatcher))
        }
        // v2.1.89 — PermissionDenied for auto-mode classifier denials.
        if version >= ClaudeCodeVersion(major: 2, minor: 1, patch: 89) {
            events.append(("PermissionDenied", withMatcher))
        }
        return events
    }

    // MARK: - Hook Entry Matching

    /// Whether a hook command runs this app's script (exactly: see
    /// `HookCommands.scriptPath`). Other apps' entries never match, so they
    /// can keep their hooks in the same settings.json.
    static func isOurHook(_ command: String, home: String = AccountPaths.homeDirectory) -> Bool {
        HookCommands.runs(command, script: AppIdentity.hookScriptName, home: home)
    }

    /// Whether a hook command is one of another notch app's.
    static func isLegacyHook(_ command: String, kind: LegacyHookKind, home: String = AccountPaths.homeDirectory) -> Bool {
        switch kind {
        case .vibeNotch:
            return HookCommands.runs(command, script: AppIdentity.legacyHookScriptName, home: home)
        case .superpoweredVibeNotch:
            return HookCommands.runs(command, script: AppIdentity.vibeNotchHookScriptName, home: home)
        case .vibeIsland:
            return HookCommands.runsVibeIsland(command)
        }
    }

    /// Remove hook entries whose command matches from every event. Matcher
    /// groups left with no hooks are dropped, and so are events left with no
    /// groups. Anything of a shape we don't recognise is kept as it is.
    private static func removingHooks(from hooks: OrderedJSON, where matches: (String) -> Bool) -> OrderedJSON {
        var cleaned: [OrderedJSON.Member] = []
        for member in hooks.members ?? [] {
            guard let groups = member.value.items else {
                cleaned.append(member)
                continue
            }
            var remaining: [OrderedJSON] = []
            for group in groups {
                guard let entries = group["hooks"]?.items else {
                    remaining.append(group)
                    continue
                }
                let kept = entries.filter { entry in
                    guard let command = entry["command"]?.stringValue else { return true }
                    return !matches(command)
                }
                if kept.count == entries.count {
                    remaining.append(group)
                } else if !kept.isEmpty {
                    var updated = group
                    updated.set("hooks", .array(kept))
                    remaining.append(updated)
                }
            }
            if !remaining.isEmpty {
                cleaned.append(OrderedJSON.Member(key: member.key, value: .array(remaining)))
            }
        }
        return .object(cleaned)
    }

    /// First hook command anywhere in a parsed settings.json that satisfies
    /// `matches`. Tolerant of any shape at any key.
    static func firstHookCommand(in json: OrderedJSON, matching matches: (String) -> Bool) -> String? {
        for member in json["hooks"]?.members ?? [] {
            for group in member.value.items ?? [] {
                for entry in group["hooks"]?.items ?? [] {
                    if let command = entry["command"]?.stringValue, matches(command) {
                        return command
                    }
                }
            }
        }
        return nil
    }

    // MARK: - Python

    nonisolated(unsafe) private static var cachedPython: String?
    private static let pythonLock = NSLock()

    /// The interpreter for the scripts, resolved once per launch: the
    /// selected developer tools' `python3` (Xcode or the Command Line
    /// Tools; never the `/usr/bin` shim, which costs a few ms per call and
    /// can offer to install the tools), else Homebrew's, else `python3`
    /// looked up at run time. Commands fall back to `python3` on their own
    /// if the path disappears.
    static func detectPython() -> String {
        pythonLock.lock()
        defer { pythonLock.unlock() }
        if let cachedPython { return cachedPython }
        let developerDir = ProcessRunner.run(executable: "/usr/bin/xcode-select", arguments: ["-p"], timeout: 5)
            .flatMap { $0.exitCode == 0 ? $0.stdout.trimmingCharacters(in: .whitespacesAndNewlines) : nil }
        let python = resolvePython(developerDir: developerDir)
        cachedPython = python
        return python
    }

    /// Pure part of `detectPython`, for tests.
    static func resolvePython(
        developerDir: String?,
        isExecutable: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> String {
        var candidates: [String] = []
        if let developerDir, developerDir.hasPrefix("/") {
            candidates.append((developerDir as NSString).appendingPathComponent("usr/bin/python3"))
        }
        candidates += [
            "/Library/Developer/CommandLineTools/usr/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
        ]
        return candidates.first(where: isExecutable) ?? "python3"
    }

    // MARK: - File IO

    /// Copy a bundled script into place when its bytes differ. Returns a
    /// failure outcome, or nil on success.
    private static func installScript(_ contents: String?, to destination: URL) -> HookInstallOutcome? {
        let fm = FileManager.default
        guard let contents else {
            // No script given: keep one an earlier install left, else fail.
            if fm.fileExists(atPath: destination.path) { return nil }
            return .writeFailed("\(destination.lastPathComponent) is not available")
        }
        let bytes = Data(contents.utf8)
        if let existing = try? Data(contentsOf: destination), existing == bytes,
           fm.isExecutableFile(atPath: destination.path) {
            return nil
        }
        do {
            try bytes.write(to: destination, options: .atomic)
            try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: destination.path)
            return nil
        } catch {
            return .writeFailed("Couldn't write \(destination.lastPathComponent): \(error.localizedDescription)")
        }
    }

    /// Refuse early (before writing scripts) for a settings.json we won't touch.
    private static func preflight(configDir: String) -> HookInstallOutcome? {
        let settingsURL = settingsFile(configDir: configDir)
        if case .broken(let target) = linkState(settingsURL) {
            return .settingsLinkBroken(target)
        }
        guard FileManager.default.fileExists(atPath: settingsURL.path) else { return nil }
        guard let data = try? Data(contentsOf: settingsURL), let document = SettingsDocument(data: data) else {
            logger.error("settings.json in \(configDir, privacy: .public) is unreadable — not installing")
            return .settingsUnreadable
        }
        if hooksObject(in: document) == nil { return .settingsHooksMalformed }
        return nil
    }

    /// What a planner gets: the bytes on disk and the saved status lines,
    /// the backup scans run only if asked for.
    struct PlanContext {
        let configDir: String
        let data: Data?
        let parsed: OrderedJSON?
        let savedPreviousStatusLine: OrderedJSON?
        let home: String

        func backupStatusLine() -> OrderedJSON? {
            HookInstaller.newestStatusLine(inBackupsBeside: HookInstaller.settingsFile(configDir: configDir),
                                           prefixes: [HookInstaller.backupPrefix, HookInstaller.originalBackupName], home: home)
        }

        func vibeNotchPreviousStatusLine() -> OrderedJSON? {
            HookInstaller.readSavedStatusLine(
                at: HookInstaller.vibeNotchSavedStatusLineURL(existing: parsed, configDir: configDir, home: home))
        }

        /// Its backups were taken before its own writes: a state with its
        /// wrapper is skipped, but ours is what it replaced (then followed).
        func vibeNotchBackupStatusLine() -> OrderedJSON? {
            let home = home
            return HookInstaller.newestStatusLine(
                inBackupsBeside: HookInstaller.settingsFile(configDir: configDir),
                prefixes: [AppIdentity.vibeNotchBackupPrefix], home: home,
                isWrapper: { HookInstaller.isLegacyStatusLine($0, kind: .superpoweredVibeNotch, home: home) })
        }
    }

    /// Read settings.json (and the saved status line), plan, and carry the plan
    /// out. All other IO (saved status line, backup, staging the new file)
    /// happens first; then the file is checked once more (inode, modification
    /// time, size and bytes) and the staged file renamed over it. If Claude
    /// Code saved it in between, we plan again from its version.
    /// (`beforeCommit` runs right before that last check; tests use it to
    /// play Claude Code saving at the worst moment.)
    static func applyChange(
        configDir: String,
        writtenOutcome: HookInstallOutcome,
        beforeCommit: (() -> Void)? = nil,
        planner: (PlanContext) -> InstallPlan
    ) -> HookInstallOutcome {
        let fm = FileManager.default
        let settingsURL = settingsFile(configDir: configDir)
        let previousURL = previousStatusLineURL(configDir: configDir)
        let home = AccountPaths.homeDirectory

        for _ in 0..<5 {
            // A symlinked settings.json is written through to its target.
            let target: URL
            switch linkState(settingsURL) {
            case .broken(let destination):
                logger.error("settings.json in \(configDir, privacy: .public) links to a missing \(destination, privacy: .public) — leaving it alone")
                return .settingsLinkBroken(destination)
            case .link(let destination):
                target = destination
            case .notALink:
                target = settingsURL
            }

            let before = FileState.read(target)
            // Present but unreadable (permissions, or a read that raced a save
            // in progress). Refuse for the same reason a parse failure refuses:
            // we have no idea what we would be replacing.
            if before.exists && before.data == nil {
                logger.error("settings.json in \(configDir, privacy: .public) exists but could not be read — leaving it alone")
                return .settingsUnreadable
            }

            let parsed = before.data.flatMap { SettingsDocument(data: $0)?.value }
            let context = PlanContext(
                configDir: configDir,
                data: before.data,
                parsed: parsed,
                savedPreviousStatusLine: readSavedStatusLine(at: savedStatusLineURL(existing: parsed, configDir: configDir, home: home)),
                home: home
            )
            let plan = planner(context)

            switch plan.settings {
            case .refuse(let refusal):
                logger.error("settings.json in \(configDir, privacy: .public) refused: \(String(describing: refusal), privacy: .public)")
                return refusal.outcome

            case .alreadyCurrent:
                switch plan.previousStatusLine {
                case .remove:
                    try? fm.removeItem(at: previousURL)
                case .chainNothing:
                    if (try? Data(contentsOf: previousURL)) != chainsNothing {
                        try? writeOwnerOnly(chainsNothing, to: previousURL)
                    }
                case .recover(let bytes):
                    recoverSavedStatusLine(bytes, at: previousURL, configDir: configDir)
                case .save, nil:
                    break
                }
                return .alreadyCurrent

            case .write(let newData):
                // Save the status line we're taking over BEFORE settings.json
                // stops pointing at it; if that fails, don't take it over.
                switch plan.previousStatusLine {
                case .save(let bytes):
                    do {
                        try writeOwnerOnly(bytes, to: previousURL)
                    } catch {
                        return .writeFailed("Couldn't save the current status line: \(error.localizedDescription)")
                    }
                case .chainNothing:
                    // Only a guard against an older status line coming back;
                    // without it a lost file falls back to the backups.
                    try? writeOwnerOnly(chainsNothing, to: previousURL)
                case .recover(let bytes):
                    // Ours is the status line either way; the chain is a bonus.
                    recoverSavedStatusLine(bytes, at: previousURL, configDir: configDir)
                case .remove, nil:
                    break
                }
                if let existing = before.data, !SettingsDocument.isBlank(existing) {
                    backUpSettings(existing, settingsURL: settingsURL)
                }

                let staged: URL
                do {
                    staged = try stage(newData, besides: target, permissions: before.permissions ?? 0o644)
                } catch {
                    logger.error("Failed to stage settings.json in \(configDir, privacy: .public): \(error.localizedDescription, privacy: .public)")
                    return .writeFailed("Couldn't write settings.json: \(error.localizedDescription)")
                }

                // The last look, then the rename: a save by Claude Code can
                // only slip into the microseconds between the two.
                beforeCommit?()
                guard FileState.read(target) == before else {
                    try? fm.removeItem(at: staged)
                    logger.info("settings.json in \(configDir, privacy: .public) changed while planning — planning again")
                    continue
                }
                guard rename(staged.path, target.path) == 0 else {
                    let reason = String(cString: strerror(errno))
                    try? fm.removeItem(at: staged)
                    return .writeFailed("Couldn't write settings.json: \(reason)")
                }

                if plan.previousStatusLine == .remove {
                    try? fm.removeItem(at: previousURL)
                }
                return writtenOutcome
            }
        }
        return .writeFailed("settings.json kept changing while we tried to update it")
    }

    /// Put back a lost saved status line (see `PreviousStatusLineChange.recover`).
    private static func recoverSavedStatusLine(_ bytes: Data, at url: URL, configDir: String) {
        guard (try? Data(contentsOf: url)) != bytes else { return }
        do {
            try writeOwnerOnly(bytes, to: url)
            logger.notice("Restored the saved status line in \(configDir, privacy: .public) from a backup")
        } catch {
            logger.error("Couldn't restore the saved status line in \(configDir, privacy: .public): \(error.localizedDescription, privacy: .public)")
        }
    }

    /// The statusLine object saved by an earlier install, if any.
    static func readSavedStatusLine(at url: URL) -> OrderedJSON? {
        guard let data = try? Data(contentsOf: url), let value = try? OrderedJSON.parse(data), value.isObject else { return nil }
        return value
    }

    /// Whether settings.json is a symlink, and where it leads.
    enum LinkState: Equatable {
        case notALink
        case link(URL)
        /// A symlink whose target doesn't exist.
        case broken(String)
    }

    static func linkState(_ url: URL) -> LinkState {
        let fm = FileManager.default
        guard let destination = try? fm.destinationOfSymbolicLink(atPath: url.path) else { return .notALink }
        let resolved = url.resolvingSymlinksInPath()
        guard resolved.path != url.path, fm.fileExists(atPath: resolved.path) else {
            let absolute = destination.hasPrefix("/")
                ? destination
                : url.deletingLastPathComponent().appendingPathComponent(destination).standardizedFileURL.path
            return .broken(absolute)
        }
        return .link(resolved)
    }

    /// Identity and contents of a file at one moment.
    struct FileState: Equatable {
        var exists = false
        var device: Int32 = 0
        var inode: UInt64 = 0
        var modified = timespec()
        var size: Int64 = 0
        var permissions: Int?
        var data: Data?

        static func read(_ url: URL) -> FileState {
            var state = FileState()
            var info = stat()
            guard stat(url.path, &info) == 0 else { return state }
            state.exists = true
            state.device = info.st_dev
            state.inode = info.st_ino
            state.modified = info.st_mtimespec
            state.size = info.st_size
            state.permissions = Int(info.st_mode & 0o7777)
            state.data = try? Data(contentsOf: url)
            return state
        }

        static func == (lhs: FileState, rhs: FileState) -> Bool {
            lhs.exists == rhs.exists && lhs.device == rhs.device && lhs.inode == rhs.inode
                && lhs.modified.tv_sec == rhs.modified.tv_sec && lhs.modified.tv_nsec == rhs.modified.tv_nsec
                && lhs.size == rhs.size && lhs.data == rhs.data
        }
    }

    /// Write `data` to a new temporary file in `target`'s folder (so the
    /// rename is atomic) with `permissions`.
    private static func stage(_ data: Data, besides target: URL, permissions: Int) throws -> URL {
        let staged = target.deletingLastPathComponent()
            .appendingPathComponent(".\(target.lastPathComponent).spcn-\(UUID().uuidString.prefix(8)).tmp")
        let descriptor = open(staged.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, mode_t(permissions & 0o777))
        guard descriptor >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        let handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        do {
            try handle.write(contentsOf: data)
            try handle.synchronize()
            try handle.close()
            // The umask may have narrowed the mode; put it back exactly.
            _ = chmod(staged.path, mode_t(permissions & 0o7777))
        } catch {
            try? FileManager.default.removeItem(at: staged)
            throw error
        }
        return staged
    }

    /// A new file readable only by the user (backups and saved status lines
    /// can hold what settings.json holds, `env` secrets included): created
    /// 0600, never readable by others even for a moment, then renamed into place.
    private static func writeOwnerOnly(_ data: Data, to url: URL) throws {
        let staged = try stage(data, besides: url, permissions: 0o600)
        guard rename(staged.path, url.path) == 0 else {
            let code = errno
            try? FileManager.default.removeItem(at: staged)
            throw POSIXError(POSIXErrorCode(rawValue: code) ?? .EIO)
        }
    }

    // MARK: - Backups

    static let backupPrefix = "settings.json.superpowered-codenotch-"
    static let backupSuffix = ".bak"
    /// The copy made before this app first changed the file; never pruned.
    static let originalBackupName = "settings.json.superpowered-codenotch.original.bak"

    /// How many of our own timestamped backups to keep beside each settings.json.
    static let maxBackups = 5

    /// Copy settings.json aside before a write, owner-only. Written from the
    /// bytes we already read rather than re-reading the file, so the backup
    /// is exactly what the write is about to replace. Skipped when the newest
    /// backup already holds these bytes. The first time, the bytes are also
    /// kept as the original, which rotation never removes.
    private static func backUpSettings(_ data: Data, settingsURL: URL) {
        let directory = settingsURL.deletingLastPathComponent()
        let fm = FileManager.default
        let originalURL = directory.appendingPathComponent(originalBackupName)
        if !fm.fileExists(atPath: originalURL.path) {
            try? writeOwnerOnly(data, to: originalURL)
        }

        let existing = ourBackups(in: directory)
        if let newest = existing.last,
           let newestData = try? Data(contentsOf: directory.appendingPathComponent(newest)),
           newestData == data {
            return
        }

        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMdd-HHmmss-SSS"
        let name = backupPrefix + formatter.string(from: Date()) + backupSuffix
        let backupURL = directory.appendingPathComponent(name)

        do {
            try writeOwnerOnly(data, to: backupURL)
            logger.info("Backed up \(settingsURL.path, privacy: .public) to \(name, privacy: .public)")
        } catch {
            logger.error("Failed to back up settings.json: \(error.localizedDescription, privacy: .public)")
            return
        }

        pruneBackups(in: directory)
    }

    /// Our timestamped backups in `directory`, oldest first (the timestamp
    /// format sorts lexicographically).
    private static func ourBackups(in directory: URL) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? [])
            .filter { $0.hasPrefix(backupPrefix) && $0.hasSuffix(backupSuffix) }
            .sorted()
    }

    /// Trim our own backups to the newest `maxBackups`. Only ever removes files
    /// matching both our prefix and suffix — nothing else in the config dir is
    /// a candidate, and the original isn't one.
    private static func pruneBackups(in directory: URL) {
        let ours = ourBackups(in: directory)
        guard ours.count > maxBackups else { return }
        for name in ours.dropLast(maxBackups) {
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
        }
    }

    /// The newest backup of ours beside `settingsURL`, if any.
    static func newestBackup(besides settingsURL: URL) -> String? {
        let directory = settingsURL.deletingLastPathComponent()
        return ourBackups(in: directory).last.map { directory.appendingPathComponent($0).path }
    }

    /// The status line the user had before a notch app's wrapper replaced
    /// it, from the backups starting with one of `prefixes`: the newest
    /// backup (by name) that wasn't taken while a wrapper was the status line
    /// says what it was. Nil when that backup has none (the user had no
    /// status line then; an older backup's may be one they removed since).
    /// For restoring when the saved copy is gone. `isWrapper` says which
    /// status lines are the wrapper in question (default: ours or Superpowered
    /// Vibe Notch's).
    static func newestStatusLine(
        inBackupsBeside settingsURL: URL,
        prefixes: [String],
        home: String,
        isWrapper: ((OrderedJSON) -> Bool)? = nil
    ) -> OrderedJSON? {
        let isWrapper = isWrapper ?? { isAnyWrapper($0, home: home) }
        let directory = settingsURL.deletingLastPathComponent()
        let names = ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? [])
            .filter { name in prefixes.contains { name.hasPrefix($0) } && name.hasSuffix(backupSuffix) }
            // Timestamped ones newest first, then the original.
            .sorted { lhs, rhs in
                let lhsOriginal = lhs == originalBackupName
                let rhsOriginal = rhs == originalBackupName
                if lhsOriginal != rhsOriginal { return rhsOriginal }
                return lhs > rhs
            }
        for name in names {
            guard let data = try? Data(contentsOf: directory.appendingPathComponent(name)),
                  let document = SettingsDocument(data: data) else { continue }
            guard let statusLine = document["statusLine"] else { return nil }
            if isWrapper(statusLine) { continue }
            return statusLine.members?.isEmpty == false ? statusLine : nil
        }
        return nil
    }
}
