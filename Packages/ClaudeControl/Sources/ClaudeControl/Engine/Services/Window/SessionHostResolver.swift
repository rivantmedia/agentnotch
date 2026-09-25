//
//  SessionHostResolver.swift
//  ClaudeControl
//
//  Finds the GUI app a Claude process runs under: the nearest ancestor
//  process that is a regular (Dock) app, e.g. iTerm2, Terminal, Ghostty or
//  VS Code. Walking real parent processes and asking NSRunningApplication
//  avoids guessing from process names (Warp's binary is called `stable`,
//  VS Code's is `Electron`).
//

import AppKit
import Foundation

/// The app hosting a session's terminal.
nonisolated struct HostApp: Equatable, Sendable {
    let pid: Int32
    let bundleIdentifier: String?
    let bundleURL: URL?
    let name: String?

    var isITerm2: Bool { bundleIdentifier == TerminalAppRegistry.iTerm2BundleId }
    var isTerminalApp: Bool { bundleIdentifier == TerminalAppRegistry.terminalAppBundleId }
    /// VS Code, Cursor, Windsurf, VSCodium.
    var isEditor: Bool { bundleIdentifier.map(TerminalAppRegistry.isEditorBundle) ?? false }

    /// The AppleScript target for iTerm2 / Terminal.app, if this is one.
    var scriptableTerminal: ScriptableTerminal? {
        if isITerm2 { return .iTerm2 }
        if isTerminalApp { return .terminalApp }
        return nil
    }

    /// Ghostty or cmux: only the host app's tab focus can select the tab.
    var usesExternalTabFocus: Bool {
        bundleIdentifier.map(TerminalAppRegistry.externalTabFocusBundleIdentifiers.contains) ?? false
    }
}

/// Where a session runs, as the hover card and the panel name it.
nonisolated enum HostAppKind: Hashable, Sendable {
    case tmux
    case iTerm2
    case terminal
    /// VS Code or a fork; the product name ("VS Code", "Cursor", …).
    case vsCode(String)
    case ghostty
    case cmux
    /// Any other app, by its name ("Warp", "kitty", …).
    case other(String)

    var displayName: String {
        switch self {
        case .tmux: return "tmux"
        case .iTerm2: return "iTerm2"
        case .terminal: return "Terminal"
        case .vsCode(let name): return name
        case .ghostty: return "Ghostty"
        case .cmux: return "cmux"
        case .other(let name): return name
        }
    }

    /// tmux wins (the terminal around it is incidental), then the app the
    /// process runs under; a `claude-vscode` session with no app found is
    /// still VS Code's. Nil when nothing is known. Pure.
    static func classify(isInTmux: Bool, host: HostApp?, entrypoint: String?) -> HostAppKind? {
        if isInTmux { return .tmux }
        if let host {
            switch host.bundleIdentifier {
            case TerminalAppRegistry.iTerm2BundleId?: return .iTerm2
            case TerminalAppRegistry.terminalAppBundleId?: return .terminal
            case TerminalAppRegistry.ghosttyBundleId?: return .ghostty
            case TerminalAppRegistry.cmuxBundleId?: return .cmux
            case let bundleId?:
                if let editor = TerminalAppRegistry.editorName(forBundle: bundleId) { return .vsCode(editor) }
            case nil:
                break
            }
            if let name = host.name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                return .other(name)
            }
        }
        if entrypoint?.lowercased() == "claude-vscode" { return .vsCode("VS Code") }
        return nil
    }
}

enum SessionHostResolver {
    /// Ancestors of `pid`, nearest first, not including `pid` itself. Stops at
    /// launchd, at a missing entry, or after `maxDepth` hops (cycles in a
    /// racy `ps` snapshot can't loop forever).
    nonisolated static func ancestors(of pid: Int, tree: [Int: ProcessEntry], maxDepth: Int = 40) -> [Int] {
        var result: [Int] = []
        var seen: Set<Int> = [pid]
        var current = tree[pid]?.ppid
        while let next = current, next > 1, result.count < maxDepth, seen.insert(next).inserted {
            result.append(next)
            current = tree[next]?.ppid
        }
        return result
    }

    /// Snapshot of the process table, taken off the calling actor.
    @concurrent
    nonisolated static func processTree() async -> [Int: ProcessEntry] {
        ProcessTreeBuilder.shared.buildTree()
    }

    /// The nearest ancestor of `pid` (or `pid` itself) that is a regular app.
    /// Some terminals run shells under a helper that isn't their child
    /// (iTerm2's `iTermServer` belongs to launchd so sessions survive a
    /// restart); those are matched to the running app by bundle.
    nonisolated static func hostApp(forPid pid: Int, tree: [Int: ProcessEntry], apps: RunningAppIndex) -> HostApp? {
        let chain = [pid] + ancestors(of: pid, tree: tree)
        for candidate in chain {
            if let pid32 = Int32(exactly: candidate), let app = apps.byPid[pid32] {
                return app
            }
        }
        for candidate in chain {
            guard let command = tree[candidate]?.command else { continue }
            if let bundlePath = appBundlePath(forCommand: command), let app = apps.byBundlePath[bundlePath] {
                return app
            }
            if let bundleId = bundleIdentifierHint(forCommand: command), let app = apps.byBundleId[bundleId] {
                return app
            }
        }
        return nil
    }

    /// The outermost `.app` bundle an executable lives in, e.g.
    /// `/Applications/iTerm.app` for `/Applications/iTerm.app/Contents/MacOS/iTerm2`.
    nonisolated static func appBundlePath(forCommand command: String) -> String? {
        guard command.hasPrefix("/"), let range = command.range(of: ".app/") else { return nil }
        let path = String(command[..<range.lowerBound]) + ".app"
        return URL(fileURLWithPath: path).standardizedFileURL.path
    }

    /// Helpers that host shells outside their app's process tree.
    nonisolated static func bundleIdentifierHint(forCommand command: String) -> String? {
        let name = (command.split(separator: "/").last.map(String.init) ?? command).lowercased()
        if name.hasPrefix("itermserver") { return TerminalAppRegistry.iTerm2BundleId }
        if name == "wezterm-mux-server" { return "com.github.wez.wezterm" }
        return nil
    }

}

/// The regular (Dock) apps running at one moment, indexed for host lookups:
/// one pass over `NSWorkspace.runningApplications` serves every session.
nonisolated struct RunningAppIndex: Sendable {
    private(set) var byPid: [Int32: HostApp] = [:]
    /// Keyed by the standardized `.app` path.
    private(set) var byBundlePath: [String: HostApp] = [:]
    private(set) var byBundleId: [String: HostApp] = [:]

    static let empty = RunningAppIndex(apps: [])

    var apps: [HostApp] { Array(byPid.values) }

    init(apps: [HostApp]) {
        for app in apps {
            byPid[app.pid] = app
            if let url = app.bundleURL {
                let path = url.standardizedFileURL.path
                if byBundlePath[path] == nil { byBundlePath[path] = app }
            }
            if let bundleId = app.bundleIdentifier, byBundleId[bundleId] == nil {
                byBundleId[bundleId] = app
            }
        }
    }

    /// The regular apps running now. Each property read may be a
    /// LaunchServices round trip, so this is called only when the set of
    /// apps changes (see `RunningApps`), never per row or per event, and off
    /// the main actor (`NSRunningApplication` is thread-safe).
    static func current() -> RunningAppIndex {
        RunningAppIndex(apps: NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular && !$0.isTerminated }
            .map {
                HostApp(
                    pid: $0.processIdentifier,
                    bundleIdentifier: $0.bundleIdentifier,
                    bundleURL: $0.bundleURL,
                    name: $0.localizedName
                )
            })
    }
}
