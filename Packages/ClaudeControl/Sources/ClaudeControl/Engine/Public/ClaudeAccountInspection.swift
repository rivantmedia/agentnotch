//
//  ClaudeAccountInspection.swift
//  ClaudeControl
//
//  What the app would make of a home folder's Claude Code folders, printed
//  for a person: `swift run --package-path Packages/ClaudeControl
//  agentnotch-inspect-accounts [--home <dir>]`. The same discovery,
//  classification and grouping the app runs, against a home folder,
//  strictly read-only:
//
//  - directory listings (the home folder, `~/.claude-windows`, each
//    folder's `sessions/` and `hooks/`), and symbolic link targets;
//  - `~/.claude-windows/.manifest.json`, and whether a folder holds the
//    `.parallel-accounts-store` marker;
//  - from each `.claude.json`, `oauthAccount` and, of
//    `cachedUsageUtilization`, only `accountUuid` and `fetchedAtMs` (the
//    rest of the file is skipped over byte by byte, never parsed).
//
//  Nothing is written, no process is signalled or inspected, no credential,
//  Keychain item, settings.json or session file is read, and `claude` is
//  never run.
//

import Foundation

public nonisolated enum ClaudeAccountInspection {
    /// One folder's login, as `.claude.json` names it.
    struct Login: Equatable, Sendable {
        var identity: ClaudeAccountIdentity?
        /// `cachedUsageUtilization.accountUuid` and `.fetchedAtMs`.
        var cachedUsageAccountUuid: String?
        var cachedUsageFetchedAt: Date?
    }

    /// Only the allowed fields of a `.claude.json`.
    static func readLogin(at path: String, fileManager: FileManager = .default) -> Login? {
        guard let data = fileManager.contents(atPath: path),
              let fields = JSONFieldScanner.values(in: data, keys: ["oauthAccount", "cachedUsageUtilization"]) else { return nil }
        var login = Login()
        if let raw = fields["oauthAccount"],
           let object = try? JSONSerialization.jsonObject(with: raw) as? [String: Any] {
            login.identity = ClaudeAccountIdentity(oauthAccount: object)
        }
        if let raw = fields["cachedUsageUtilization"],
           let inner = JSONFieldScanner.values(in: raw, keys: ["accountUuid", "fetchedAtMs"]) {
            login.cachedUsageAccountUuid = inner["accountUuid"].flatMap {
                try? JSONSerialization.jsonObject(with: $0, options: [.fragmentsAllowed]) as? String
            }
            if let bytes = inner["fetchedAtMs"],
               let number = try? JSONSerialization.jsonObject(with: bytes, options: [.fragmentsAllowed]) as? NSNumber {
                login.cachedUsageFetchedAt = Date(timeIntervalSince1970: number.doubleValue / 1000)
            }
        }
        return login
    }

    /// What the inspection found.
    public struct Result: Sendable {
        public var home: String
        public var extensionDetected: Bool
        public var manifestStores: [String]
        /// Profiles the extension adopted (in `stores`, not made by it): run folders.
        public var adoptedFolders: [String] = []
        /// Who `~/.claude`'s own `accountUuid` names, when the extension has
        /// mirrored another account into it (its old ring's choices go there).
        public var defaultOwner: String?
        /// One entry per account: its label, email, ring id, run folders and stores.
        public var accounts: [Account]
        public var infrastructure: [String]
        public var unsignedFolders: [String]
        /// Where "Turn on" installs (every run folder of a tracked account).
        public var installTargets: [String]
        /// Folders whose `hooks/` holds Superpowered Vibe Notch's scripts.
        public var vibeNotchCleanupTargets: [String]
        /// Per-folder ring ids of earlier versions → the account ring (nil: retired).
        public var ringMoves: [(old: String, new: String?)]
        /// Every folder found, with what it is and who its `.claude.json` names.
        public var folders: [Folder] = []

        public struct Folder: Sendable {
            public var path: String
            public var kind: String
            public var email: String?
            public var accountUuidPrefix: String?
            /// Its UUID belongs to another account (a mirrored `~/.claude.json`):
            /// its email decided.
            public var corrected: Bool
        }

        public struct Account: Sendable {
            public var label: String
            public var email: String?
            public var accountUuid: String?
            public var plan: String?
            public var ringID: String
            public var runDirs: [String]
            public var storeDirs: [String]
            /// Where Claude Code's freshest cached usage (of this identity) is.
            public var cachedUsageFolder: String?
            public var cachedUsageFetchedAt: Date?
            /// Where the usage check would run (never a store).
            public var probeFolder: String?
        }
    }

    /// Discover, classify and group the Claude Code folders of `home`.
    public static func inspect(home: String, fileManager: FileManager = .default) -> Result {
        let home = AccountPaths.normalize(home)
        // No process is asked whether it runs (`isAlive` false).
        let snapshot = ConfigDirClassifier.readSnapshot(home: home, fileManager: fileManager, isAlive: { _ in false })
        let discovery = AccountRegistry.discover(snapshot: snapshot)
        let layout = discovery.layout
        let defaultDir = AccountRegistry.defaultConfigDir(home: home)

        var logins: [String: Login] = [:]
        var folders: [ClaudeAccount] = []
        for dir in discovery.accounts {
            let isDefault = dir == defaultDir
            var folder = ClaudeAccount(configDir: dir, configDirEnv: isDefault ? nil : dir,
                                       source: .discovered, kind: layout.kind(of: dir) ?? .run)
            let identityFile = isDefault
                ? (home as NSString).appendingPathComponent(".claude.json")
                : (dir as NSString).appendingPathComponent(".claude.json")
            if let login = readLogin(at: identityFile, fileManager: fileManager) {
                logins[folder.id] = login
                if let identity = login.identity {
                    folder.email = identity.email
                    folder.displayName = identity.displayName
                    folder.organizationName = identity.organizationName
                    folder.organizationUuid = identity.organizationUuid
                    folder.accountUuid = identity.accountUuid
                    folder.rateLimitTier = identity.rateLimitTier
                    folder.subscriptionType = identity.subscriptionType
                }
            }
            folders.append(folder)
        }
        let grouping = AccountIdentityGrouping.group(folders, prefs: [:], mirrorsDefault: layout.extensionDetected, home: home)

        func display(_ path: String) -> String { AccountPathDisplayName.abbreviated(path, home: home) }
        var accounts: [Result.Account] = []
        for identity in grouping.identities {
            let uuid = identity.accountUuid?.lowercased()
            let cached = identity.folders.compactMap { folder -> (String, Date)? in
                guard let login = logins[folder.id], let fetched = login.cachedUsageFetchedAt,
                      login.cachedUsageAccountUuid?.lowercased() == uuid else { return nil }
                return (folder.configDir, fetched)
            }.max { $0.1 < $1.1 }
            let probe = UsageProbePlanner.probeFolder(runDirs: identity.runDirs,
                                                      activity: identity.runDirs.map { _ in nil }, home: home,
                                                      prefersOwnFolders: layout.extensionDetected)
            accounts.append(Result.Account(
                label: identity.label,
                email: identity.email,
                accountUuid: identity.accountUuid,
                plan: identity.planName,
                ringID: identity.ringID,
                runDirs: identity.runDirs.map { display($0.configDir) },
                storeDirs: identity.storeDirs.map { display($0.configDir) },
                cachedUsageFolder: cached.map { display($0.0) },
                cachedUsageFetchedAt: cached?.1,
                probeFolder: probe.map { display($0.configDir) }
            ))
        }

        let install = (grouping.identities.filter { !$0.isHidden }.flatMap(\.runDirs) + grouping.unsignedFolders)
            .filter { $0.kind == .run }
            .map(\.configDir)
        let cleanup = (folders.map(\.configDir) + layout.infrastructure)
            .filter { VibeNotchLeftovers.hasScripts(configDir: $0, fileManager: fileManager) }

        let currentRings = Set(grouping.identities.map(\.ringID))
        var moves: [(old: String, new: String?)] = []
        var seen = Set<String>()
        // The app's own rule (`ClaudeHostProjections.account(identity:…)`):
        // a mirrored ~/.claude's old ring goes to its owner.
        let defaultRing = ClaudeHostProjections.DefaultRingOwner(mirrored: grouping.correctedFolders.contains(defaultDir),
                                                                  owner: grouping.defaultOwner)
        for identity in grouping.identities {
            let summary = ClaudeHostProjections.account(identity: identity, hookStatuses: [:], defaultRing: defaultRing, home: home)
            for old in summary.formerRingIDs where old != identity.ringID && seen.insert(old).inserted {
                moves.append((old, identity.ringID))
            }
        }
        // The windows folder itself never had a ring.
        for dir in layout.infrastructure.filter({ $0 != ParallelProfiles.windowsRoot(home: home) })
            + grouping.unsignedFolders.map(\.configDir) {
            let old = ClaudeRingIdentity.ringID(configDir: dir, home: home)
            guard !currentRings.contains(old), seen.insert(old).inserted else { continue }
            moves.append((old, nil))
        }

        let keys = AccountIdentityGrouping.identityKeys(folders, mirroredDefault: layout.extensionDetected ? defaultDir : nil)
        let folderRows = folders.map { folder in
            Result.Folder(path: display(folder.configDir), kind: folder.kind.rawValue, email: folder.email,
                          accountUuidPrefix: folder.accountUuid.map { String($0.prefix(8)) },
                          corrected: keys[folder.id]?.corrected ?? false)
        }

        var result = Result(
            home: home,
            extensionDetected: layout.extensionDetected,
            manifestStores: (snapshot.manifest?.stores ?? []).map(display),
            accounts: accounts,
            infrastructure: layout.infrastructure.map(display),
            unsignedFolders: grouping.unsignedFolders.map { display($0.configDir) },
            installTargets: install.map(display),
            vibeNotchCleanupTargets: cleanup.map(display),
            ringMoves: moves
        )
        result.folders = folderRows
        result.adoptedFolders = layout.adoptedByExtension.sorted().map(display)
        if defaultRing.mirrored {
            result.defaultOwner = grouping.identities.first { $0.id == grouping.defaultOwner }?.label ?? "nobody known"
        }
        return result
    }

    /// The inspection as text.
    public static func report(home: String) -> String {
        let result = inspect(home: home)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        var lines: [String] = []
        lines.append("Claude accounts in \(result.home) (read-only inspection)")
        if result.extensionDetected {
            lines.append("Claude Parallel Profiles: detected; manifest stores: \(result.manifestStores.isEmpty ? "none" : result.manifestStores.joined(separator: ", "))")
            if !result.adoptedFolders.isEmpty {
                lines.append("Adopted by it (your own profiles, run folders): \(result.adoptedFolders.joined(separator: ", "))")
            }
        } else {
            lines.append("Claude Parallel Profiles: not detected")
        }
        lines.append("")
        lines.append("Accounts: \(result.accounts.count)")
        for (index, account) in result.accounts.enumerated() {
            let uuid = account.accountUuid.map { " (accountUuid \($0.prefix(8))…)" } ?? ""
            let plan = account.plan.map { " · \($0)" } ?? ""
            lines.append("\(index + 1). \(account.label) — \(account.email ?? "not signed in")\(uuid)\(plan)")
            lines.append("   ring:   \(account.ringID)")
            lines.append("   runs:   \(account.runDirs.isEmpty ? "nowhere now" : account.runDirs.joined(separator: ", "))")
            lines.append("   stores: \(account.storeDirs.isEmpty ? "none" : account.storeDirs.joined(separator: ", "))")
            if let folder = account.cachedUsageFolder, let fetched = account.cachedUsageFetchedAt {
                lines.append("   cached usage: freshest in \(folder), fetched \(formatter.string(from: fetched))")
            } else {
                lines.append("   cached usage: none matching this account")
            }
            lines.append("   usage check: \(account.probeFolder.map { "runs in \($0) (a run folder; VS Code windows and standalone folders before a mirrored ~/.claude, then the most recently active)" } ?? "not run (no run folder; passive data only)")")
        }
        lines.append("")
        lines.append("Folders (each one's own oauthAccount):")
        for folder in result.folders {
            var line = "   \(folder.path) [\(folder.kind)] \(folder.email ?? "not signed in")"
            if let uuid = folder.accountUuidPrefix { line += " (\(uuid)…)" }
            if folder.corrected { line += " — its accountUuid is another account's (a mirrored .claude.json): its email decides" }
            lines.append(line)
        }
        lines.append("")
        lines.append("Infrastructure (never accounts): \(result.infrastructure.isEmpty ? "none" : result.infrastructure.joined(separator: ", "))")
        lines.append("Not signed in (no ring): \(result.unsignedFolders.isEmpty ? "none" : result.unsignedFolders.joined(separator: ", "))")
        lines.append("")
        lines.append("Install targets after consent (hooks + status line): \(result.installTargets.count)")
        for target in result.installTargets { lines.append("   \(target)/settings.json") }
        lines.append("Superpowered Vibe Notch cleanup targets (its scripts in hooks/): \(result.vibeNotchCleanupTargets.count)")
        for target in result.vibeNotchCleanupTargets { lines.append("   \(target)") }
        lines.append("")
        if let owner = result.defaultOwner {
            lines.append("~/.claude's own accountUuid belongs to: \(owner) (its earlier ring and saved choices go there)")
        }
        lines.append("Ring ids of earlier versions:")
        for move in result.ringMoves {
            lines.append("   \(move.old) → \(move.new ?? "retired (not an account)")")
        }
        return lines.joined(separator: "\n")
    }
}
