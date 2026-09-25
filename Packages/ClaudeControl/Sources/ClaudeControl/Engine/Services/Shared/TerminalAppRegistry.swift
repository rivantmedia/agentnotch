//
//  TerminalAppRegistry.swift
//  ClaudeControl
//
//  Centralized registry of known terminal applications and code editors
//  that host Claude Code sessions.
//
//  Matching is exact (per app name, `.app` bundle name or executable name),
//  never by substring: a substring match on names like "st", "Code" or "foot"
//  used to count "System Settings", "Xcode" and "Postman" as terminals.
//

import Foundation

/// Registry of known terminal application names and bundle identifiers
nonisolated struct TerminalAppRegistry: Sendable {
    // MARK: - Bundle identifiers

    static let iTerm2BundleId = "com.googlecode.iterm2"
    static let terminalAppBundleId = "com.apple.Terminal"
    static let ghosttyBundleId = "com.mitchellh.ghostty"
    static let cmuxBundleId = "com.cmuxterm.app"

    /// Terminals whose tabs only the host app can select (Ghostty and cmux
    /// by working directory, cmux also by surface id), through
    /// `ClaudeControlConfiguration.externalTabFocus`.
    static let externalTabFocusBundleIdentifiers: Set<String> = [ghosttyBundleId, cmuxBundleId]

    /// VS Code and its forks: they focus the window for a folder when asked to
    /// open it, and host the `claude-vscode` extension.
    static let editorBundleIdentifiers: Set<String> = [
        "com.microsoft.VSCode",
        "com.microsoft.VSCodeInsiders",
        "com.todesktop.230313mzl4w4u92",  // Cursor
        "com.exafunction.windsurf",       // Windsurf
        "com.vscodium",                   // VSCodium
        "com.vscodium.VSCodiumInsiders",
    ]

    /// Bundle identifiers for terminal apps and editors with terminals.
    static let bundleIdentifiers: Set<String> = Set([
        terminalAppBundleId,
        iTerm2BundleId,
        ghosttyBundleId,
        cmuxBundleId,
        "io.alacritty",
        "org.alacritty",
        "net.kovidgoyal.kitty",
        "co.zeit.hyper",
        "dev.warp.Warp-Stable",
        "dev.warp.Warp-Preview",
        "com.github.wez.wezterm",
        "org.tabby",
        "com.raphaelamorim.rio",
        "dev.zed.Zed",
    ]).union(editorBundleIdentifiers)

    // MARK: - Names

    /// App names (`.app` bundle names, window owner names) and executable
    /// names of terminals and editors, lowercased.
    static let appNames: Set<String> = [
        "terminal",
        "iterm2",
        "iterm",
        "ghostty",
        "cmux",
        "alacritty",
        "kitty",
        "hyper",
        "warp",
        "wezterm",
        "wezterm-gui",
        "tabby",
        "rio",
        "contour",
        "foot",
        "st",
        "urxvt",
        "xterm",
        "code",
        "code - insiders",
        "visual studio code",
        "visual studio code - insiders",
        "cursor",
        "windsurf",
        "vscodium",
        "zed",
    ]

    /// Whether an app name, window owner name, or process command (a bare
    /// name or a full executable path such as
    /// `/Applications/iTerm.app/Contents/MacOS/iTerm2`) is a known terminal.
    static func isTerminal(_ appNameOrCommand: String) -> Bool {
        candidateNames(for: appNameOrCommand).contains { appNames.contains($0) }
    }

    /// Check if a bundle identifier is a known terminal
    static func isTerminalBundle(_ bundleId: String) -> Bool {
        bundleIdentifiers.contains(bundleId)
    }

    /// VS Code family editor.
    static func isEditorBundle(_ bundleId: String) -> Bool {
        editorBundleIdentifiers.contains(bundleId)
    }

    /// The short name the hover card and the panel show for a host app:
    /// "VS Code" for both VS Code builds, the product name for the forks.
    static func editorName(forBundle bundleId: String) -> String? {
        switch bundleId {
        case "com.microsoft.VSCode", "com.microsoft.VSCodeInsiders": return "VS Code"
        case "com.todesktop.230313mzl4w4u92": return "Cursor"
        case "com.exafunction.windsurf": return "Windsurf"
        case "com.vscodium", "com.vscodium.VSCodiumInsiders": return "VSCodium"
        default: return nil
        }
    }

    /// Names to match for a name or command: the value itself, the name of
    /// the enclosing `.app` bundle, and the executable's file name, all
    /// lowercased and trimmed.
    static func candidateNames(for appNameOrCommand: String) -> [String] {
        let trimmed = appNameOrCommand.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return [] }
        var names: [String] = []
        if !trimmed.contains("/") {
            names.append(trimmed.lowercased())
            return names
        }
        // Innermost-first `.app` component, so a helper inside an app's
        // Frameworks still reports the outer app last.
        for component in trimmed.split(separator: "/").reversed() where component.hasSuffix(".app") {
            names.append(String(component.dropLast(4)).lowercased())
        }
        if let last = trimmed.split(separator: "/").last {
            names.append(String(last).lowercased())
        }
        return names
    }
}
