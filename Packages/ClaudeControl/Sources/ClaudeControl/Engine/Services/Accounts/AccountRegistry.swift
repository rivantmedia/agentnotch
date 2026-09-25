//
//  AccountRegistry.swift
//  ClaudeControl
//
//  The Claude accounts the app knows about. An account is a Claude Code
//  config dir (`CLAUDE_CONFIG_DIR`, default ~/.claude). They come from:
//  - discovery at launch and every 5 minutes, which adds only folders that
//    are clearly in use: ~/.claude, and a ~/.claude-* or ~/.claude_* folder
//    that is signed in (`.claude.json` with `oauthAccount`) or has a live
//    session (a `sessions/<pid>.json` whose process runs), unless its name
//    says it is a backup; `SPCN_EXTRA_CONFIG_DIRS`;
//  - hook and status line events (`AppEventBus.accountSightings`), which
//    also carry the raw CLAUDE_CONFIG_DIR a session runs with;
//  - the user (add an existing folder, create a new one, or accept a
//    suggestion).
//  Other look-alike folders (a copy of ~/.claude with history but no login,
//  `~/.claude-backup`) are only suggested; a forgotten account that shows
//  up again in a session is suggested too, never re-added by itself.
//
//  User-facing fields (label, colour, hidden, ...) persist in
//  `supportDirectory/accounts.json`. Identity (email, plan, ...) is read from
//  each account's `.claude.json` off the main thread, and only re-parsed when
//  that file changes. Folders are never deleted, only forgotten.
//
//  Folders are classified (`ConfigDirClassifier`): Claude Code runs in some,
//  others are Claude Parallel Profiles stores (read, never written), and
//  infrastructure (`~/.claude-shared`, `~/.claude-windows`) is never an
//  account. A VS Code window's working copy (`~/.claude-windows/<id>`) is
//  added as soon as it has a `.claude.json`. The folders are then grouped
//  by who is signed in (`AccountIdentityGrouping`): `identities` is the list
//  of accounts, one per identity, and name, colour and tracking are chosen
//  per identity.
//

import Combine
import Foundation
import os.log

/// A folder that looks like a Claude config dir but wasn't added by itself.
nonisolated struct AccountFolderSuggestion: Identifiable, Hashable, Sendable {
    enum Reason: Hashable, Sendable {
        /// Has Claude Code's folders but no login and no session files.
        case found
        /// Named like a backup copy (`-backup`, `-old`, a date, …).
        case looksLikeBackup
        /// Forgotten earlier, and a session ran there since.
        case seenAgain
    }

    var id: String { configDir }
    let configDir: String
    let reason: Reason
}

/// Why a folder can't be an account.
nonisolated enum AccountFolderError: LocalizedError, Equatable, Sendable {
    case missing
    case notAFolder
    /// The home folder itself: Claude Code would treat all of home as its config.
    case homeFolder
    /// A folder that holds the home folder or ~/.claude (`/`, `/Users`, …).
    case containsAccounts
    /// Inside another account's config folder (its `projects/`, `hooks/`, …).
    case insideAccount(String)
    /// Sealed runs never touch real folders.
    case unavailableWhenSealed
    /// Claude Parallel Profiles' shared history or windows folder: never an account.
    case infrastructure

    var errorDescription: String? {
        switch self {
        case .missing:
            return "That folder doesn't exist."
        case .notAFolder:
            return "That's a file, not a folder."
        case .homeFolder:
            return "That's your home folder, not a Claude Code config folder. Pick a folder like ~/.claude-work."
        case .containsAccounts:
            return "That folder contains your home folder. Pick a Claude Code config folder like ~/.claude-work."
        case .insideAccount(let label):
            return "That folder is inside \(label)'s config folder. Pick the config folder itself."
        case .unavailableWhenSealed:
            return "Not available in the sealed demo."
        case .infrastructure:
            return "That folder is Claude Parallel Profiles' shared history (or its windows folder), not an account. Its accounts are found by themselves."
        }
    }
}

/// What a folder holds, for the "doesn't look like a Claude Code folder" question.
nonisolated struct AccountFolderMarkers: Equatable, Sendable {
    var hasProjects = false
    var hasSessions = false
    var hasGlobalConfig = false
    var isSignedIn = false

    /// Session history or a session registry: clearly a config folder, no
    /// need to ask. A `.claude.json` alone could be anything (home has one).
    var isClearlyConfigDir: Bool { hasProjects || hasSessions }
}

@MainActor
final class AccountRegistry: ObservableObject {
    static let shared: AccountRegistry = {
        let registry = AccountRegistry(deferred: true)
        sharedIfCreated = registry
        registry.discoverSynchronously()
        return registry
    }()
    /// The shared registry, once made (only it tells `AccountPaths` which
    /// folders are infrastructure).
    nonisolated(unsafe) private static weak var sharedIfCreated: AccountRegistry?

    nonisolated private static var logger: Logger { EngineLog.logger("Accounts") }

    /// Distinct colours in `AccountPalette`; colour indices cycle through these.
    nonisolated static let paletteSize = 8

    /// How often the home folder is rescanned for new config dirs.
    static let discoveryInterval: TimeInterval = 5 * 60

    /// Sightings closer together than this don't update `lastSeenAt`.
    private static let sightingResolution: TimeInterval = 60

    /// Every known account: the default account first, then by label.
    /// Hidden accounts are included (flagged `isHidden`).
    @Published private(set) var accounts: [ClaudeAccount] = []

    /// Folders that look like accounts but need the user's yes.
    @Published private(set) var suggestions: [AccountFolderSuggestion] = []

    /// Accounts to show and poll.
    var visibleAccounts: [ClaudeAccount] {
        accounts.filter { !$0.isHidden }
    }

    /// One per signed-in identity (plus folders added by hand that nobody
    /// has signed in to yet): what the notch, the panel and Settings call an
    /// account. By label (not by which one `~/.claude` runs as, which the
    /// extension changes whenever another VS Code window is focused).
    @Published private(set) var identities: [ClaudeIdentityAccount] = []

    /// Run folders nobody is signed in to, with no ring of their own
    /// (Settings lists them as "not signed in").
    @Published private(set) var unsignedFolders: [ClaudeAccount] = []

    /// The latest classification of every Claude folder found: which are
    /// run folders, stores and infrastructure, and whether Claude Parallel
    /// Profiles is in use.
    @Published private(set) var layout = ConfigDirLayout()

    /// Discovery has classified the folders at least once this run (the
    /// hook manager installs nothing before).
    private(set) var hasClassified = false

    /// Identities to show and poll.
    var visibleIdentities: [ClaudeIdentityAccount] {
        identities.filter { !$0.isHidden }
    }

    /// What the user chose per identity (name, colour, tracking).
    private var identityPrefs: [String: IdentityPrefs] = [:]
    /// Identities the user forgot: their folders stay known (a window can
    /// switch to another account), but have no ring and no hooks.
    private var forgottenIdentityKeys: Set<String> = []
    /// Folder id → identity id.
    private var identityOfFolder: [String: String] = [:]
    /// Folders of forgotten identities.
    private var forgottenIdentityFolders: Set<String> = []
    /// Folders accounts.json had (their per-folder choices seed their
    /// identity's, when it has none yet).
    private var savedFolderIds: Set<String> = []
    /// Identities have been read from the folders at least once.
    private var identitiesRead = false
    /// Folder → identity key, forgotten identities included.
    private var folderKeys: [String: String] = [:]
    /// The identity `~/.claude`'s own `accountUuid` names (see
    /// `AccountIdentityGrouping.defaultOwner`).
    private(set) var defaultOwner: String?
    /// Folders whose `.claude.json` names another account's UUID (mirrored).
    private(set) var correctedFolders: Set<String> = []
    /// Who `~/.claude` ran as over time (see `FolderIdentityTimeline`).
    private(set) var defaultTimeline = FolderIdentityTimeline()
    /// The next look at `~/.claude` is the first since launch.
    private var defaultTimelineResumed = true
    /// When `~/.claude.json` was written, as of its last read.
    private var defaultIdentityModifiedAt: Date?
    /// The clock timelines are kept by (injectable for tests).
    private let clock: () -> Date
    /// Sealed: fixtures only, nothing observed or saved.
    private var holdsFixtures = false

    private let home: String
    /// The home folder this registry discovers in.
    var homePath: String { home }
    private let storeURL: URL
    private let configReader: ClaudeGlobalConfigReader
    private let extraConfigDirs: [String]

    /// Accounts the user forgot (or suggestions they dismissed). Neither
    /// discovery nor a sighting brings them back; a sighting only suggests.
    private var removedIds: Set<String> = []
    /// Removed accounts a session ran in since.
    private var seenAgainIds: Set<String> = []
    /// What the last discovery found but did not add.
    private var discoveredSuggestions: [AccountFolderSuggestion] = []

    private var started = false
    private var cancellables = Set<AnyCancellable>()
    private var discoveryTask: Task<Void, Never>?
    private var windowWatchTask: Task<Void, Never>?
    private var saveTask: Task<Void, Never>?
    private var identityRefreshTail: Task<Void, Never>?

    /// - Parameters:
    ///   - home: the home folder to discover in (injectable for tests).
    ///   - storeURL: where accounts.json lives (injectable for tests).
    ///   - extraConfigDirs: `SPCN_EXTRA_CONFIG_DIRS`, added like discovered accounts.
    init(
        home: String = AccountPaths.homeDirectory,
        storeURL: URL? = nil,
        configReader: ClaudeGlobalConfigReader = .shared,
        extraConfigDirs: [String] = DevFlags.extraConfigDirs,
        deferred: Bool = false,
        clock: @escaping () -> Date = { Date() }
    ) {
        self.home = AccountPaths.normalize(home)
        self.storeURL = storeURL ?? AppIdentity.supportDirectory.appendingPathComponent("accounts.json")
        self.configReader = configReader
        self.extraConfigDirs = extraConfigDirs
        self.clock = clock
        loadPersisted()
        if !deferred { discoverSynchronously() }
    }

    /// Before bootstrap (tests, the snapshots tool) a registry pointed at
    /// the real home folder reads nothing there.
    private var mayReadHome: Bool {
        !AccountPaths.isRealHomeBeforeBootstrap(home)
    }

    /// Discover, classify and read who is signed in where, right away and
    /// synchronously, before anything reads `accounts` or `identities`: the
    /// rings made at launch need their identities, and the hook manager's
    /// first pass must never mistake a store for a run folder. Directory
    /// listings, link targets, the manifest and two keys of each
    /// `.claude.json` (milliseconds per folder).
    private func discoverSynchronously() {
        guard mayReadHome else { return }
        let found = Self.discover(home: home, extraDirs: extraConfigDirs, knownDirs: accounts.map(\.configDir),
                                  previousStores: Set(layout.stores))
        applyLayout(found.layout)
        var updated = accounts
        for dir in found.accounts where !updated.contains(where: { $0.id == dir }) && !removedIds.contains(dir) {
            let isDefaultDir = dir == Self.defaultConfigDir(home: home)
            updated.append(ClaudeAccount(configDir: dir, configDirEnv: isDefaultDir ? nil : dir,
                                         colorIndex: Self.nextColorIndex(used: updated.map(\.colorIndex)),
                                         source: .discovered, kind: found.layout.kind(of: dir) ?? .run))
        }
        for index in updated.indices {
            let config = configReader.read(path: Self.globalConfigPath(for: updated[index], home: home))
            updated[index] = Self.applying(config?.identity, to: updated[index])
            if isDefault(updated[index]) { defaultIdentityModifiedAt = config?.modifiedAt }
        }
        identitiesRead = true
        discoveredSuggestions = found.suggestions
        publish(updated)
        refreshSuggestions()
    }

    /// Take a new classification: drop folders that turned out to be
    /// infrastructure, and window copies and stores that are gone.
    private func applyLayout(_ layout: ConfigDirLayout) {
        if self.layout != layout { self.layout = layout }
        if self === AccountRegistry.sharedIfCreated { AccountPaths.setInfrastructureDirs(layout.infrastructure) }
        hasClassified = true
        let kept = accounts.filter { folder in
            switch layout.kind(of: folder.configDir) {
            case .infrastructure:
                Self.logger.info("Not an account (shared history or windows folder): \(folder.configDir, privacy: .public)")
                return false
            case .some:
                return true
            case nil:
                // Gone from disk: the extension's own folders go with it;
                // anything else stays ("Folder missing").
                let isExtensionFolder = ParallelProfiles.isWindowDir(folder.configDir, home: home) || folder.kind == .store
                return !isExtensionFolder
            }
        }
        publish(kept)
    }

    // MARK: - Lifecycle

    /// Start discovery, the periodic rescan and sighting intake. Idempotent.
    func start() {
        guard !started else { return }
        started = true

        AppEventBus.shared.accountSightings
            .receive(on: DispatchQueue.main)
            .sink { [weak self] sighting in
                self?.record(sighting)
            }
            .store(in: &cancellables)

        discoveryTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.discoverNow()
                try? await Task.sleep(for: .seconds(Self.discoveryInterval))
            }
        }
        watchWindows()
    }

    /// Follow `~/.claude-windows`: a new VS Code window's folder is added
    /// (and gets hooks) within seconds; a window that switched account is
    /// regrouped (see `WindowDirWatch`).
    private func watchWindows() {
        guard mayReadHome, windowWatchTask == nil else { return }
        let home = self.home
        windowWatchTask = Task { [weak self] in
            var last: WindowDirWatch.Fingerprint?
            while !Task.isCancelled {
                let now = await Task.detached(priority: .utility) { WindowDirWatch.fingerprint(home: home) }.value
                switch WindowDirWatch.change(from: last, to: now) {
                case .none: break
                case .folders: await self?.discoverNow()
                case .identities: await self?.refreshIdentities()
                }
                last = now
                try? await Task.sleep(for: .seconds(WindowDirWatch.interval))
            }
        }
    }

    func stop() {
        discoveryTask?.cancel()
        discoveryTask = nil
        windowWatchTask?.cancel()
        windowWatchTask = nil
        cancellables.removeAll()
        started = false
        saveNow()
    }

    // MARK: - Fixtures

    /// Sealed mode: show `fixtures` and nothing else. In memory only; nothing
    /// is discovered, read or saved.
    func replaceAllWithFixtures(_ fixtures: [ClaudeAccount], layout: ConfigDirLayout? = nil) {
        holdsFixtures = true
        saveTask?.cancel()
        saveTask = nil
        suggestions = []
        if let layout, self.layout != layout { self.layout = layout }
        identitiesRead = true
        publish(fixtures)
    }

    // MARK: - Queries

    func account(id: String) -> ClaudeAccount? {
        accounts.first { $0.id == id }
    }

    func account(forConfigDir configDir: String) -> ClaudeAccount? {
        account(id: AccountPaths.accountId(forConfigDir: configDir))
    }

    func identity(id: String) -> ClaudeIdentityAccount? {
        identities.first { $0.id == id }
    }

    /// The identity a folder (an account id sessions and hooks carry) belongs to now.
    func identity(forFolderId folderId: String) -> ClaudeIdentityAccount? {
        identityOfFolder[AccountPaths.normalize(folderId)].flatMap(identity(id:))
    }

    /// The identity id for a folder id, or the identity id itself.
    func identityId(for id: String) -> String? {
        if identities.contains(where: { $0.id == id }) { return id }
        return identityOfFolder[AccountPaths.normalize(id)]
    }

    /// Who a session in `folderId`, whose process started at `startedAt`,
    /// runs as: `~/.claude` is attributed by who it ran as then while
    /// Claude Parallel Profiles mirrors accounts into it (see
    /// `FolderIdentityTimeline`); other folders by who they name now.
    func attribution(forFolderId folderId: String, startedAt: Date?) -> FolderAttribution {
        let folder = AccountPaths.normalize(folderId)
        let isDefaultFolder = folder == Self.defaultConfigDir(home: home)
        return FolderAttribution.attribute(current: folderKeys[folder] ?? identityOfFolder[folder],
                                           timeline: isDefaultFolder ? defaultTimeline : nil,
                                           startedAt: startedAt,
                                           mirrored: isDefaultFolder && layout.extensionDetected)
    }

    /// Whether Claude Parallel Profiles mirrors accounts into `~/.claude`.
    var mirrorsDefault: Bool { layout.extensionDetected }

    /// The identity what was saved for a folder (before accounts were
    /// identities) belongs to: its identity, except for a mirrored
    /// `~/.claude`, whose saved choices and readings are its owner's (the
    /// account its own `accountUuid` names), or nobody's when that is
    /// unknown.
    func owner(ofSavedFolder folderId: String) -> String? {
        let folder = AccountPaths.normalize(folderId)
        if folder == Self.defaultConfigDir(home: home), correctedFolders.contains(folder) {
            return defaultOwner
        }
        return identityId(for: folder)
    }

    /// The folders an id stands for: an identity's (run folders and
    /// stores), or the one folder.
    func folders(for id: String) -> [ClaudeAccount] {
        if let identity = identity(id: id) { return identity.folders }
        return account(id: id).map { [$0] } ?? []
    }

    /// Folders classified as infrastructure (the shared history), which the
    /// Superpowered Vibe Notch takeover still cleans.
    var infrastructureDirs: [String] { layout.infrastructure }

    /// Whether this account is the default one (~/.claude with CLAUDE_CONFIG_DIR unset).
    func isDefault(_ account: ClaudeAccount) -> Bool {
        Self.isDefault(account, home: home)
    }

    /// Sessions of this folder were seen with more than one CLAUDE_CONFIG_DIR
    /// spelling, which Claude Code treats as separate logins.
    /// (Even `/x` and `/x/` are two logins: the keychain item is keyed by the
    /// exact string.)
    func hasLoginConflict(_ account: ClaudeAccount) -> Bool {
        Set(account.seenConfigDirEnvs).count > 1
    }

    // MARK: - Discovery

    /// What a scan of the home folder found.
    nonisolated struct Discovery: Equatable, Sendable {
        /// Add these without asking (run folders and stores).
        var accounts: [String] = []
        /// Offer these.
        var suggestions: [AccountFolderSuggestion] = []
        /// What every folder found is for.
        var layout = ConfigDirLayout()
    }

    /// Scan the home folder for config dirs, add the ones clearly in use,
    /// refresh the suggestions, and refresh every account's identity. The
    /// file system work runs off the main actor.
    func discoverNow() async {
        guard mayReadHome else { return }
        let home = self.home
        let extras = extraConfigDirs
        let known = accounts.map(\.configDir)
        let previousStores = Set(layout.stores)
        let found = await Task.detached(priority: .utility) {
            Self.discover(home: home, extraDirs: extras, knownDirs: known, previousStores: previousStores)
        }.value

        applyLayout(found.layout)
        var changed = false
        var added: [ClaudeAccount] = []
        for dir in found.accounts {
            let id = AccountPaths.accountId(forConfigDir: dir)
            guard account(id: id) == nil, !removedIds.contains(id) else { continue }
            let isDefaultDir = dir == Self.defaultConfigDir(home: home)
            added.append(ClaudeAccount(
                configDir: dir,
                // Discovered custom dirs are used as CLAUDE_CONFIG_DIR=<path>;
                // a sighting replaces this with the exact string sessions use.
                configDirEnv: isDefaultDir ? nil : dir,
                colorIndex: Self.nextColorIndex(used: (accounts + added).map(\.colorIndex)),
                source: .discovered,
                kind: found.layout.kind(of: dir) ?? .run
            ))
            changed = true
            Self.logger.info("Discovered \(found.layout.kind(of: dir)?.rawValue ?? "run", privacy: .public) folder at \(dir, privacy: .public)")
        }
        if !added.isEmpty { publish(accounts + added) }
        discoveredSuggestions = found.suggestions
        refreshSuggestions()
        if changed { scheduleSave() }
        await refreshIdentities()
    }

    /// Config dirs to add without asking (see `discover`). Default first.
    nonisolated static func discoverConfigDirs(
        home: String,
        extraDirs: [String] = [],
        fileManager: FileManager = .default,
        isAlive: (Int32) -> Bool = AccountRegistry.isProcessAlive
    ) -> [String] {
        discover(home: home, extraDirs: extraDirs, fileManager: fileManager, isAlive: isAlive).accounts
    }

    /// Discovery from a snapshot that was already read (pure; see
    /// `discover(home:…)` for the rules).
    nonisolated static func discover(snapshot: ConfigDirSnapshot, extraDirs: [String] = [],
                                     previousStores: Set<String> = []) -> Discovery {
        let home = snapshot.home
        let layout = ConfigDirClassifier.classify(snapshot, previousStores: previousStores)
        var discovery = Discovery(layout: layout)
        let defaultDir = defaultConfigDir(home: home)
        if snapshot.folder(defaultDir) != nil {
            discovery.accounts.append(defaultDir)
        }
        var others: [String] = []
        var windows: [String] = []
        var stores: [String] = []
        let extras = Set(extraDirs.map(AccountPaths.normalize))
        for folder in snapshot.folders where folder.path != defaultDir {
            let path = folder.path
            guard canBeAccount(path, home: home) else { continue }
            switch layout.kind(of: path) {
            case .infrastructure, nil:
                continue
            case .store:
                // A store with an account in it; its identity is read.
                if folder.hasGlobalConfig { stores.append(path) }
            case .run:
                if ParallelProfiles.isWindowDir(path, home: home) {
                    // A VS Code window's working copy, once it has been stocked.
                    if folder.hasGlobalConfig { windows.append(path) }
                    continue
                }
                if extras.contains(path) {
                    others.append(path)
                    continue
                }
                let name = (path as NSString).lastPathComponent
                let isHomeLookalike = (path as NSString).deletingLastPathComponent == home
                    && (name.hasPrefix(".claude-") || name.hasPrefix(".claude_"))
                // Folders already known (by hand, from a hook) are classified
                // but not rediscovered.
                guard isHomeLookalike else { continue }
                let isClearlyConfigDir = folder.hasProjects || folder.hasSessions
                guard isClearlyConfigDir || folder.hasGlobalConfig else { continue }
                if looksLikeBackup(name) {
                    discovery.suggestions.append(AccountFolderSuggestion(configDir: path, reason: .looksLikeBackup))
                } else if folder.isSignedIn || folder.hasLiveSession {
                    others.append(path)
                } else {
                    discovery.suggestions.append(AccountFolderSuggestion(configDir: path, reason: .found))
                }
            }
        }
        discovery.accounts += others.sorted() + windows.sorted() + stores.sorted()
        return discovery
    }

    /// Scan `home`: ~/.claude when it exists; each ~/.claude-* or ~/.claude_*
    /// folder that is signed in or has a live session, unless named like a
    /// backup; and `extraDirs` that exist. Other folders with Claude Code's
    /// markers (projects/, sessions/, .claude.json) become suggestions: a
    /// copy of ~/.claude keeps the session files of whatever ran when it was
    /// made, but none of their processes. Never the home folder itself.
    /// Returned normalized, default first, then sorted. Reads `.claude.json`
    /// only for the `oauthAccount` key.
    nonisolated static func discover(
        home: String,
        extraDirs: [String] = [],
        knownDirs: [String] = [],
        previousStores: Set<String> = [],
        fileManager: FileManager = .default,
        isAlive: (Int32) -> Bool = AccountRegistry.isProcessAlive
    ) -> Discovery {
        let home = AccountPaths.normalize(home)
        let explicit = (extraDirs + knownDirs).map(AccountPaths.normalize).filter { canBeAccount($0, home: home) }
        let snapshot = ConfigDirClassifier.readSnapshot(home: home, explicitDirs: explicit + Array(previousStores),
                                                        fileManager: fileManager, isAlive: isAlive)
        return discover(snapshot: snapshot, extraDirs: extraDirs, previousStores: previousStores)
    }

    /// Words a backup copy's folder name usually has: `backup`, `bak`,
    /// `old`, `copy`, `orig`, `archive`, `tmp`, or a year or date.
    nonisolated static func looksLikeBackup(_ folderName: String) -> Bool {
        var name = folderName.lowercased()
        for prefix in [".claude-", ".claude_", "."] where name.hasPrefix(prefix) {
            name.removeFirst(prefix.count)
            break
        }
        let words = name.split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
        let backupWords: Set<String> = ["backup", "backups", "bak", "bkp", "old", "copy", "orig", "original",
                                        "archive", "archived", "tmp", "temp", "save", "saved", "prev", "previous"]
        if words.contains(where: { backupWords.contains($0) }) { return true }
        if words.contains(where: { $0.hasPrefix("backup") || $0.hasSuffix("backup") || $0.hasSuffix("bak") }) { return true }
        // A year (2019…2099), a yyyymmdd stamp, or a trailing number after a date-like word.
        return words.contains { word in
            guard word.allSatisfy(\.isNumber) else { return false }
            if word.count == 8 { return true }
            guard word.count == 4, let year = Int(word) else { return false }
            return (2019...2099).contains(year)
        }
    }

    /// What `path` holds. `.claude.json` is read (not parsed) only to see
    /// whether it has a login.
    nonisolated static func markers(of path: String, fileManager: FileManager = .default) -> AccountFolderMarkers {
        var markers = AccountFolderMarkers()
        markers.hasProjects = isDirectory((path as NSString).appendingPathComponent("projects"), fileManager: fileManager)
        markers.hasSessions = isDirectory((path as NSString).appendingPathComponent("sessions"), fileManager: fileManager)
        let globalConfig = (path as NSString).appendingPathComponent(".claude.json")
        markers.hasGlobalConfig = fileManager.fileExists(atPath: globalConfig)
        if markers.hasGlobalConfig {
            markers.isSignedIn = hasLogin(globalConfigAt: globalConfig)
        }
        return markers
    }

    /// Whether a `.claude.json` has an `oauthAccount` object, by scanning its
    /// bytes (it can be megabytes; a full parse isn't needed to know).
    nonisolated static func hasLogin(globalConfigAt path: String) -> Bool {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path), options: .mappedIfSafe) else { return false }
        let key = Data("\"oauthAccount\"".utf8)
        var searchStart = data.startIndex
        while let found = data.range(of: key, in: searchStart..<data.endIndex) {
            var index = found.upperBound
            func skipSpaces() {
                while index < data.endIndex, [0x20, 0x09, 0x0A, 0x0D].contains(data[index]) { index += 1 }
            }
            skipSpaces()
            if index < data.endIndex, data[index] == UInt8(ascii: ":") {
                index += 1
                skipSpaces()
                if index < data.endIndex, data[index] == UInt8(ascii: "{") {
                    index += 1
                    skipSpaces()
                    // `{}` is no login.
                    return index < data.endIndex && data[index] != UInt8(ascii: "}")
                }
            }
            searchStart = found.upperBound
        }
        return false
    }

    /// `sessions/` holds a `<pid>.json` whose process is alive (the file is
    /// never opened; `.key` files are ignored).
    nonisolated static func hasLiveSession(
        _ path: String,
        fileManager: FileManager = .default,
        isAlive: (Int32) -> Bool = AccountRegistry.isProcessAlive
    ) -> Bool {
        let sessions = (path as NSString).appendingPathComponent("sessions")
        return ((try? fileManager.contentsOfDirectory(atPath: sessions)) ?? []).contains { name in
            guard name.hasSuffix(".json"), let pid = Int32(name.dropLast(".json".count)), pid > 0 else { return false }
            return isAlive(pid)
        }
    }

    /// Whether process `pid` exists (someone else's counts too).
    nonisolated static func isProcessAlive(_ pid: Int32) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }

    nonisolated private static func isDirectory(_ path: String, fileManager: FileManager) -> Bool {
        var isDirectory: ObjCBool = false
        return fileManager.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    nonisolated static func defaultConfigDir(home: String) -> String {
        AccountPaths.normalize((home as NSString).appendingPathComponent(".claude"))
    }

    nonisolated static func isDefault(_ account: ClaudeAccount, home: String) -> Bool {
        (account.configDirEnv ?? "").isEmpty && account.configDir == defaultConfigDir(home: home)
    }

    /// The account's `.claude.json`, mirroring Claude Code:
    /// `join(CLAUDE_CONFIG_DIR || homedir, ".claude.json")`.
    nonisolated static func globalConfigPath(for account: ClaudeAccount, home: String) -> String {
        globalConfigPath(configDir: account.configDir, env: account.configDirEnv, home: home)
    }

    nonisolated static func globalConfigPath(configDir: String, env: String?, home: String) -> String {
        if let env, !env.isEmpty {
            return (AccountPaths.normalize(env) as NSString).appendingPathComponent(".claude.json")
        }
        if AccountPaths.normalize(configDir) == defaultConfigDir(home: home) {
            return (AccountPaths.normalize(home) as NSString).appendingPathComponent(".claude.json")
        }
        return (AccountPaths.normalize(configDir) as NSString).appendingPathComponent(".claude.json")
    }

    func globalConfigPath(for account: ClaudeAccount) -> String {
        Self.globalConfigPath(for: account, home: home)
    }

    // MARK: - Suggestions

    private func refreshSuggestions() {
        let known = Set(accounts.map(\.id))
        var list = discoveredSuggestions.filter { !known.contains($0.configDir) && !removedIds.contains($0.configDir) }
        for id in seenAgainIds.sorted() where !known.contains(id) {
            list.removeAll { $0.configDir == id }
            list.append(AccountFolderSuggestion(configDir: id, reason: .seenAgain))
        }
        if suggestions != list { suggestions = list }
    }

    /// Add a suggested folder as an account.
    @discardableResult
    func acceptSuggestion(_ configDir: String) throws -> ClaudeAccount {
        try addAccount(configDir: configDir)
    }

    /// Stop suggesting a folder (until the user adds it by hand).
    func dismissSuggestion(_ configDir: String) {
        let id = AccountPaths.accountId(forConfigDir: configDir)
        removedIds.insert(id)
        seenAgainIds.remove(id)
        refreshSuggestions()
        scheduleSave()
    }

    // MARK: - Identity

    /// Re-read every account's `.claude.json` (cheap when unchanged) and apply
    /// the signed-in identity. Refreshes run one after another, each from the
    /// accounts as they are when it starts, so the last one to finish is the
    /// newest. A folder used with two CLAUDE_CONFIG_DIR spellings moves to
    /// the one that is signed in, if the current one isn't.
    func refreshIdentities() async {
        let previous = identityRefreshTail
        let refresh = Task { [weak self] in
            await previous?.value
            await self?.performIdentityRefresh()
        }
        identityRefreshTail = refresh
        await refresh.value
    }

    private func performIdentityRefresh() async {
        // Two rounds at most: a move to the signed-in spelling, then its identity.
        for _ in 0..<2 {
            let home = self.home
            let targets = accounts.map { account in
                (account.id, globalConfigPath(for: account),
                 account.seenConfigDirEnvs.map { ($0, Self.globalConfigPath(configDir: account.configDir, env: $0, home: home)) })
            }
            let reader = configReader
            let results: [(String, ClaudeAccountIdentity?, String?, Date?)] = await Task.detached(priority: .utility) {
                targets.map { id, path, variants in
                    let config = reader.read(path: path)
                    let identity = config?.identity
                    // Only when the current spelling has no login: the first
                    // other spelling that does.
                    var better: String?
                    if identity == nil, variants.count > 1 {
                        better = variants.first { _, variantPath in
                            variantPath != path && reader.read(path: variantPath)?.identity != nil
                        }?.0
                    }
                    return (id, identity, better, config?.modifiedAt)
                }
            }.value
            var moved = false
            var updated = accounts
            identitiesRead = true
            for (id, identity, better, modifiedAt) in results {
                guard let index = updated.firstIndex(where: { $0.id == id }) else { continue }
                if isDefault(updated[index]) { defaultIdentityModifiedAt = modifiedAt }
                if let better {
                    updated[index].configDirEnv = better.isEmpty ? nil : better
                    moved = true
                    Self.logger.notice("\(id, privacy: .public) now follows its signed-in CLAUDE_CONFIG_DIR spelling")
                } else {
                    updated[index] = Self.applying(identity, to: updated[index])
                }
            }
            publish(updated)
            guard moved else { return }
            scheduleSave()
        }
    }

    /// Apply identity read from the account's `.claude.json` (nil = signed out),
    /// last written at `modifiedAt`. UsageStore calls this too, since it reads
    /// the same file more often.
    func applyIdentity(_ identity: ClaudeAccountIdentity?, forAccountId id: String, modifiedAt: Date? = nil) {
        guard let index = accounts.firstIndex(where: { $0.id == id }) else { return }
        identitiesRead = true
        if isDefault(accounts[index]), let modifiedAt { defaultIdentityModifiedAt = modifiedAt }
        let account = Self.applying(identity, to: accounts[index])
        guard account != accounts[index] else {
            if isDefault(account) { observeDefault() }
            return
        }
        var updated = accounts
        updated[index] = account
        publish(updated)
    }

    nonisolated private static func applying(_ identity: ClaudeAccountIdentity?, to account: ClaudeAccount) -> ClaudeAccount {
        var account = account
        account.email = identity?.email
        account.displayName = identity?.displayName
        account.organizationName = identity?.organizationName
        account.organizationUuid = identity?.organizationUuid
        account.accountUuid = identity?.accountUuid
        account.rateLimitTier = identity?.rateLimitTier
        if let subscription = identity?.subscriptionType {
            account.subscriptionType = subscription
        } else if identity == nil {
            account.subscriptionType = nil
        }
        return account
    }

    /// Record the plan reported by `get_usage` (e.g. "max") when `.claude.json`
    /// doesn't carry an organization type.
    func noteSubscriptionType(_ subscriptionType: String, forAccountId id: String) {
        guard !subscriptionType.isEmpty,
              let index = accounts.firstIndex(where: { $0.id == id }),
              accounts[index].subscriptionType != subscriptionType else { return }
        var updated = accounts
        updated[index].subscriptionType = subscriptionType
        publish(updated)
    }

    // MARK: - Sightings

    /// A hook or status line event came from this config dir.
    func record(_ sighting: AccountSighting) {
        let id = AccountPaths.accountId(forConfigDir: sighting.configDir)
        // A transcript path resolved through the shared history's links, or
        // the windows folder itself: never an account.
        if layout.kind(of: id) == .infrastructure
            || id == ParallelProfiles.sharedStore(home: home) || id == ParallelProfiles.windowsRoot(home: home) {
            return
        }
        let isDefaultDir = id == Self.defaultConfigDir(home: home)
        // What Claude Code saw: a raw CLAUDE_CONFIG_DIR, or "" for unset
        // (which only ever means ~/.claude).
        let rawEnv = sighting.configDirEnv.flatMap { $0.isEmpty ? nil : $0 }
        let variant: String? = rawEnv ?? (isDefaultDir ? "" : nil)

        guard let index = accounts.firstIndex(where: { $0.id == id }) else {
            guard Self.canBeAccount(id, home: home) else { return }
            if removedIds.contains(id) {
                // Forgotten on purpose: ask, don't re-add.
                if seenAgainIds.insert(id).inserted {
                    Self.logger.info("Forgotten account \(id, privacy: .public) seen again in a session")
                    refreshSuggestions()
                }
                return
            }
            insert(ClaudeAccount(
                configDir: sighting.configDir,
                configDirEnv: rawEnv ?? (isDefaultDir ? nil : sighting.configDir),
                seenConfigDirEnvs: variant.map { [$0] } ?? [],
                colorIndex: Self.nextColorIndex(used: accounts.map(\.colorIndex)),
                source: .hook,
                lastSeenAt: sighting.at
            ))
            scheduleSave()
            Self.logger.info("New account seen in a session: \(id, privacy: .public)")
            // Classify it (a window's working copy, a standalone folder) and
            // read who is signed in there.
            Task { await discoverNow() }
            return
        }

        var account = accounts[index]
        // Keep the raw CLAUDE_CONFIG_DIR verbatim: Claude Code hashes that exact
        // string to name the account's keychain item, so the usage probe must
        // run with the same one. The first spelling seen sticks; a second one
        // is remembered (a login conflict) and `refreshIdentities` moves to
        // it only if it is the one signed in.
        if let variant, !account.seenConfigDirEnvs.contains(variant) {
            if account.seenConfigDirEnvs.isEmpty {
                account.configDirEnv = variant.isEmpty ? nil : variant
            }
            account.seenConfigDirEnvs.append(variant)
        }
        if account.lastSeenAt.map({ sighting.at.timeIntervalSince($0) >= Self.sightingResolution }) ?? true {
            account.lastSeenAt = sighting.at
        }
        guard account != accounts[index] else { return }

        let envChanged = account.configDirEnv != accounts[index].configDirEnv
            || account.seenConfigDirEnvs.count != accounts[index].seenConfigDirEnvs.count
        var updated = accounts
        updated[index] = account
        publish(updated)
        scheduleSave()
        if envChanged {
            // The global config file moves with the env var.
            Task { await refreshIdentities() }
        }
    }

    /// The home folder, and anything holding it, can never be an account:
    /// not as written, and not through a symlink (`~/.claude-x` → `~`).
    nonisolated static func canBeAccount(_ configDir: String, home: String) -> Bool {
        let path = AccountPaths.normalize(configDir)
        let homePath = AccountPaths.normalize(home)
        func resolved(_ path: String) -> String { URL(fileURLWithPath: path).resolvingSymlinksInPath().path }
        return [(path, homePath), (resolved(path), resolved(homePath))].allSatisfy { folder, home in
            folder != home && !isAncestor(folder, of: home)
        }
    }

    nonisolated private static func isAncestor(_ ancestor: String, of path: String) -> Bool {
        ancestor == "/" ? path != "/" : path.hasPrefix(ancestor + "/")
    }

    // MARK: - User Actions

    /// Whether `path` can be added as an account, and what it holds. Throws
    /// for the home folder, a folder containing it or ~/.claude, a folder
    /// inside another account's config folder, and anything not a folder.
    func checkFolder(_ path: String) throws -> AccountFolderMarkers {
        let normalized = AccountPaths.normalize(path)
        if layout.kind(of: normalized) == .infrastructure
            || normalized == ParallelProfiles.sharedStore(home: home) || normalized == ParallelProfiles.windowsRoot(home: home) {
            throw AccountFolderError.infrastructure
        }
        return try Self.checkFolder(path, home: home, accounts: accounts)
    }

    nonisolated static func checkFolder(
        _ path: String,
        home: String,
        accounts: [ClaudeAccount],
        fileManager: FileManager = .default
    ) throws -> AccountFolderMarkers {
        let normalized = AccountPaths.normalize(path)
        let homePath = AccountPaths.normalize(home)
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: normalized, isDirectory: &isDirectory) else { throw AccountFolderError.missing }
        guard isDirectory.boolValue else { throw AccountFolderError.notAFolder }
        // Checked as picked and with links resolved: a link to home is home.
        func resolved(_ path: String) -> String { URL(fileURLWithPath: path).resolvingSymlinksInPath().path }
        for (folder, home) in [(normalized, homePath), (resolved(normalized), resolved(homePath))] {
            if folder == home { throw AccountFolderError.homeFolder }
            if isAncestor(folder, of: home) || isAncestor(folder, of: defaultConfigDir(home: home)) {
                throw AccountFolderError.containsAccounts
            }
        }
        let containers = accounts.map(\.configDir) + [defaultConfigDir(home: homePath)]
        if let container = containers.first(where: { isAncestor($0, of: normalized) }) {
            let label = accounts.first { $0.configDir == container }?.label ?? AccountPathDisplayName.abbreviated(container, home: homePath)
            throw AccountFolderError.insideAccount(label)
        }
        return Self.markers(of: normalized, fileManager: fileManager)
    }

    /// Add an existing config dir (checked with `checkFolder`). Returns the
    /// (possibly already known, now tracked again) account.
    @discardableResult
    func addAccount(configDir: String, configDirEnv: String? = nil) throws -> ClaudeAccount {
        _ = try checkFolder(configDir)
        let id = AccountPaths.accountId(forConfigDir: configDir)
        removedIds.remove(id)
        seenAgainIds.remove(id)
        // Adding a folder of a forgotten account brings the account back.
        if forgottenIdentityFolders.contains(id), let key = forgottenKey(ofFolder: id) {
            forgottenIdentityKeys.remove(key)
            publish(accounts)
        }

        if let existing = account(id: id) {
            if existing.isHidden {
                setHidden(id: id, false)
            }
            refreshSuggestions()
            return account(id: id) ?? existing
        }

        let normalized = AccountPaths.normalize(configDir)
        let isDefaultDir = normalized == Self.defaultConfigDir(home: home)
        let account = ClaudeAccount(
            configDir: normalized,
            configDirEnv: configDirEnv ?? (isDefaultDir ? nil : normalized),
            colorIndex: Self.nextColorIndex(used: accounts.map(\.colorIndex)),
            source: .manual
        )
        insert(account)
        refreshSuggestions()
        scheduleSave()
        Task { await refreshIdentities() }
        return self.account(id: id) ?? account
    }

    enum CreateAccountError: LocalizedError, Equatable {
        case invalidName
        case createFailed(String)
        /// `~/.claude-<name>` exists and isn't a Claude Code folder (another tool's?).
        case folderExistsNotClaude(String)

        var errorDescription: String? {
            switch self {
            case .invalidName:
                return "Use letters, numbers, dashes or underscores for the account name."
            case .createFailed(let reason):
                return "Couldn't create the account folder: \(reason)"
            case .folderExistsNotClaude(let folder):
                return "\(folder) already exists and isn't a Claude Code folder. Pick another name."
            }
        }
    }

    /// Create `~/.claude-<name>` for a new account and add it. The user then
    /// signs in by running the account's `launchCommand` and `/login`. An
    /// existing folder is adopted only if it already is a Claude Code folder.
    func createAccount(name: String) throws -> ClaudeAccount {
        guard let slug = Self.sanitizedAccountName(name) else {
            throw CreateAccountError.invalidName
        }
        let folder = ".claude-\(slug)"
        let path = (home as NSString).appendingPathComponent(folder)
        // Before bootstrap (tests, the snapshots tool) nothing is created in
        // the real home, whatever `home` this registry was given (S6).
        guard !HookInstaller.isProtectedBeforeBootstrap(configDir: path) else {
            throw CreateAccountError.createFailed("the engine isn't bootstrapped and \(folder) would be in your real home folder")
        }
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        if !fm.fileExists(atPath: path, isDirectory: &isDirectory) {
            do {
                try fm.createDirectory(
                    atPath: path,
                    withIntermediateDirectories: true,
                    attributes: [.posixPermissions: 0o700]
                )
            } catch {
                throw CreateAccountError.createFailed(error.localizedDescription)
            }
        } else if !isDirectory.boolValue {
            throw CreateAccountError.createFailed("a file named \(folder) already exists")
        } else {
            let markers = Self.markers(of: path)
            let hasSettings = fm.fileExists(atPath: (path as NSString).appendingPathComponent("settings.json"))
            guard markers.isClearlyConfigDir || markers.hasGlobalConfig || hasSettings else {
                throw CreateAccountError.folderExistsNotClaude("~/" + folder)
            }
        }

        var account = try addAccount(configDir: path, configDirEnv: AccountPaths.normalize(path))
        let label = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if account.customLabel == nil, !label.isEmpty {
            rename(id: account.id, label: label)
            account = self.account(id: account.id) ?? account
        }
        return account
    }

    /// Folder-safe version of a user-typed account name: letters, digits,
    /// `-`, `_` and `.`, with spaces turned into dashes. Nil if nothing is left.
    nonisolated static func sanitizedAccountName(_ name: String) -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
        var slug = ""
        for scalar in trimmed.unicodeScalars {
            if scalar.isASCII && allowed.contains(scalar) {
                slug.unicodeScalars.append(scalar)
            } else if CharacterSet.whitespaces.contains(scalar) {
                slug.append("-")
            }
        }
        while slug.hasPrefix(".") || slug.hasPrefix("-") { slug.removeFirst() }
        return slug.isEmpty ? nil : slug.lowercased()
    }

    /// Set (or with nil/empty, clear) an account's custom label: an
    /// identity's, or a folder's (and then its identity's too).
    func rename(id: String, label: String?) {
        let trimmed = label?.trimmingCharacters(in: .whitespacesAndNewlines)
        let newLabel = (trimmed?.isEmpty ?? true) ? nil : trimmed
        if let identityId = identityId(for: id), var prefs = identityPrefs[identityId], prefs.customLabel != newLabel {
            prefs.customLabel = newLabel
            identityPrefs[identityId] = prefs
        }
        var updated = accounts
        if let index = updated.firstIndex(where: { $0.id == id }) {
            updated[index].customLabel = newLabel
        }
        publish(updated)
        scheduleSave()
    }

    /// "Track sessions and hooks": hidden accounts get no hooks (the hook
    /// manager removes ours), sessions or probes. For an identity (or a
    /// folder of one) it applies to every folder of it, windows opened
    /// later included.
    func setHidden(id: String, _ hidden: Bool) {
        if let identityId = identityId(for: id), var prefs = identityPrefs[identityId] {
            guard prefs.isHidden != hidden else { return }
            prefs.isHidden = hidden
            identityPrefs[identityId] = prefs
            publish(accounts)
            scheduleSave()
            return
        }
        guard let index = accounts.firstIndex(where: { $0.id == id }),
              accounts[index].isHidden != hidden else { return }
        var updated = accounts
        updated[index].isHidden = hidden
        publish(updated)
        scheduleSave()
    }

    /// An account the user forgot (and hasn't added back): its sessions are
    /// shown nowhere and announce nothing, rather than landing on the default
    /// ring (BHV-3). Takes a folder id or an identity id.
    func isForgotten(_ id: String) -> Bool {
        if forgottenIdentityFolders.contains(id) { return true }
        if id.hasPrefix(AccountIdentityGrouping.uuidPrefix) || id.hasPrefix(AccountIdentityGrouping.emailPrefix)
            || id.hasPrefix(AccountIdentityGrouping.dirPrefix) {
            return forgottenIdentityKeys.contains(id)
        }
        return removedIds.contains(id) && account(id: id) == nil
    }

    /// Every forgotten folder id (see `isForgotten`).
    var forgottenIds: Set<String> {
        removedIds.filter { account(id: $0) == nil }.union(forgottenIdentityFolders)
    }

    /// Forget an account. Its folders are left alone; neither discovery nor
    /// a session re-adds it (a session only makes it a suggestion). Remove
    /// our hooks first (`AccountHookManager.forget`), or they keep firing
    /// there. An identity is forgotten as a whole: every folder of it, VS
    /// Code windows opened later included, until one of them is added back.
    func remove(id: String) {
        if let identity = identity(id: id) {
            forgottenIdentityKeys.insert(identity.id)
            if identity.isStandaloneUnsigned {
                // A folder added by hand: forgotten as that folder.
                for folder in identity.folders { removedIds.insert(folder.id) }
                publish(accounts.filter { !identity.folderIds.contains($0.id) })
            } else {
                publish(accounts)
            }
            refreshSuggestions()
            scheduleSave()
            return
        }
        guard accounts.contains(where: { $0.id == id }) else { return }
        publish(accounts.filter { $0.id != id })
        removedIds.insert(id)
        seenAgainIds.remove(id)
        refreshSuggestions()
        scheduleSave()
    }

    /// The forgotten identity a folder belongs to.
    private func forgottenKey(ofFolder id: String) -> String? {
        folderKeys[id].flatMap {
            forgottenIdentityKeys.contains($0) ? $0 : nil
        }
    }

    // MARK: - Ordering, names and colours

    private func insert(_ account: ClaudeAccount) {
        publish(accounts + [account])
    }

    /// Classify, group by identity, name (distinct default labels and
    /// badges) and sort, then publish folders and identities.
    private func publish(_ list: [ClaudeAccount]) {
        var folders = list.map { folder -> ClaudeAccount in
            var copy = folder
            if let kind = layout.kind(of: folder.configDir), kind != .infrastructure { copy.kind = kind }
            return copy
        }
        folders.removeAll { layout.kind(of: $0.configDir) == .infrastructure || $0.kind == .infrastructure }

        let grouping = AccountIdentityGrouping.group(folders, prefs: identityPrefs,
                                                     forgotten: forgottenIdentityKeys,
                                                     savedFolders: savedFolderIds,
                                                     mirrorsDefault: layout.extensionDetected, home: home)
        // Prefs are derived for good only once who is signed in where has
        // been read (before, every folder looks signed out).
        if identitiesRead, grouping.prefs != identityPrefs {
            let derived = Set(grouping.prefs.keys).subtracting(identityPrefs.keys)
            identityPrefs = grouping.prefs
            if !derived.isEmpty { scheduleSave() }
        }
        identityOfFolder = grouping.identityOfFolder
        folderKeys = grouping.folderKeys
        defaultOwner = grouping.defaultOwner
        correctedFolders = grouping.correctedFolders
        forgottenIdentityFolders = Set(grouping.forgottenFolders.map(\.id))
        // Tracking is an identity's choice: every folder of it follows (a
        // forgotten identity's folders are untracked).
        let hiddenByIdentity = Dictionary(grouping.identities.map { ($0.id, $0.isHidden) }, uniquingKeysWith: { first, _ in first })
        folders = folders.map { folder in
            var copy = folder
            if forgottenIdentityFolders.contains(folder.id) {
                copy.isHidden = true
            } else if let identityId = grouping.identityOfFolder[folder.id], let hidden = hiddenByIdentity[identityId] {
                copy.isHidden = hidden
            }
            return copy
        }
        // The grouping saw the folders before tracking was copied in.
        let identities = grouping.identities.map { identity -> ClaudeIdentityAccount in
            var copy = identity
            copy.runDirs = identity.runDirs.map { folder in folders.first { $0.id == folder.id } ?? folder }
            copy.storeDirs = identity.storeDirs.map { folder in folders.first { $0.id == folder.id } ?? folder }
            return copy
        }
        let named = Self.named(folders)
        let sorted = Self.sorted(named, home: home)
        if accounts != sorted { accounts = sorted }
        if self.identities != identities { self.identities = identities }
        let unsigned = grouping.unsignedFolders.map { folder in sorted.first { $0.id == folder.id } ?? folder }
        if unsignedFolders != unsigned { unsignedFolders = unsigned }
        observeDefault()
    }

    /// Note who `~/.claude` names now in its timeline (once identities were
    /// read), and publish what the socket server and mappers read.
    private func observeDefault() {
        let defaultId = Self.defaultConfigDir(home: home)
        if identitiesRead, !holdsFixtures, let folder = accounts.first(where: { $0.id == defaultId && isDefault($0) }) {
            let started = defaultTimeline.observe(folderKeys[defaultId], rawUuid: folder.accountUuid,
                                                  modifiedAt: defaultIdentityModifiedAt, at: clock(),
                                                  resumed: defaultTimelineResumed)
            defaultTimelineResumed = false
            if started { scheduleSave() }
        }
        guard self === AccountRegistry.sharedIfCreated else { return }
        var snapshot = FolderRings.Snapshot()
        for identity in identities {
            snapshot.ringOfIdentity[identity.id] = identity.ringID
            for folder in identity.folders { snapshot.rings[folder.id] = identity.ringID }
        }
        snapshot.identityOfFolder = folderKeys.merging(identityOfFolder) { key, _ in key }
        snapshot.untrackedIdentities = Set(identities.filter(\.isHidden).map(\.id)).union(forgottenIdentityKeys)
        snapshot.untrackedFolders = Set(accounts.filter(\.isHidden).map(\.id))
        snapshot.defaultFolder = defaultId
        snapshot.mirrorsDefault = layout.extensionDetected
        snapshot.defaultTimeline = defaultTimeline
        FolderRings.set(snapshot)
    }

    /// Every account with its default label and badge among `accounts`
    /// (only tracked accounts count for collisions; an untracked one is
    /// named as if it joined them).
    nonisolated static func named(_ accounts: [ClaudeAccount]) -> [ClaudeAccount] {
        let tracked = accounts.filter { !$0.isHidden }
        var names = AccountNaming.assign(tracked)
        for account in accounts where account.isHidden {
            names[account.id] = AccountNaming.assign(tracked + [account])[account.id]
        }
        return accounts.map { account in
            var copy = account
            copy.defaultLabel = names[account.id]?.label
            copy.defaultMonogram = names[account.id]?.monogram
            return copy
        }
    }

    /// Default account first, then by label (case-insensitive), then by path.
    nonisolated static func sorted(_ accounts: [ClaudeAccount], home: String) -> [ClaudeAccount] {
        accounts.sorted { lhs, rhs in
            let lhsDefault = isDefault(lhs, home: home)
            let rhsDefault = isDefault(rhs, home: home)
            if lhsDefault != rhsDefault { return lhsDefault }
            switch lhs.label.localizedCaseInsensitiveCompare(rhs.label) {
            case .orderedAscending: return true
            case .orderedDescending: return false
            case .orderedSame: return lhs.id < rhs.id
            }
        }
    }

    /// The first palette index nobody uses; once all are taken, the least
    /// used one (lowest on ties), so colours stay as distinct as possible.
    nonisolated static func nextColorIndex(used: [Int]) -> Int {
        var counts = [Int](repeating: 0, count: paletteSize)
        for index in used {
            counts[((index % paletteSize) + paletteSize) % paletteSize] += 1
        }
        let minimum = counts.min() ?? 0
        return counts.firstIndex(of: minimum) ?? 0
    }

    // MARK: - Persistence

    nonisolated private struct PersistedAccount: Codable {
        var id: String
        var configDir: String
        var configDirEnv: String?
        var customLabel: String?
        var colorIndex: Int
        var isHidden: Bool
        var source: AccountSource
        var lastSeenAt: Date?
        /// Added after Superpowered Vibe Notch; absent in its files.
        var seenConfigDirEnvs: [String]?
    }

    nonisolated private struct PersistedState: Codable {
        var version = 2
        var accounts: [PersistedAccount]
        var removedIds: [String]
        /// Per identity (`uuid:…`, `email:…`, `dir:…`): name, colour and
        /// tracking. Absent before accounts were identities (version 1).
        var identities: [String: IdentityPrefs]?
        var forgottenIdentities: [String]?
        /// Who `~/.claude` ran as over time (absent before it was kept).
        var defaultIdentityTimeline: FolderIdentityTimeline?
    }

    private nonisolated static var decoder: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }

    /// Whether `data` is an accounts.json this registry can load (the
    /// Superpowered Vibe Notch import checks before copying).
    nonisolated static func canLoad(_ data: Data) -> Bool {
        (try? decoder.decode(PersistedState.self, from: data)) != nil
    }

    private func loadPersisted() {
        guard let data = try? Data(contentsOf: storeURL) else { return }
        guard let state = try? Self.decoder.decode(PersistedState.self, from: data) else {
            Self.logger.error("accounts.json is unreadable; starting from discovery")
            return
        }
        removedIds = Set(state.removedIds)
        identityPrefs = state.identities ?? [:]
        forgottenIdentityKeys = Set(state.forgottenIdentities ?? [])
        defaultTimeline = state.defaultIdentityTimeline ?? FolderIdentityTimeline()
        var seen = Set<String>()
        var loaded: [ClaudeAccount] = []
        for stored in state.accounts {
            guard Self.canBeAccount(stored.configDir, home: home) else { continue }
            let account = ClaudeAccount(
                configDir: stored.configDir,
                configDirEnv: stored.configDirEnv,
                customLabel: stored.customLabel,
                seenConfigDirEnvs: stored.seenConfigDirEnvs ?? [],
                colorIndex: stored.colorIndex,
                source: stored.source,
                lastSeenAt: stored.lastSeenAt,
                isHidden: stored.isHidden
            )
            guard seen.insert(account.id).inserted else { continue }
            loaded.append(account)
        }
        savedFolderIds = seen
        publish(loaded)
    }

    private func scheduleSave() {
        guard !holdsFixtures else { return }
        saveTask?.cancel()
        saveTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            self?.saveNow()
        }
    }

    /// Write accounts.json now (also cancels a pending debounced save).
    func saveNow() {
        saveTask?.cancel()
        saveTask = nil
        guard !holdsFixtures else { return }
        let state = PersistedState(
            accounts: accounts.map {
                PersistedAccount(
                    id: $0.id,
                    configDir: $0.configDir,
                    configDirEnv: $0.configDirEnv,
                    customLabel: $0.customLabel,
                    colorIndex: $0.colorIndex,
                    isHidden: $0.isHidden,
                    source: $0.source,
                    lastSeenAt: $0.lastSeenAt,
                    seenConfigDirEnvs: $0.seenConfigDirEnvs.isEmpty ? nil : $0.seenConfigDirEnvs
                )
            },
            removedIds: removedIds.sorted(),
            identities: identityPrefs.isEmpty ? nil : identityPrefs,
            forgottenIdentities: forgottenIdentityKeys.isEmpty ? nil : forgottenIdentityKeys.sorted(),
            defaultIdentityTimeline: defaultTimeline.spans.isEmpty ? nil : defaultTimeline
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        encoder.dateEncodingStrategy = .iso8601
        do {
            let data = try encoder.encode(state)
            try FileManager.default.createDirectory(
                at: storeURL.deletingLastPathComponent(),
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            try data.write(to: storeURL, options: .atomic)
        } catch {
            Self.logger.error("Couldn't save accounts.json: \(error.localizedDescription, privacy: .public)")
        }
    }
}
