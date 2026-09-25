//
//  VibeNotchLeftovers.swift
//  ClaudeControl
//
//  What Superpowered Vibe Notch leaves behind once its entries are out of a
//  settings.json: its script copies in `<folder>/hooks/`, the status line it
//  saved there, and — in a folder it had no business writing to (a Claude
//  Parallel Profiles store, `~/.claude-shared`) — a settings.json it created
//  that is now just `{}`. The takeover removes these so a store looks the
//  way the extension made it again.
//
//  Only files that are provably Superpowered Vibe Notch's go:
//  - its two scripts and its saved status line, and only when no known
//    settings.json still runs a script of that folder (a missing script
//    could make a hook exit with an error Claude Code treats as "block");
//  - a settings.json only when it is `{}` now, only in a store or the
//    shared folder, and only when no backup beside it (its own or this
//    app's) holds anything of the user's: a backup that is blank, `{}`, or
//    becomes `{}` once that app's and this app's entries are taken out
//    holds nothing (an earlier build of this app installed into stores and
//    backed up the `{}` the takeover left); together with those backups;
//  - the `hooks` folder when that leaves it empty.
//  Scripts are left alone while any known settings.json can't be read: an
//  unreadable file may still run them, and their hook commands have no
//  "script missing" guard (a missing script blocks every tool call there).
//  Never `.claude.json`, a credential, the store marker, the manifest or the
//  shared history.
//

import Foundation
import os.log

nonisolated enum VibeNotchLeftovers {
    private static var logger: Logger { EngineLog.logger("Hooks") }

    static var scriptNames: [String] { [AppIdentity.vibeNotchHookScriptName, AppIdentity.vibeNotchStatusLineScriptName] }

    /// Superpowered Vibe Notch's files in `<folder>/hooks/` (a listing).
    static func files(configDir: String, fileManager: FileManager = .default) -> [String] {
        let hooks = HookInstaller.hooksDir(configDir: configDir).path
        let names = Set(scriptNames + [AppIdentity.vibeNotchPreviousStatusLineFileName])
        return ((try? fileManager.contentsOfDirectory(atPath: hooks)) ?? [])
            .filter { names.contains($0) }
            .sorted()
            .map { (hooks as NSString).appendingPathComponent($0) }
    }

    /// Whether `<folder>/hooks/` holds Superpowered Vibe Notch's scripts.
    static func hasScripts(configDir: String, fileManager: FileManager = .default) -> Bool {
        files(configDir: configDir, fileManager: fileManager).contains { path in
            scriptNames.contains((path as NSString).lastPathComponent)
        }
    }

    /// Every Superpowered Vibe Notch script path the settings.json files of
    /// `configDirs` still run (hooks and status line), links resolved.
    static func referencedScripts(configDirs: [String], home: String = AccountPaths.homeDirectory) -> Set<String> {
        referencedScripts(configDirs: configDirs, home: home, requireReadable: false) ?? []
    }

    /// `referencedScripts`, or nil when an existing settings.json among
    /// `configDirs` can't be read or parsed (it might run them: no script
    /// may go then).
    static func referencedScriptsIfAllReadable(configDirs: [String], home: String = AccountPaths.homeDirectory) -> Set<String>? {
        referencedScripts(configDirs: configDirs, home: home, requireReadable: true)
    }

    private static func referencedScripts(configDirs: [String], home: String, requireReadable: Bool) -> Set<String>? {
        var referenced = Set<String>()
        for configDir in configDirs {
            let url = HookInstaller.settingsFile(configDir: configDir)
            guard FileManager.default.fileExists(atPath: url.path) else { continue }
            guard let data = try? Data(contentsOf: url),
                  let document = SettingsDocument(data: data) else {
                if requireReadable { return nil }
                continue
            }
            let json = document.value
            var commands: [String] = []
            for member in json["hooks"]?.members ?? [] {
                for group in member.value.items ?? [] {
                    for entry in group["hooks"]?.items ?? [] {
                        if let command = entry["command"]?.stringValue { commands.append(command) }
                    }
                }
            }
            if let command = json["statusLine"]?["command"]?.stringValue { commands.append(command) }
            for command in commands {
                for name in scriptNames {
                    if let script = HookCommands.scriptPath(in: command, named: name, home: home) {
                        referenced.insert(URL(fileURLWithPath: script).resolvingSymlinksInPath().path)
                    }
                }
            }
        }
        return referenced
    }

    /// What `cleanUp` removed.
    struct Result: Equatable, Sendable {
        var removedFiles: [String] = []
        var removedSettings = false
    }

    /// Remove the leftovers of one folder whose settings.json no longer has
    /// Superpowered Vibe Notch's entries (see the file comment).
    /// `removesBlankSettings` is for stores and the shared folder only.
    @discardableResult
    static func cleanUp(
        configDir: String,
        referenced: Set<String>,
        removesBlankSettings: Bool,
        home: String = AccountPaths.homeDirectory,
        fileManager: FileManager = .default
    ) -> Result {
        var result = Result()
        guard !HookInstaller.isProtectedBeforeBootstrap(configDir: configDir), !DevFlags.installsDisabled else { return result }
        // Its entries must be gone first, from a file that can be read.
        let status = HookInstaller.readStatus(configDir: configDir)
        guard status.settingsReadable, !status.superpoweredVibeNotchHooksPresent else { return result }

        for path in files(configDir: configDir, fileManager: fileManager) {
            let real = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
            guard !referenced.contains(real) else { continue }
            if (try? fileManager.removeItem(atPath: path)) != nil { result.removedFiles.append(path) }
        }

        if removesBlankSettings, let removed = removeCreatedBlankSettings(configDir: configDir, home: home, fileManager: fileManager) {
            result.removedSettings = true
            result.removedFiles += removed
        }

        let hooks = HookInstaller.hooksDir(configDir: configDir).path
        if (try? fileManager.contentsOfDirectory(atPath: hooks))?.isEmpty == true,
           (try? fileManager.removeItem(atPath: hooks)) != nil {
            result.removedFiles.append(hooks)
        }
        if !result.removedFiles.isEmpty {
            logger.info("Removed Superpowered Vibe Notch's leftovers in \(configDir, privacy: .public): \(result.removedFiles.map { ($0 as NSString).lastPathComponent }.joined(separator: ", "), privacy: .public)")
        }
        return result
    }

    /// Whether a settings.json's bytes, with Superpowered Vibe Notch's
    /// entries taken out, are an empty object. Pure.
    static func isOnlyVibeNotch(_ data: Data, home: String = AccountPaths.homeDirectory) -> Bool {
        guard SettingsDocument(data: data) != nil else { return false }
        let plan = HookInstaller.planLegacyRemoval(existingData: data, kinds: [.superpoweredVibeNotch], home: home)
        let remaining: Data
        switch plan.settings {
        case .write(let written): remaining = written
        case .alreadyCurrent: remaining = data
        case .refuse: return false
        }
        guard let document = SettingsDocument(data: remaining) else { return false }
        return (document.value.members ?? []).isEmpty
    }

    /// Whether a settings.json's bytes hold nothing of the user's: blank,
    /// `{}`, or `{}` once Superpowered Vibe Notch's entries and this app's
    /// (hooks, status line wrapper) are taken out. Pure.
    static func holdsNoUserContent(_ data: Data, home: String = AccountPaths.homeDirectory) -> Bool {
        if SettingsDocument.isBlank(data) { return true }
        guard SettingsDocument(data: data) != nil else { return false }
        var remaining = data
        if case .write(let written) = HookInstaller.planLegacyRemoval(existingData: remaining, kinds: [.superpoweredVibeNotch], home: home).settings {
            remaining = written
        }
        switch HookInstaller.planUninstall(existingData: remaining, savedPreviousStatusLine: nil, home: home).settings {
        case .write(let written): remaining = written
        case .alreadyCurrent: break
        case .refuse: return false
        }
        guard let document = SettingsDocument(data: remaining) else { return false }
        return (document.value.members ?? []).isEmpty
    }

    /// Whether a settings.json's bytes hold any of Superpowered Vibe Notch's
    /// entries. Pure.
    static func hasVibeNotchEntries(_ data: Data, home: String = AccountPaths.homeDirectory) -> Bool {
        guard let document = SettingsDocument(data: data) else { return false }
        let json = document.value
        return HookInstaller.firstHookCommand(in: json) { HookInstaller.isLegacyHook($0, kind: .superpoweredVibeNotch, home: home) } != nil
            || HookInstaller.isLegacyStatusLine(json["statusLine"], kind: .superpoweredVibeNotch, home: home)
    }

    /// `<folder>/settings.json` is a plain file holding `{}` (what taking
    /// Superpowered Vibe Notch's entries out of a file it created leaves).
    static func hasEmptyObjectSettings(configDir: String) -> Bool {
        let url = HookInstaller.settingsFile(configDir: configDir)
        guard case .notALink = HookInstaller.linkState(url), let data = try? Data(contentsOf: url),
              !SettingsDocument.isBlank(data), let document = SettingsDocument(data: data) else { return false }
        return (document.value.members ?? []).isEmpty
    }

    /// Remove `<folder>/settings.json` if it is `{}` and Superpowered Vibe
    /// Notch created it, with the backups beside it that hold only that
    /// app's entries. Returns what was removed, or nil when it stays.
    static func removeCreatedBlankSettings(configDir: String, home: String, fileManager: FileManager = .default) -> [String]? {
        let settingsURL = HookInstaller.settingsFile(configDir: configDir)
        guard case .notALink = HookInstaller.linkState(settingsURL),
              let data = try? Data(contentsOf: settingsURL),
              let document = SettingsDocument(data: data),
              (document.value.members ?? []).isEmpty else { return nil }
        let directory = settingsURL.deletingLastPathComponent()
        let backups = ((try? fileManager.contentsOfDirectory(atPath: directory.path)) ?? [])
            .filter { name in
                (name.hasPrefix(AppIdentity.vibeNotchBackupPrefix) || name.hasPrefix(HookInstaller.backupPrefix)
                    || name == HookInstaller.originalBackupName) && name.hasSuffix(HookInstaller.backupSuffix)
            }
            .map { directory.appendingPathComponent($0) }
        // A version with anything of the user's in it: the file is theirs,
        // and stays.
        for backup in backups {
            guard let bytes = try? Data(contentsOf: backup) else { return nil }
            if !holdsNoUserContent(bytes, home: home) { return nil }
        }
        var removed: [String] = []
        guard (try? fileManager.removeItem(at: settingsURL)) != nil else { return nil }
        removed.append(settingsURL.path)
        for backup in backups {
            guard let bytes = try? Data(contentsOf: backup), holdsNoUserContent(bytes, home: home) else { continue }
            if (try? fileManager.removeItem(at: backup)) != nil { removed.append(backup.path) }
        }
        return removed
    }
}
