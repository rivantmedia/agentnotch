//
//  HookCommands.swift
//  ClaudeControl
//
//  The shell commands the installer writes into settings.json, and how any
//  notch app's command is recognised again. Pure.
//
//  A command we write fails open:
//
//      [ -f '<script>' ] || exit 0; P='<python>'; [ -x "$P" ] || P=python3; exec "$P" -S '<script>'
//
//  Claude Code treats exit status 2 from a hook as "block": it erases the
//  prompt, blocks the tool call, keeps Stop from stopping. Python exits 2
//  when its script is missing, so a plain `python3 '<script>'` turns a moved
//  config folder (or a settings.json synced to a machine without the app)
//  into an unusable Claude Code. Here a missing script exits 0, and a
//  missing interpreter exits 127, which Claude Code only logs.
//
//  The interpreter is resolved once per launch to an absolute path (skipping
//  the `/usr/bin/python3` shim, about 5 ms per call) with `python3` on PATH as
//  the fallback, and runs with `-S` (no `site` import; the scripts use only
//  the standard library), because hooks run on every tool call.
//
//  When no absolute interpreter was found, `python3` is looked up at run
//  time on Claude Code's own PATH (conda, pyenv, mise and uv interpreters
//  work), but never run when that is `/usr/bin/python3` on a Mac without
//  the developer tools: there it is only xcode-select's shim, which would
//  ask to install them on every tool call (S8). The hook then does nothing:
//
//      [ -f '<script>' ] || exit 0; P='python3'; case "$(command -v "$P")" in
//      ''|/usr/bin/python3) /usr/bin/xcode-select -p >/dev/null 2>&1 || exit 0;;
//      esac; exec "$P" -S '<script>'
//
//  Recognising a command: its last simple command must run a Python
//  interpreter (or our `$P`) on an absolute path whose file name is the
//  script's. A third-party hook that merely mentions the name, like
//  `notify.sh --skip claude-island-state.py`, is not a match.
//

import Foundation

nonisolated enum HookCommands {
    /// The fail-open command that runs `script` with `python` (an absolute
    /// path, or a bare name found on PATH at run time).
    static func command(runningScript script: String, python: String) -> String {
        let quotedScript = ShellWords.quote(script)
        if python.hasPrefix("/") {
            return "[ -f \(quotedScript) ] || exit 0; P=\(ShellWords.quote(python)); [ -x \"$P\" ] || P=python3; exec \"$P\" -S \(quotedScript)"
        }
        let interpreter = python.isEmpty ? "python3" : python
        return "[ -f \(quotedScript) ] || exit 0; P=\(ShellWords.quote(interpreter)); "
            + "case \"$(command -v \"$P\")\" in ''|/usr/bin/python3) \(shimGuard);; esac; "
            + "exec \"$P\" -S \(quotedScript)"
    }

    /// Exits 0 unless the developer tools are installed: without them
    /// `/usr/bin/python3` is only the shim that asks to install them.
    static let shimGuard = "/usr/bin/xcode-select -p >/dev/null 2>&1 || exit 0"

    /// The script `command` runs, when it runs a Python interpreter on an
    /// absolute path (`~`, `$HOME` and `${HOME}` expanded against `home`)
    /// whose file name is `scriptName`. Nil otherwise.
    static func scriptPath(in command: String, named scriptName: String, home: String = AccountPaths.homeDirectory) -> String? {
        guard var words = lastSimpleCommand(command), let script = words.popLast() else { return nil }
        guard let interpreter = interpreterWord(in: words), isInterpreter(interpreter) else { return nil }
        let path = expandHome(script, home: home)
        guard path.hasPrefix("/"), (path as NSString).lastPathComponent == scriptName else { return nil }
        return (path as NSString).standardizingPath
    }

    /// Whether `command` runs `scriptName` (see `scriptPath`).
    static func runs(_ command: String, script scriptName: String, home: String = AccountPaths.homeDirectory) -> Bool {
        scriptPath(in: command, named: scriptName, home: home) != nil
    }

    /// Vibe Island's bridge (`~/.vibe-island/bin/vibe-island-bridge`), which
    /// it registers inside a `/bin/sh -c '…'` wrapper.
    static func runsVibeIsland(_ command: String) -> Bool {
        ShellWords.words(command).contains { $0.contains("/.vibe-island/bin/") }
    }

    // MARK: - Parts

    /// The words of the last simple command (after the last `;`, `&&`, `||`,
    /// `|`, `&` or newline), or nil when there are none.
    static func lastSimpleCommand(_ command: String) -> [String]? {
        var current: [String] = []
        var last: [String]?
        for token in ShellWords.tokens(command) {
            switch token {
            case .word(let word):
                current.append(word)
            case .operator:
                if !current.isEmpty { last = current }
                current = []
            }
        }
        if !current.isEmpty { last = current }
        return last
    }

    /// The program a simple command runs: past `exec`, `env` (with its
    /// options and `NAME=value` pairs) and leading assignments.
    static func interpreterWord(in words: [String]) -> String? {
        var index = 0
        func isAssignment(_ word: String) -> Bool {
            guard let equals = word.firstIndex(of: "="), equals != word.startIndex else { return false }
            return word[..<equals].allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" }
        }
        while index < words.count {
            let word = words[index]
            if word == "exec" || isAssignment(word) {
                index += 1
            } else if (word as NSString).lastPathComponent == "env" {
                index += 1
                while index < words.count, words[index].hasPrefix("-") || isAssignment(words[index]) { index += 1 }
            } else {
                return word
            }
        }
        return nil
    }

    /// `python`, `python3`, `python3.12`, any path to one, or our `$P`.
    static func isInterpreter(_ word: String) -> Bool {
        if word == "$P" || word == "${P}" { return true }
        return (word as NSString).lastPathComponent.hasPrefix("python")
    }

    static func expandHome(_ path: String, home: String) -> String {
        for prefix in ["~/", "$HOME/", "${HOME}/"] where path.hasPrefix(prefix) {
            return (home as NSString).appendingPathComponent(String(path.dropFirst(prefix.count)))
        }
        if path == "~" || path == "$HOME" || path == "${HOME}" { return home }
        return path
    }
}
