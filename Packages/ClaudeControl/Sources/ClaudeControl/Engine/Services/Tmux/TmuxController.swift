//
//  TmuxController.swift
//  ClaudeControl
//
//  The tmux questions the focus, message and "is the user looking at it"
//  paths ask: which pane a Claude process runs in, which terminals are
//  attached to a session and what each shows, and selecting a pane.
//

import Foundation

/// A terminal attached to a tmux session (`tmux list-clients`).
nonisolated struct TmuxClient: Equatable, Sendable {
    /// Pid of the `tmux` client process (a child of the terminal's shell).
    let pid: Int
    /// The client's terminal device, e.g. `/dev/ttys004`: the terminal tab.
    let tty: String

    /// Parses `list-clients -F "#{client_pid} #{client_tty}"` output.
    static func parse(listClientsOutput output: String) -> [TmuxClient] {
        output.split(whereSeparator: \.isNewline).compactMap { line in
            let parts = line.split(separator: " ", maxSplits: 1)
            guard parts.count == 2, let pid = Int(parts[0].trimmingCharacters(in: .whitespaces)) else {
                return nil
            }
            let tty = parts[1].trimmingCharacters(in: .whitespaces)
            return tty.isEmpty ? nil : TmuxClient(pid: pid, tty: tty)
        }
    }
}

/// Controller for tmux operations
actor TmuxController {
    static let shared = TmuxController()

    private init() {}

    func findTmuxTarget(forClaudePid pid: Int) async -> TmuxTarget? {
        await TmuxTargetFinder.shared.findTarget(forClaudePid: pid)
    }

    /// Finds the pane whose TTY is `tty` (a Claude process's controlling terminal).
    func findTmuxTarget(forTTY tty: String) async -> TmuxTarget? {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath() else {
            return nil
        }
        let wanted = tty.replacingOccurrences(of: "/dev/", with: "")
        guard let output = try? await ProcessExecutor.shared.run(
            tmuxPath,
            arguments: ["list-panes", "-a", "-F", "#{session_name}:#{window_index}.#{pane_index} #{pane_tty}"]
        ) else {
            return nil
        }
        for line in output.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: " ")
            guard parts.count >= 2 else { continue }
            let paneTty = String(parts[parts.count - 1]).replacingOccurrences(of: "/dev/", with: "")
            if paneTty == wanted {
                return TmuxTarget(from: parts.dropLast().joined(separator: " "))
            }
        }
        return nil
    }

    /// Terminals attached to a tmux session.
    func clients(ofSession session: String) async -> [TmuxClient] {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath(),
              let output = try? await ProcessExecutor.shared.run(tmuxPath, arguments: [
                  "list-clients", "-t", session, "-F", "#{client_pid} #{client_tty}"
              ]) else {
            return []
        }
        return TmuxClient.parse(listClientsOutput: output)
    }

    /// The pane a client (by its terminal device) is showing, as
    /// `session:window.pane`.
    func activePane(ofClientTTY tty: String) async -> String? {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath(),
              let output = try? await ProcessExecutor.shared.run(tmuxPath, arguments: [
                  "display-message", "-p", "-c", tty, "#{session_name}:#{window_index}.#{pane_index}"
              ]) else {
            return nil
        }
        let pane = output.trimmingCharacters(in: .whitespacesAndNewlines)
        return pane.isEmpty ? nil : pane
    }

    func switchToPane(target: TmuxTarget) async -> Bool {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath() else {
            return false
        }

        do {
            _ = try await ProcessExecutor.shared.run(tmuxPath, arguments: [
                "select-window", "-t", "\(target.session):\(target.window)"
            ])

            _ = try await ProcessExecutor.shared.run(tmuxPath, arguments: [
                "select-pane", "-t", target.targetString
            ])

            return true
        } catch {
            return false
        }
    }
}
