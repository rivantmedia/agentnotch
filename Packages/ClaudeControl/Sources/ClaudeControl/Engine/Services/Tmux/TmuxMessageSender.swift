//
//  TmuxMessageSender.swift
//  ClaudeControl
//
//  Types a chat message into the tmux pane Claude runs in: the text with
//  `send-keys -l` (literal, so nothing in it is read as a key name), then
//  Enter as a separate key. Callers check first that Claude's prompt is what
//  will read it (`MessageSafety`). Approvals never go this way: they are
//  answered through the hook socket, not by typing into the terminal.
//

import Foundation
import os.log

actor TmuxMessageSender {
    static let shared = TmuxMessageSender()

    private static var logger: Logger { EngineLog.logger("TmuxSend") }

    private init() {}

    /// `send-keys` typing `message` literally. `--` ends the options, so a
    /// message that starts with `-` ("-R", "--help", "- fix the tests") is
    /// typed rather than read as send-keys flags.
    nonisolated static func typeArguments(pane: String, message: String) -> [String] {
        ["send-keys", "-t", pane, "-l", "--", message]
    }

    nonisolated static func enterArguments(pane: String) -> [String] {
        ["send-keys", "-t", pane, "Enter"]
    }

    /// Types `message` and presses Enter in `target`. Returns whether both
    /// keystrokes were delivered. `precondition` is asked again right before
    /// each keystroke (nil: go ahead): a dialog can open between the two.
    func send(_ message: String, to target: TmuxTarget,
              precondition: TerminalScriptRunner.Precondition? = nil) async -> Bool {
        guard let tmuxPath = await TmuxPathFinder.shared.getTmuxPath() else {
            return false
        }
        let pane = target.targetString
        do {
            if let reason = await precondition?() {
                Self.logger.info("Not typing into \(pane, privacy: .public): \(reason, privacy: .public)")
                return false
            }
            _ = try await ProcessExecutor.shared.run(tmuxPath, arguments: Self.typeArguments(pane: pane, message: message))
            if let reason = await precondition?() {
                Self.logger.info("Not pressing Enter in \(pane, privacy: .public): \(reason, privacy: .public)")
                return false
            }
            _ = try await ProcessExecutor.shared.run(tmuxPath, arguments: Self.enterArguments(pane: pane))
            return true
        } catch {
            Self.logger.error("Typing into \(pane, privacy: .public) failed: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }
}
