//
//  ConfigDirClassifier.swift
//  ClaudeControl
//
//  What each Claude Code config folder on this Mac is for, so an account is
//  a signed-in identity and not a folder. Claude Parallel Profiles (the VS
//  Code extension, a fork of "Claude Parallel Accounts") deliberately makes
//  many folders per account:
//
//  - `~/.claude-<name>`: an account *store* the extension created (holding a
//    `.parallel-accounts-store` marker, listed in the manifest's `created`).
//    Claude Code never runs in one: a window copies its `.claude.json` and
//    token and runs on the copy. A signed-in `~/.claude-<name>` profile the
//    user made (and runs `CLAUDE_CONFIG_DIR=… claude` in) is *adopted*: the
//    manifest lists it under `stores` but not `created`, no marker is
//    written, and it stays the user's run folder.
//  - `~/.claude-windows/<12 hex>/`: a VS Code window's working copy. Claude
//    Code runs here with `CLAUDE_CONFIG_DIR` set to it.
//  - `~/.claude-windows/.manifest.json`: `{"stores": […], "created": […]}`.
//  - `~/.claude-shared`: the one conversation history, whose `projects`,
//    `sessions`, `plans`, … are symlinked into every folder above
//    (including `~/.claude`). Not an account at all.
//
//  So a folder is one of three kinds (`ConfigDirKind`): Claude Code *runs*
//  there (hooks and the status line go there, usage checks run there), it is
//  a *store* (its identity is read, nothing is ever written or run there), or
//  it is *infrastructure* (never an account). The rules are a pure function
//  of a directory snapshot, and work the same without the extension (plain
//  `CLAUDE_CONFIG_DIR` users simply have run folders).
//
//  Reading the snapshot is read-only: directory listings, symbolic link
//  targets, the manifest, whether the marker exists, and whether a
//  `.claude.json` names a login (a byte scan for `oauthAccount`).
//

import Foundation

/// What a Claude Code config folder is for.
nonisolated enum ConfigDirKind: String, Codable, Hashable, Sendable {
    /// Claude Code runs here: `~/.claude`, a VS Code window's working copy,
    /// or a standalone folder. Hooks, the status line and usage checks go here.
    case run
    /// An account store Claude Parallel Profiles created (its marker, or
    /// listed in the manifest's `created`). Its identity is read (an account
    /// with no open window still has a ring and usage), but nothing is ever
    /// installed, run or written there.
    case store
    /// Not an account: the shared history (`~/.claude-shared`, or any folder
    /// other folders' `projects`/`sessions` link into) and `~/.claude-windows`.
    case infrastructure
}

/// Names and places Claude Parallel Profiles uses.
nonisolated enum ParallelProfiles {
    static let windowsFolderName = ".claude-windows"
    static let sharedFolderName = ".claude-shared"
    static let storeMarkerName = ".parallel-accounts-store"
    static let manifestName = ".manifest.json"
    static let displayName = "Claude Parallel Profiles"

    static func windowsRoot(home: String) -> String {
        (AccountPaths.normalize(home) as NSString).appendingPathComponent(windowsFolderName)
    }

    static func sharedStore(home: String) -> String {
        (AccountPaths.normalize(home) as NSString).appendingPathComponent(sharedFolderName)
    }

    static func manifestPath(home: String) -> String {
        (windowsRoot(home: home) as NSString).appendingPathComponent(manifestName)
    }

    /// A VS Code window's working copy: a folder right inside `~/.claude-windows`.
    static func isWindowDir(_ path: String, home: String) -> Bool {
        let normalized = AccountPaths.normalize(path)
        let name = (normalized as NSString).lastPathComponent
        return (normalized as NSString).deletingLastPathComponent == windowsRoot(home: home)
            && !name.hasPrefix(".") && !name.isEmpty
    }
}

/// `~/.claude-windows/.manifest.json`, the extension's list of the stores it
/// manages. Only its folder lists are read.
nonisolated struct ParallelProfilesManifest: Equatable, Sendable {
    /// Every account store (normalized).
    var stores: [String]
    /// The stores the extension created itself (the rest it adopted).
    var created: [String]

    static func parse(_ data: Data) -> ParallelProfilesManifest? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        func paths(_ key: String) -> [String] {
            ((json[key] as? [Any]) ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty }.map(AccountPaths.normalize)
        }
        return ParallelProfilesManifest(stores: paths("stores"), created: paths("created"))
    }

    static func read(home: String, fileManager: FileManager = .default) -> ParallelProfilesManifest? {
        guard let data = fileManager.contents(atPath: ParallelProfiles.manifestPath(home: home)) else { return nil }
        return parse(data)
    }

    /// The manifest exists but can't be parsed (caught mid-write: the
    /// extension writes it in place). What it said last still stands then.
    static func isUnreadable(home: String, fileManager: FileManager = .default) -> Bool {
        let path = ParallelProfiles.manifestPath(home: home)
        guard fileManager.fileExists(atPath: path) else { return false }
        guard let data = fileManager.contents(atPath: path) else { return true }
        return parse(data) == nil
    }

    /// A store the extension made: never run, hooked or probed. The other
    /// folders in `stores` are profiles it adopted, which the user runs.
    func isCreatedStore(_ path: String) -> Bool {
        created.contains(AccountPaths.normalize(path))
    }
}

/// The facts classification needs, read once from disk.
nonisolated struct ConfigDirSnapshot: Equatable, Sendable {
    struct Folder: Equatable, Sendable {
        var path: String
        /// `<folder>/.claude.json` exists.
        var hasGlobalConfig = false
        /// Its identity file (`~/.claude.json` for the default folder) names
        /// a claude.ai login.
        var isSignedIn = false
        /// `<folder>/.parallel-accounts-store` exists.
        var hasStoreMarker = false
        var hasProjects = false
        var hasSessions = false
        /// `sessions/` holds a `<pid>.json` whose process runs (and isn't a
        /// link into the shared history, where every window's sessions are).
        var hasLiveSession = false
        /// Where `projects` / `sessions` point when they are symbolic links:
        /// resolved, absolute paths.
        var linkTargets: [String] = []
        /// Known already (the registry, a hook, `AGENTNOTCH_EXTRA_CONFIG_DIRS`):
        /// classified, but not subject to discovery's rules.
        var isExplicit = false

        init(path: String) {
            self.path = AccountPaths.normalize(path)
        }
    }

    var home: String
    var folders: [Folder]
    var manifest: ParallelProfilesManifest?
    /// The manifest exists but couldn't be parsed (a write in progress).
    var manifestUnreadable = false

    init(home: String, folders: [Folder], manifest: ParallelProfilesManifest? = nil, manifestUnreadable: Bool = false) {
        self.home = AccountPaths.normalize(home)
        self.folders = folders
        self.manifest = manifest
        self.manifestUnreadable = manifestUnreadable
    }

    func folder(_ path: String) -> Folder? {
        let normalized = AccountPaths.normalize(path)
        return folders.first { $0.path == normalized }
    }
}

/// The classification of every folder in a snapshot.
nonisolated struct ConfigDirLayout: Equatable, Sendable {
    var home: String
    var kinds: [String: ConfigDirKind]
    /// The manifest or a store marker was found.
    var extensionDetected: Bool
    /// Run folders Claude Parallel Profiles adopted as account sources (in
    /// its manifest's `stores`, not made by it): the user's own profiles,
    /// which it only copies from. Settings says so beside them.
    var adoptedByExtension: Set<String> = []

    init(home: String = "", kinds: [String: ConfigDirKind] = [:], extensionDetected: Bool = false,
         adoptedByExtension: Set<String> = []) {
        self.home = home
        self.kinds = kinds
        self.extensionDetected = extensionDetected
        self.adoptedByExtension = adoptedByExtension
    }

    func kind(of path: String) -> ConfigDirKind? {
        kinds[AccountPaths.normalize(path)]
    }

    private func paths(_ kind: ConfigDirKind) -> [String] {
        kinds.filter { $0.value == kind }.map(\.key).sorted()
    }

    var runDirs: [String] { paths(.run) }
    var stores: [String] { paths(.store) }
    var infrastructure: [String] { paths(.infrastructure) }
    var windowDirs: [String] { runDirs.filter { ParallelProfiles.isWindowDir($0, home: home) } }
}

nonisolated enum ConfigDirClassifier {
    /// Folders whose links make another folder the shared history.
    static let sharedEntryNames = ["projects", "sessions"]

    /// Classify every folder of `snapshot` (pure):
    /// 1. `~/.claude-windows` and `~/.claude-shared` are infrastructure;
    /// 2. a folder holding the store marker, or listed in the manifest's
    ///    `created`, is a store (one the extension made). A folder only in its
    ///    `stores` is a profile it adopted: the user runs Claude Code there,
    ///    so it stays a run folder (flagged in `adoptedByExtension`);
    /// 3. a folder other folders' `projects`/`sessions` link into is
    ///    infrastructure, unless it is `~/.claude` or signed in itself (a
    ///    user who links a profile's history to `~/.claude` still runs there);
    /// 4. everything else is a run folder: `~/.claude`, every window's working
    ///    copy, and standalone folders.
    ///
    /// - Parameter previousStores: the stores of the last classification.
    ///   While the manifest can't be read (caught mid-write), they stay
    ///   stores: an unreadable manifest never turns a store into a folder
    ///   Claude Code could be run in.
    static func classify(_ snapshot: ConfigDirSnapshot, previousStores: Set<String> = []) -> ConfigDirLayout {
        let home = snapshot.home
        let defaultDir = AccountRegistry.defaultConfigDir(home: home)
        let fixedInfrastructure: Set<String> = [
            ParallelProfiles.windowsRoot(home: home),
            ParallelProfiles.sharedStore(home: home),
        ]
        let createdStores = Set(snapshot.manifest?.created ?? [])
        let listedStores = Set(snapshot.manifest?.stores ?? [])
        let stickyStores = snapshot.manifestUnreadable ? previousStores : []
        // Owners of link targets: `~/.claude-shared/projects` → `~/.claude-shared`.
        var linkOwners = Set<String>()
        for folder in snapshot.folders {
            for target in folder.linkTargets {
                let normalized = AccountPaths.normalize(target)
                guard sharedEntryNames.contains((normalized as NSString).lastPathComponent) else { continue }
                let owner = (normalized as NSString).deletingLastPathComponent
                if owner != folder.path { linkOwners.insert(owner) }
            }
        }

        var kinds: [String: ConfigDirKind] = [:]
        var adopted = Set<String>()
        var markerFound = false
        for folder in snapshot.folders {
            let path = folder.path
            guard path != home else { continue }
            if folder.hasStoreMarker { markerFound = true }
            if fixedInfrastructure.contains(path) {
                kinds[path] = .infrastructure
            } else if folder.hasStoreMarker || createdStores.contains(path) || stickyStores.contains(path) {
                kinds[path] = .store
            } else if linkOwners.contains(path), path != defaultDir, !folder.isSignedIn {
                kinds[path] = .infrastructure
            } else {
                kinds[path] = .run
                if listedStores.contains(path), path != defaultDir,
                   !ParallelProfiles.isWindowDir(path, home: home) { adopted.insert(path) }
            }
        }
        return ConfigDirLayout(home: home, kinds: kinds,
                               extensionDetected: snapshot.manifest != nil || snapshot.manifestUnreadable || markerFound,
                               adoptedByExtension: adopted)
    }

    // MARK: - Reading the snapshot (read-only)

    /// Everything classification and discovery need, from disk: `~/.claude`,
    /// every `~/.claude-*` / `~/.claude_*` folder, every window's working
    /// copy, the manifest's stores, and `explicitDirs` (folders already known).
    /// Reads listings, link targets, the manifest, the marker's existence and
    /// a byte scan of `.claude.json` for a login; never a credential.
    static func readSnapshot(
        home: String,
        explicitDirs: [String] = [],
        fileManager: FileManager = .default,
        isAlive: (Int32) -> Bool = AccountRegistry.isProcessAlive
    ) -> ConfigDirSnapshot {
        let home = AccountPaths.normalize(home)
        var paths: [String] = []
        func add(_ path: String) {
            let normalized = AccountPaths.normalize(path)
            if !paths.contains(normalized) { paths.append(normalized) }
        }
        let defaultDir = AccountRegistry.defaultConfigDir(home: home)
        if isDirectory(defaultDir, fileManager) { add(defaultDir) }
        for name in ((try? fileManager.contentsOfDirectory(atPath: home)) ?? []).sorted()
        where name.hasPrefix(".claude-") || name.hasPrefix(".claude_") {
            let path = (home as NSString).appendingPathComponent(name)
            if isDirectory(path, fileManager) { add(path) }
        }
        let windowsRoot = ParallelProfiles.windowsRoot(home: home)
        for name in ((try? fileManager.contentsOfDirectory(atPath: windowsRoot)) ?? []).sorted() where !name.hasPrefix(".") {
            let path = (windowsRoot as NSString).appendingPathComponent(name)
            if isDirectory(path, fileManager) { add(path) }
        }
        let manifest = ParallelProfilesManifest.read(home: home, fileManager: fileManager)
        let manifestUnreadable = manifest == nil && ParallelProfilesManifest.isUnreadable(home: home, fileManager: fileManager)
        for store in manifest?.stores ?? [] where isDirectory(store, fileManager) { add(store) }
        let explicit = Set(explicitDirs.map(AccountPaths.normalize))
        for dir in explicitDirs where isDirectory(AccountPaths.normalize(dir), fileManager) { add(dir) }

        let folders = paths.map { path -> ConfigDirSnapshot.Folder in
            var folder = folderFacts(path, home: home, fileManager: fileManager, isAlive: isAlive)
            folder.isExplicit = explicit.contains(path)
            return folder
        }
        return ConfigDirSnapshot(home: home, folders: folders, manifest: manifest, manifestUnreadable: manifestUnreadable)
    }

    /// One folder's facts.
    static func folderFacts(
        _ path: String,
        home: String,
        fileManager: FileManager = .default,
        isAlive: (Int32) -> Bool = AccountRegistry.isProcessAlive
    ) -> ConfigDirSnapshot.Folder {
        var folder = ConfigDirSnapshot.Folder(path: path)
        let path = folder.path
        let globalConfig = (path as NSString).appendingPathComponent(".claude.json")
        folder.hasGlobalConfig = fileManager.fileExists(atPath: globalConfig)
        // The default folder's login is in ~/.claude.json, beside it.
        let identityFile = path == AccountRegistry.defaultConfigDir(home: home)
            ? (AccountPaths.normalize(home) as NSString).appendingPathComponent(".claude.json")
            : globalConfig
        folder.isSignedIn = fileManager.fileExists(atPath: identityFile)
            && AccountRegistry.hasLogin(globalConfigAt: identityFile)
        folder.hasStoreMarker = fileManager.fileExists(atPath: (path as NSString).appendingPathComponent(ParallelProfiles.storeMarkerName))
        folder.hasProjects = isDirectory((path as NSString).appendingPathComponent("projects"), fileManager)
        folder.hasSessions = isDirectory((path as NSString).appendingPathComponent("sessions"), fileManager)
        var sessionsAreShared = false
        for name in sharedEntryNames {
            let entry = (path as NSString).appendingPathComponent(name)
            guard let destination = try? fileManager.destinationOfSymbolicLink(atPath: entry) else { continue }
            let absolute = destination.hasPrefix("/")
                ? destination
                : ((path as NSString).appendingPathComponent(destination) as NSString).standardizingPath
            folder.linkTargets.append(AccountPaths.normalize(absolute))
            if name == "sessions" { sessionsAreShared = true }
        }
        if folder.hasSessions, !sessionsAreShared {
            folder.hasLiveSession = AccountRegistry.hasLiveSession(path, fileManager: fileManager, isAlive: isAlive)
        }
        return folder
    }

    private static func isDirectory(_ path: String, _ fileManager: FileManager) -> Bool {
        var isDirectory: ObjCBool = false
        return fileManager.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
    }
}
