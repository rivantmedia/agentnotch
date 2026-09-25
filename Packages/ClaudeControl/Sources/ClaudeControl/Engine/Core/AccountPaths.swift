//
//  AccountPaths.swift
//  ClaudeControl
//
//  Pure path helpers for mapping Claude Code config directories (one per
//  account) to stable account IDs. Safe to call from any actor or thread.
//
//  Background (Claude Code 2.1.x):
//  - Each account lives in its own config dir, selected with CLAUDE_CONFIG_DIR.
//    Unset means ~/.claude.
//  - Transcripts are always `<configDir>/projects/<slug>/<sessionId>.jsonl`, so
//    the hook payload's `transcript_path` identifies the account.
//  - The account's global config (`oauthAccount`, `cachedUsageUtilization`) is
//    `~/.claude.json` for the default dir, but `<configDir>/.claude.json` when
//    CLAUDE_CONFIG_DIR is set.
//

import Foundation

nonisolated enum AccountPaths {
    /// The home folder from the frozen configuration (a temporary one when sealed).
    static var homeDirectory: String {
        AppIdentity.homeDirectory
    }

    /// ~/.claude, the directory Claude Code uses when CLAUDE_CONFIG_DIR is unset.
    static var defaultConfigDir: String {
        normalize("~/.claude")
    }

    /// Expands `~` (to `homeDirectory`), resolves `.`/`..` and drops any
    /// trailing slash. Symlinks are NOT resolved, so the result stays
    /// recognisable to the user.
    static func normalize(_ path: String) -> String {
        let expanded: String
        if path == "~" {
            expanded = homeDirectory
        } else if path.hasPrefix("~/") {
            expanded = (homeDirectory as NSString).appendingPathComponent(String(path.dropFirst(2)))
        } else {
            expanded = (path as NSString).expandingTildeInPath
        }
        var standardized = (expanded as NSString).standardizingPath
        while standardized.count > 1 && standardized.hasSuffix("/") {
            standardized.removeLast()
        }
        return standardized
    }

    /// Stable account key for a config dir: its normalized absolute path.
    static func accountId(forConfigDir configDir: String) -> String {
        normalize(configDir)
    }

    /// Whether this config dir is the default one (~/.claude).
    static func isDefaultConfigDir(_ configDir: String) -> Bool {
        normalize(configDir) == defaultConfigDir
    }

    /// Config dir for a main-session transcript path, i.e. the part before
    /// `/projects/<slug>/<file>.jsonl`. Returns nil if the path doesn't have that shape.
    static func configDir(fromTranscriptPath transcriptPath: String) -> String? {
        let components = (normalize(transcriptPath) as NSString).pathComponents
        // Search from the end so a config dir that itself contains "projects"
        // still resolves correctly. Main transcripts are exactly two levels
        // below `projects`; subagent transcripts are deeper (`<sid>/subagents/...`).
        guard components.count >= 4 else { return nil }
        for index in stride(from: components.count - 3, through: 1, by: -1) where components[index] == "projects" {
            let prefix = Array(components[0..<index])
            guard !prefix.isEmpty else { return nil }
            return normalize(NSString.path(withComponents: prefix))
        }
        return nil
    }

    /// Account ID for a transcript path, if it can be derived.
    static func accountId(forTranscriptPath transcriptPath: String) -> String? {
        configDir(fromTranscriptPath: transcriptPath).map(accountId(forConfigDir:))
    }

    /// Path of the account's global config file (`oauthAccount`, `cachedUsageUtilization`).
    /// Mirrors Claude Code: `join(CLAUDE_CONFIG_DIR || homedir, ".claude.json")`.
    static func globalConfigFile(configDir: String, configDirEnv: String?) -> String {
        if let env = configDirEnv, !env.isEmpty {
            return (normalize(env) as NSString).appendingPathComponent(".claude.json")
        }
        if isDefaultConfigDir(configDir) {
            return (homeDirectory as NSString).appendingPathComponent(".claude.json")
        }
        return (normalize(configDir) as NSString).appendingPathComponent(".claude.json")
    }

    private static let infrastructureLock = NSLock()
    nonisolated(unsafe) private static var knownInfrastructure: Set<String> = []

    /// Folders the registry classified as infrastructure (the shared
    /// history): a transcript path through one names no account.
    static func setInfrastructureDirs(_ dirs: [String]) {
        infrastructureLock.lock()
        knownInfrastructure = Set(dirs.map(normalize))
        infrastructureLock.unlock()
    }

    /// `~/.claude-shared`, `~/.claude-windows`, or a folder the registry
    /// classified as infrastructure: never an account.
    static func isInfrastructureDir(_ dir: String) -> Bool {
        let path = normalize(dir)
        if path == ParallelProfiles.sharedStore(home: homeDirectory) || path == ParallelProfiles.windowsRoot(home: homeDirectory) {
            return true
        }
        infrastructureLock.lock()
        defer { infrastructureLock.unlock() }
        return knownInfrastructure.contains(path)
    }

    /// Whether `home` is the real home folder of the user running this
    /// process while the engine isn't bootstrapped (tests, the snapshots
    /// tool): nothing there is read or written then.
    static func isRealHomeBeforeBootstrap(_ home: String) -> Bool {
        guard !AppIdentity.isFrozen, let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir else { return false }
        let realHome = (String(cString: dir) as NSString).standardizingPath
        let path = (normalize(home) as NSString).standardizingPath
        return path == realHome || URL(fileURLWithPath: path).resolvingSymlinksInPath().path == realHome
    }

    /// Short, human-readable name for a config dir, e.g. `.claude-work`.
    static func shortName(forConfigDir configDir: String) -> String {
        (normalize(configDir) as NSString).lastPathComponent
    }
}
