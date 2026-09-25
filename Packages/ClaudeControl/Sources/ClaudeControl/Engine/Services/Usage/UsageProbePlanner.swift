//
//  UsageProbePlanner.swift
//  ClaudeControl
//
//  Where an account's usage check runs. The check (`claude -p`, see
//  `UsageProbe`) runs Claude Code itself, which may refresh the folder's
//  token and rewrite its `.claude.json`, so it runs once per account, and
//  only in a folder Claude Code runs in as that account anyway: never in a
//  Claude Parallel Profiles store (the extension keeps stores as its own
//  copy of an account; a store must not change behind its back). Pure.
//

import Foundation

nonisolated enum UsageProbePlanner {
    /// The run folder to check an identity's usage in: the most recently
    /// active one (a session seen there, or Claude Code writing its config),
    /// else `~/.claude` when it runs as this identity, else the first. Nil
    /// when the identity runs nowhere (only its store holds it), and never a
    /// store.
    ///
    /// - Parameters:
    ///   - activity: when each of `runDirs` was last active, if known.
    ///   - prefersOwnFolders: Claude Parallel Profiles mirrors the focused
    ///     window's account into `~/.claude`, so `~/.claude` may change hands
    ///     at any moment (and its `.claude.json` is rewritten with every
    ///     mirror): its VS Code windows and standalone folders come first,
    ///     `~/.claude` only when it has none.
    static func probeFolder(runDirs: [ClaudeAccount], activity: [Date?], home: String,
                            prefersOwnFolders: Bool = false) -> ClaudeAccount? {
        var candidates = runDirs.enumerated().filter { $0.element.kind == .run }
        if prefersOwnFolders, candidates.contains(where: { !AccountRegistry.isDefault($0.element, home: home) }) {
            candidates.removeAll { AccountRegistry.isDefault($0.element, home: home) }
        }
        guard !candidates.isEmpty else { return nil }
        let active = candidates.compactMap { index, folder -> (ClaudeAccount, Date)? in
            guard index < activity.count, let date = activity[index] else { return nil }
            return (folder, date)
        }
        if let newest = active.max(by: { lhs, rhs in
            if lhs.1 != rhs.1 { return lhs.1 < rhs.1 }
            // Equal times: the default folder, then the first by path.
            let lhsDefault = AccountRegistry.isDefault(lhs.0, home: home)
            let rhsDefault = AccountRegistry.isDefault(rhs.0, home: home)
            if lhsDefault != rhsDefault { return rhsDefault }
            return lhs.0.configDir > rhs.0.configDir
        }) {
            return newest.0
        }
        if let defaultFolder = candidates.first(where: { AccountRegistry.isDefault($0.element, home: home) }) {
            return defaultFolder.element
        }
        return candidates.first?.element
    }
}
