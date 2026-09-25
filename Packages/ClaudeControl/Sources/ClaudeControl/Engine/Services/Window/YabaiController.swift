//
//  YabaiController.swift
//  ClaudeControl
//
//  Focuses the terminal window showing a tmux session through yabai, for
//  tmux clients whose terminal can't be scripted by TTY. The caller has
//  already selected Claude's pane (SessionFocusService.focusTmux).
//

import Foundation

actor YabaiController {
    static let shared = YabaiController()

    private init() {}

    /// Raise the yabai window of a terminal attached to the tmux session
    /// `target` belongs to. False without yabai or a matching window.
    func focusWindow(forTmuxTarget target: TmuxTarget) async -> Bool {
        guard await WindowFinder.shared.isYabaiAvailable() else {
            return false
        }
        let windows = await WindowFinder.shared.getAllWindows()
        guard !windows.isEmpty else { return false }
        let tree = await SessionHostResolver.processTree()
        guard let terminalPid = await findTmuxClientTerminal(forSession: target.session, tree: tree, windows: windows) else {
            return false
        }
        return await WindowFocuser.shared.focusTmuxWindow(terminalPid: terminalPid, windows: windows)
    }

    /// The pid of a terminal (with a yabai window) running a client of `session`.
    private func findTmuxClientTerminal(forSession session: String, tree: [Int: ProcessEntry], windows: [YabaiWindow]) async -> Int? {
        let clients = await TmuxController.shared.clients(ofSession: session)
        let windowPids = Set(windows.map(\.pid))
        return Self.terminalPid(clientPids: clients.map(\.pid), tree: tree, windowPids: windowPids)
    }

    /// Walks up from each client to the first terminal process that owns a
    /// window. Pure.
    nonisolated static func terminalPid(clientPids: [Int], tree: [Int: ProcessEntry], windowPids: Set<Int>) -> Int? {
        for clientPid in clientPids {
            var current = clientPid
            var depth = 0
            while current > 1 && depth < 40 {
                guard let info = tree[current] else { break }
                if TerminalAppRegistry.isTerminal(info.command) && windowPids.contains(current) {
                    return current
                }
                current = info.ppid
                depth += 1
            }
        }
        return nil
    }
}
