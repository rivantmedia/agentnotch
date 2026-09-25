//
//  RegistryDirsBridge.swift
//  ClaudeIsland
//
//  Keeps the session registry scanner pointed at every visible account's
//  config dir, so sessions started before the app (or without hooks, e.g.
//  in VS Code) show up for all accounts, not only ~/.claude.
//

import Combine
import Foundation

@MainActor
final class RegistryDirsBridge {
    static let shared = RegistryDirsBridge()

    private var cancellable: AnyCancellable?

    private init() {}

    /// Starts following AccountRegistry. Idempotent.
    func start() {
        guard cancellable == nil else { return }
        cancellable = AccountRegistry.shared.$accounts
            .map { Self.configDirs(for: $0) }
            .removeDuplicates()
            .sink { dirs in
                SessionRegistryScanner.shared.setConfigDirs(dirs)
            }
    }

    func stop() {
        cancellable = nil
    }

    /// Dirs to scan: every tracked folder Claude Code runs in (never a
    /// Claude Parallel Profiles store: nothing runs there). The scanner
    /// always adds ~/.claude and the configuration's extra dirs
    /// (`SPCN_EXTRA_CONFIG_DIRS`), and reads a shared sessions folder once.
    nonisolated static func configDirs(for accounts: [ClaudeAccount]) -> Set<String> {
        Set(accounts.filter { !$0.isHidden && $0.kind == .run }.map { AccountPaths.normalize($0.configDir) })
    }
}
