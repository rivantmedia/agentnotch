//
//  ShellWords.swift
//  ClaudeControl
//
//  Just enough of the POSIX shell's word splitting to read back the hook and
//  status line commands in a settings.json: which script a command runs, and
//  how to quote a path so the shell hands it over unchanged.
//
//  It understands single quotes, double quotes, backslash escapes and the
//  operators that end a simple command (`;`, `&`, `|`, `&&`, `||`), which is
//  everything the commands we write (and the ones other notch apps write)
//  use. It never expands anything: `$HOME` stays `$HOME`.
//

import Foundation

nonisolated enum ShellWords {
    /// One token of a command line: a word, or a control operator.
    enum Token: Equatable, Sendable {
        case word(String)
        case `operator`(String)
    }

    /// `value` quoted for the shell: single quotes, with any single quote
    /// written as `'\''`. Safe for every byte a path can hold, including
    /// `$`, backticks, `"` and newlines.
    static func quote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// Split `command` into words and operators. An unterminated quote ends
    /// the command (what is left is kept as the last word), so this never fails.
    static func tokens(_ command: String) -> [Token] {
        var tokens: [Token] = []
        var word = ""
        var inWord = false
        var characters = command.makeIterator()
        var pending: Character?

        func next() -> Character? {
            if let character = pending {
                pending = nil
                return character
            }
            return characters.next()
        }
        func endWord() {
            if inWord { tokens.append(.word(word)) }
            word = ""
            inWord = false
        }

        while let character = next() {
            switch character {
            case "'":
                inWord = true
                while let quoted = next(), quoted != "'" { word.append(quoted) }
            case "\"":
                inWord = true
                while let quoted = next(), quoted != "\"" {
                    if quoted == "\\", let escaped = next() {
                        // Inside double quotes a backslash only escapes these.
                        if !"$`\"\\\n".contains(escaped) { word.append("\\") }
                        if escaped != "\n" { word.append(escaped) }
                    } else {
                        word.append(quoted)
                    }
                }
            case "\\":
                inWord = true
                if let escaped = next(), escaped != "\n" { word.append(escaped) }
            case ";", "&", "|":
                endWord()
                var op = String(character)
                if character != ";", let following = next() {
                    if following == character {
                        op.append(following)
                    } else {
                        pending = following
                    }
                }
                tokens.append(.operator(op))
            case "\n":
                endWord()
                tokens.append(.operator(";"))
            default:
                if character.isWhitespace {
                    endWord()
                } else {
                    inWord = true
                    word.append(character)
                }
            }
        }
        endWord()
        return tokens
    }

    /// The words of `command`, operators dropped.
    static func words(_ command: String) -> [String] {
        tokens(command).compactMap { token in
            if case .word(let word) = token { return word }
            return nil
        }
    }
}
