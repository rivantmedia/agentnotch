//
//  CloudLiveEnvironment.swift
//  ClaudeControl
//
//  What `CloudSync` reads from the running engine: the accounts the website
//  may hear of (the registry's visible, remembered, signed-in identities,
//  named as the app names them, each keyed by its own account UUID and
//  organization), who is signed in to each folder (for the backfill's
//  "signed in since" record), the folders the backfill may read, each
//  account's 5-hour usage (summaries wait while it is high), and where a
//  session summary may run. The summary's folder is chosen and checked as
//  the usage probe's is (`UsageStore.startProbe`): one of the account's run
//  folders (`UsageProbePlanner`, never a Claude Parallel Profiles store),
//  signed in as the account right before, and again right after, the run.
//

import Foundation

@MainActor
final class LiveCloudEnvironment: CloudSyncEnvironment {
    private let registry: AccountRegistry
    private let configReader: ClaudeGlobalConfigReader

    init(registry: AccountRegistry, configReader: ClaudeGlobalConfigReader) {
        self.registry = registry
        self.configReader = configReader
    }

    /// The shared registry's.
    convenience init() {
        self.init(registry: .shared, configReader: .shared)
    }

    func accounts() -> [CloudAccountInfo] {
        let names = ClaudeControlHub.shared?.accounts ?? []
        return Self.accounts(identities: registry.visibleIdentities.filter { !registry.isForgotten($0.id) },
                             labels: Dictionary(names.map { ($0.id, $0.label) }, uniquingKeysWith: { first, _ in first }),
                             correctedFolders: registry.correctedFolders)
    }

    /// The accounts of `identities` the website may hear of: signed in,
    /// with an account UUID (an email-only or unsigned folder has no key
    /// that is the same on every Mac), keyed by their own account UUID and
    /// organization (`CloudKeys.accountKey(identity:)`), never by how this
    /// Mac happens to group them. Pure.
    nonisolated static func accounts(identities: [ClaudeIdentityAccount], labels: [String: String],
                                     correctedFolders: Set<String> = []) -> [CloudAccountInfo] {
        identities.compactMap { identity in
            guard !identity.isHidden, identity.isSignedIn,
                  let uuid = AccountIdentityGrouping.accountUuid(ofKey: identity.id), !uuid.isEmpty,
                  let key = CloudKeys.accountKey(identity: identity, correctedFolders: correctedFolders) else { return nil }
            return CloudAccountInfo(identityId: identity.id, accountKey: key, accountUuid: uuid, email: identity.email,
                                    organizationName: identity.organizationName, plan: identity.planName,
                                    label: labels[identity.id] ?? identity.label)
        }
    }

    /// Who is signed in to each folder, once the registry has read it (nil
    /// before: every folder would look signed out, or unchanged since the
    /// last launch).
    func folderLogins() -> [String: String]? {
        guard registry.identitiesRead else { return nil }
        return Self.folderLogins(registry.accounts)
    }

    /// Folder → login for every folder someone is signed in to. Pure.
    nonisolated static func folderLogins(_ folders: [ClaudeAccount]) -> [String: String] {
        var logins: [String: String] = [:]
        for folder in folders {
            guard let login = CloudBackfill.login(accountUuid: folder.accountUuid, organizationUuid: folder.organizationUuid,
                                                  email: folder.email) else { continue }
            logins[folder.configDir] = login
        }
        return logins
    }

    func backfillFolders() -> [CloudBackfill.Folder] {
        let folders = registry.accounts
        var identityOfFolder: [String: String] = [:]
        for folder in folders {
            if let identity = registry.identity(forFolderId: folder.id) { identityOfFolder[folder.id] = identity.id }
        }
        return Self.backfillFolders(folders: folders, accounts: accounts(), identityOfFolder: identityOfFolder,
                                    defaultFolder: AccountRegistry.defaultConfigDir(home: registry.homePath),
                                    mirrorsDefault: registry.mirrorsDefault, correctedFolders: registry.correctedFolders,
                                    infrastructure: registry.infrastructureDirs)
    }

    /// Every folder the registry knows, with the account of those the
    /// backfill may read: a run folder of an allowed account whose own
    /// `.claude.json` names that account (not a mirrored or corrected copy,
    /// and not `~/.claude` while Claude Parallel Profiles mirrors accounts
    /// into it: whose its history is can't be told), with who is signed in
    /// there. Pure.
    nonisolated static func backfillFolders(folders: [ClaudeAccount], accounts: [CloudAccountInfo],
                                            identityOfFolder: [String: String], defaultFolder: String,
                                            mirrorsDefault: Bool, correctedFolders: Set<String>,
                                            infrastructure: [String]) -> [CloudBackfill.Folder] {
        let allowed = Dictionary(accounts.map { ($0.identityId, $0) }, uniquingKeysWith: { first, _ in first })
        var result = folders.map { folder -> CloudBackfill.Folder in
            var account: CloudAccountInfo?
            if folder.kind == .run, let identity = identityOfFolder[folder.id] {
                account = allowed[identity]
            }
            if correctedFolders.contains(folder.id) || (folder.configDir == defaultFolder && mirrorsDefault) {
                account = nil
            }
            return CloudBackfill.Folder(configDir: folder.configDir, identityId: account?.identityId,
                                        accountKey: account?.accountKey,
                                        login: CloudBackfill.login(accountUuid: folder.accountUuid,
                                                                   organizationUuid: folder.organizationUuid,
                                                                   email: folder.email))
        }
        result += infrastructure.map { CloudBackfill.Folder(configDir: $0, identityId: nil, accountKey: nil) }
        return result
    }

    func sessionUtilization(identityId: String) -> Double? {
        UsageStore.shared.usage[identityId]?.fiveHour?.effectiveUtilization(now: Date())
    }

    func summaryFolder(forIdentity identityId: String) async -> CloudSummaryFolder? {
        guard let identity = registry.identity(id: identityId), !identity.runDirs.isEmpty else { return nil }
        let home = registry.homePath
        let mirrorsDefault = registry.mirrorsDefault
        let runDirs = identity.runDirs
        let paths = runDirs.map { registry.globalConfigPath(for: $0) }
        let allRunDirs = registry.accounts.filter { $0.kind == .run }.map(\.configDir)
        let activity: [Date?] = await Task.detached(priority: .utility) {
            zip(runDirs, paths).map { folder, path in
                // ~/.claude.json is rewritten by every mirror: not a sign of this account.
                let isMirroredDefault = mirrorsDefault && AccountRegistry.isDefault(folder, home: home)
                let modified = isMirroredDefault ? nil
                    : (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
                return [folder.lastSeenAt, modified].compactMap { $0 }.max()
            }
        }.value
        guard let folder = UsageProbePlanner.probeFolder(runDirs: runDirs, activity: activity, home: home,
                                                         prefersOwnFolders: mirrorsDefault),
              folder.kind == .run, let index = runDirs.firstIndex(where: { $0.id == folder.id }) else { return nil }
        let configPath = paths[index]
        let reader = configReader
        // Right before it runs: still signed in as this account, and not a store.
        let usable = await Task.detached(priority: .utility) {
            UsageStore.folderRuns(reader.read(path: configPath)?.identity, as: identity)
                && !HookInstaller.isNeverInstallTarget(configDir: folder.configDir, home: home)
        }.value
        guard usable else { return nil }
        return CloudSummaryFolder(configDir: folder.configDir,
                                  configDirEnv: folder.configDirEnv.flatMap { $0.isEmpty ? nil : $0 },
                                  configDirs: allRunDirs)
    }

    func summaryFolderStillRuns(_ folder: CloudSummaryFolder, identityId: String) async -> Bool {
        guard let identity = registry.identity(id: identityId) else { return false }
        let configPath = AccountRegistry.globalConfigPath(configDir: folder.configDir, env: folder.configDirEnv,
                                                          home: registry.homePath)
        let reader = configReader
        return await Task.detached(priority: .utility) {
            UsageStore.folderRuns(reader.read(path: configPath)?.identity, as: identity)
        }.value
    }

    var isLaunchingClaude: Bool { UsageStore.shared.isProbing }
}
