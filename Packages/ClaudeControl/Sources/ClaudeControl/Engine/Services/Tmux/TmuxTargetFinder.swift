//
//  TmuxTargetFinder.swift
//  ClaudeControl
//
//  Finds tmux targets for Claude processes
//

import Foundation

/// Finds tmux session/window/pane targets for Claude processes
actor TmuxTargetFinder {
    static let shared = TmuxTargetFinder()

    private init() {}

    /// Find the tmux target for a given Claude PID
    func findTarget(forClaudePid claudePid: Int) async -> TmuxTarget? {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath() else {
            return nil
        }

        guard let output = await runTmuxCommand(tmuxPath: tmuxPath, args: [
            "list-panes", "-a", "-F", "#{session_name}:#{window_index}.#{pane_index} #{pane_pid}"
        ]) else {
            return nil
        }

        let tree = await SessionHostResolver.processTree()

        for (target, panePid) in Self.parsePanes(output)
        where ProcessTreeBuilder.shared.isDescendant(targetPid: claudePid, ofAncestor: panePid, tree: tree) {
            return TmuxTarget(from: target)
        }
        return nil
    }

    /// `list-panes -F "<target> <pane_pid>"` lines. The pid is split off the
    /// end, so a session name with spaces stays whole. Pure.
    nonisolated static func parsePanes(_ output: String) -> [(target: String, pid: Int)] {
        output.split(whereSeparator: \.isNewline).compactMap { line in
            guard let space = line.lastIndex(of: " "),
                  let pid = Int(line[line.index(after: space)...]) else { return nil }
            let target = String(line[..<space])
            return target.isEmpty ? nil : (target, pid)
        }
    }

    // MARK: - Private Methods

    private func runTmuxCommand(tmuxPath: String, args: [String]) async -> String? {
        do {
            return try await ProcessExecutor.shared.run(tmuxPath, arguments: args)
        } catch {
            return nil
        }
    }
}
