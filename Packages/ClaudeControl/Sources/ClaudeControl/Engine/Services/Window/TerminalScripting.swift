//
//  TerminalScripting.swift
//  ClaudeControl
//
//  AppleScript for iTerm2 and Terminal.app, addressed by TTY: find the
//  session (iTerm2) or tab (Terminal) whose `tty` is the Claude process's
//  terminal, then select it or send it a message. Every script talks to the
//  terminal's own scripting dictionary; nothing ever goes through System
//  Events keystrokes, so text can't land in whatever window happens to be
//  frontmost.
//
//  Scripts run in `/usr/bin/osascript` off the main actor. The first run
//  shows macOS's Automation prompt ("… wants to control iTerm2"), which is
//  why the timeout is generous: the script waits while the user decides.
//

import Foundation
import os.log

/// A terminal whose tabs can be driven by TTY.
nonisolated enum ScriptableTerminal: String, Sendable, CaseIterable {
    case iTerm2
    case terminalApp

    var bundleIdentifier: String {
        switch self {
        case .iTerm2: return TerminalAppRegistry.iTerm2BundleId
        case .terminalApp: return TerminalAppRegistry.terminalAppBundleId
        }
    }

    var displayName: String {
        switch self {
        case .iTerm2: return "iTerm2"
        case .terminalApp: return "Terminal"
        }
    }
}

/// Builds the scripts. Pure and deterministic, so it is unit-tested.
nonisolated enum TerminalScript {
    /// What a script prints when it did its job.
    static let successMarker = "ok"
    /// What a script prints when no session/tab has the TTY.
    static let notFoundMarker = "not_found"
    /// Seconds between typing a message and pressing Return (iTerm2).
    static let submitDelay: TimeInterval = 0.15

    /// `ttys003`, `/dev/ttys003` → `/dev/ttys003`. Nil for anything that isn't
    /// a plain device name (so nothing odd is ever spliced into a script).
    static func devicePath(forTTY tty: String) -> String? {
        var name = tty.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.hasPrefix("/dev/") { name.removeFirst(5) }
        guard !name.isEmpty, name != "??",
              name.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.contains($0) && $0.isASCII }) else {
            return nil
        }
        return "/dev/\(name)"
    }

    /// An AppleScript string literal (quoted, with `\` and `"` escaped).
    static func quoted(_ text: String) -> String {
        let escaped = text
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }

    /// A message as one line: Claude Code submits on every newline it
    /// receives, so line breaks become spaces and control characters go.
    static func singleLine(_ message: String) -> String {
        var result = ""
        for scalar in message.unicodeScalars {
            if scalar == "\n" || scalar == "\r" || scalar == "\t" {
                result.append(" ")
            } else if CharacterSet.controlCharacters.contains(scalar) {
                continue
            } else {
                result.unicodeScalars.append(scalar)
            }
        }
        return result.trimmingCharacters(in: .whitespaces)
    }

    /// Selects the session/tab on `tty`, raises its window and activates the app.
    static func focus(_ terminal: ScriptableTerminal, tty: String) -> String? {
        guard let device = devicePath(forTTY: tty) else { return nil }
        switch terminal {
        case .iTerm2:
            return """
            tell application id "\(terminal.bundleIdentifier)"
                repeat with w in windows
                    repeat with t in tabs of w
                        repeat with s in sessions of t
                            if tty of s is \(quoted(device)) then
                                select w
                                select t
                                select s
                                activate
                                return "\(successMarker)"
                            end if
                        end repeat
                    end repeat
                end repeat
            end tell
            return "\(notFoundMarker)"
            """
        case .terminalApp:
            return """
            tell application id "\(terminal.bundleIdentifier)"
                repeat with w in windows
                    repeat with t in tabs of w
                        if tty of t is \(quoted(device)) then
                            set selected of t to true
                            set index of w to 1
                            activate
                            return "\(successMarker)"
                        end if
                    end repeat
                end repeat
            end tell
            return "\(notFoundMarker)"
            """
        }
    }

    /// Presses Return (CR, on its own) in the iTerm2 session on `tty`: the
    /// second half of an iTerm2 `send`. Nil for Terminal.app, whose `do
    /// script` already submitted, and for an unusable TTY.
    static func submit(to terminal: ScriptableTerminal, tty: String) -> String? {
        guard terminal == .iTerm2, let device = devicePath(forTTY: tty) else { return nil }
        return "set enterKey to character id 13\n" + writeToITermSession(device: device, text: "enterKey")
    }

    private static func writeToITermSession(device: String, text: String) -> String {
        """
        tell application id "\(ScriptableTerminal.iTerm2.bundleIdentifier)"
            repeat with w in windows
                repeat with t in tabs of w
                    repeat with s in sessions of t
                        if tty of s is \(quoted(device)) then
                            tell s to write text \(text) newline NO
                            return "\(successMarker)"
                        end if
                    end repeat
                end repeat
            end repeat
        end tell
        return "\(notFoundMarker)"
        """
    }

    /// Reads the TTY of the selected session/tab of the front window. Changes
    /// nothing; prints an empty line when there is no window.
    static func selectedTTY(_ terminal: ScriptableTerminal) -> String {
        switch terminal {
        case .iTerm2:
            return """
            tell application id "\(terminal.bundleIdentifier)"
                if (count of windows) is 0 then return ""
                return tty of current session of current window
            end tell
            """
        case .terminalApp:
            return """
            tell application id "\(terminal.bundleIdentifier)"
                if (count of windows) is 0 then return ""
                return tty of selected tab of front window
            end tell
            """
        }
    }

    /// Types `message` into the session/tab on `tty`, without changing which
    /// window is in front: for Terminal.app with its Return, for iTerm2
    /// without (`submit` sends that, as a separate script, after the caller
    /// has checked again that Claude's prompt is still what reads it). Nil
    /// when the TTY or the message is unusable.
    static func send(_ message: String, to terminal: ScriptableTerminal, tty: String) -> String? {
        guard let device = devicePath(forTTY: tty) else { return nil }
        let line = singleLine(message)
        guard !line.isEmpty else { return nil }
        switch terminal {
        case .iTerm2:
            // The text only; Return (CR) follows as its own write a moment
            // later, like tmux's `send-keys -l <text>` + `send-keys Enter`.
            // `write text` alone appends its own line ending in the same
            // write: Claude Code can take that burst as a paste, and a line
            // feed as Ctrl+J (a new line), neither of which submits.
            return writeToITermSession(device: device, text: quoted(line))
        case .terminalApp:
            // `do script … in <tab>` feeds the text plus a line ending to the
            // tab's foreground process (here Claude Code's prompt). Terminal's
            // dictionary has no way to send the two separately.
            return """
            tell application id "\(terminal.bundleIdentifier)"
                repeat with w in windows
                    repeat with t in tabs of w
                        if tty of t is \(quoted(device)) then
                            do script \(quoted(line)) in t
                            return "\(successMarker)"
                        end if
                    end repeat
                end repeat
            end tell
            return "\(notFoundMarker)"
            """
        }
    }
}

/// Runs scripts in `osascript`, one at a time, off the main actor.
actor TerminalScriptRunner {
    static let shared = TerminalScriptRunner()

    private static var logger: Logger { EngineLog.logger("TerminalScript") }

    /// Long enough for the user to answer the first Automation prompt.
    static let timeout: TimeInterval = 90
    /// Read-only queries never prompt (they run only once access is granted).
    static let queryTimeout: TimeInterval = 5

    /// The last queued script; each waits for the one before it, so two
    /// messages sent in a row can't interleave their text and Returns.
    private var tail: Task<Void, Never>?

    private init() {}

    enum Outcome: Equatable, Sendable {
        case succeeded
        /// The script ran but no session/tab has that TTY.
        case notFound
        /// osascript failed (e.g. Automation permission denied: -1743).
        case failed(String)
        /// Not run: the precondition, checked once the scripts queued before
        /// it had finished, said why.
        case blocked(String)
    }

    /// Why a queued script must not run after all (nil: run it). Checked
    /// after the scripts before it have finished, immediately before
    /// osascript starts: a script that types into a terminal can wait here
    /// behind others (a focus script waits up to 90 s on the Automation
    /// prompt), and a dialog may have opened meanwhile.
    typealias Precondition = @Sendable () async -> String?

    /// Runs `script` after the scripts queued before it, and interprets its
    /// success / not-found markers.
    func run(_ script: String, label: String, precondition: Precondition? = nil) async -> Outcome {
        switch await enqueue(script, timeout: Self.timeout, precondition: precondition) {
        case .success(let output):
            let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed == TerminalScript.notFoundMarker {
                Self.logger.info("\(label, privacy: .public): no tab with that TTY")
                return .notFound
            }
            return .succeeded
        case .failure(let failure) where failure.blocked:
            Self.logger.info("\(label, privacy: .public) not run: \(failure.message, privacy: .public)")
            return .blocked(failure.message)
        case .failure(let failure):
            Self.logger.error("\(label, privacy: .public) failed: \(failure.message, privacy: .public)")
            return .failed(failure.message)
        }
    }

    /// Runs a read-only script and returns what it printed, trimmed; nil
    /// when it failed.
    func query(_ script: String, label: String) async -> String? {
        switch await enqueue(script, timeout: Self.queryTimeout) {
        case .success(let output):
            return output.trimmingCharacters(in: .whitespacesAndNewlines)
        case .failure(let failure):
            Self.logger.info("\(label, privacy: .public) failed: \(failure.message, privacy: .public)")
            return nil
        }
    }

    private func enqueue(_ script: String, timeout: TimeInterval,
                         precondition: Precondition? = nil) async -> Result<String, ScriptFailure> {
        let previous = tail
        let execution = Task {
            await previous?.value
            if let precondition, let reason = await precondition() {
                return Result<String, ScriptFailure>.failure(ScriptFailure(message: reason, blocked: true))
            }
            return await Self.execute(script: script, timeout: timeout)
        }
        tail = Task { _ = await execution.value }
        return await execution.value
    }

    struct ScriptFailure: Error, Sendable {
        let message: String
        var blocked = false
    }

    /// The script is one argument to osascript: no shell is involved.
    nonisolated private static func execute(script: String, timeout: TimeInterval) async -> Result<String, ScriptFailure> {
        let outcome = await HelperProcess.run("/usr/bin/osascript", arguments: ["-e", script], timeout: timeout)
        switch outcome {
        case .success(let output) where output.succeeded:
            return .success(output.stdoutText)
        case .success(let output):
            if output.timedOut { return .failure(ScriptFailure(message: "timed out")) }
            let reason = output.stderrText.trimmingCharacters(in: .whitespacesAndNewlines)
            return .failure(ScriptFailure(message: reason.isEmpty ? "exit \(output.status)" : reason))
        case .failure(let error):
            return .failure(ScriptFailure(message: "\(error)"))
        }
    }
}
