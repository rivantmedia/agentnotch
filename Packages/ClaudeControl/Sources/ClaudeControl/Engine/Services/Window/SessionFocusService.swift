//
//  SessionFocusService.swift
//  ClaudeControl
//
//  Brings a session's terminal (or editor) to the front. Focusing a session
//  counts as reviewing it.
//
//  Strategies, tried in order until one works:
//  1. tmux: select the Claude pane, then bring up the terminal showing that
//     tmux session (its tab by TTY in iTerm2 / Terminal, the host's tab focus
//     for Ghostty / cmux, else yabai when installed, else the client's app).
//  2. iTerm2 / Terminal.app: select the session/tab whose TTY is Claude's,
//     raise its window and activate the app (AppleScript, no keystrokes).
//  3. Ghostty / cmux: the host app's own tab focus
//     (`ClaudeControlConfiguration.externalTabFocus`), then activate.
//  4. VS Code family (the `claude-vscode` extension, or a terminal inside the
//     editor): ask the editor to open the session's folder, which focuses the
//     window that has it open.
//  5. Anything else (Warp, kitty, WezTerm, Alacritty, Hyper, …): activate the
//     nearest ancestor process that is a regular app.
//

import AppKit
import Foundation
import os.log

/// One way of focusing a session.
nonisolated enum FocusStep: Equatable, Sendable {
    /// Select the tmux pane running Claude, then focus the attached client's terminal.
    case tmux
    /// Select the tab with this TTY in iTerm2 / Terminal.app.
    case scriptedTab(ScriptableTerminal, tty: String)
    /// Ask the host app to select the tab (Ghostty, cmux), then activate it.
    case externalTab(ClaudeExternalTabRequest, hostPid: Int32, bundleURL: URL?)
    /// Open `folder` with the editor at `appURL` (focuses its window).
    case openFolder(appURL: URL, folder: String)
    /// Activate the app with this pid.
    case activate(pid: Int32, bundleURL: URL?)

    static func == (lhs: FocusStep, rhs: FocusStep) -> Bool {
        switch (lhs, rhs) {
        case (.tmux, .tmux): return true
        case let (.scriptedTab(a, x), .scriptedTab(b, y)): return a == b && x == y
        case let (.externalTab(a, p, u), .externalTab(b, q, v)):
            return a.bundleID == b.bundleID && a.pid == b.pid && a.tty == b.tty && a.cwd == b.cwd && p == q && u == v
        case let (.openFolder(a, x), .openFolder(b, y)): return a == b && x == y
        case let (.activate(p, u), .activate(q, v)): return p == q && u == v
        default: return false
        }
    }
}

/// What the focus planner knows about a session.
nonisolated struct FocusContext: Equatable, Sendable {
    var pid: Int?
    var isInTmux: Bool
    var tty: String?
    var entrypoint: String?
    /// The folder the session started in.
    var cwd: String
    /// Nearest regular app above the Claude process, if any.
    var host: HostApp?
    /// A running VS Code-family editor to use for `claude-vscode` sessions
    /// whose process tree doesn't lead to one.
    var fallbackEditorURL: URL?
    /// The host app can select Ghostty / cmux tabs for us.
    var hasExternalTabFocus: Bool = false
    /// The workspace an editor most likely has open for `cwd`: its git
    /// toplevel (see `WorkspaceRoot`); nil when there is none.
    var workspaceRoot: String? = nil

    var isVSCodeExtension: Bool {
        entrypoint?.lowercased() == "claude-vscode"
    }
}

/// The folder an editor window most likely has open for a session started
/// in `cwd`: the nearest folder at or above it holding `.git` (a folder, or
/// a file for a worktree), below the home folder. Nil when there is none.
nonisolated enum WorkspaceRoot {
    static func find(from cwd: String, home: String = AccountPaths.homeDirectory,
                     exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String? {
        guard !cwd.isEmpty else { return nil }
        let stop = (home as NSString).standardizingPath
        var folder = (cwd as NSString).standardizingPath
        while folder != "/" && folder != stop && !folder.isEmpty {
            if exists((folder as NSString).appendingPathComponent(".git")) { return folder }
            folder = (folder as NSString).deletingLastPathComponent
        }
        return nil
    }
}

nonisolated enum FocusPlanner {
    /// Strategies worth trying for a session, best first. Empty when nothing
    /// could focus it (e.g. a Claude process under ssh with no local app).
    static func plan(_ context: FocusContext) -> [FocusStep] {
        var steps: [FocusStep] = []
        if context.isInTmux {
            steps.append(.tmux)
        }
        // Inside tmux the process's TTY is the pane's, not the terminal
        // tab's; the tmux step finds the client's tab instead.
        if !context.isInTmux, let host = context.host {
            if let terminal = host.scriptableTerminal,
               let tty = context.tty,
               TerminalScript.devicePath(forTTY: tty) != nil {
                steps.append(.scriptedTab(terminal, tty: tty))
            } else if host.usesExternalTabFocus, context.hasExternalTabFocus,
                      let pid = context.pid, let pid32 = Int32(exactly: pid) {
                let request = ClaudeExternalTabRequest(
                    bundleID: host.bundleIdentifier,
                    pid: pid32,
                    tty: context.tty,
                    cwd: context.cwd.isEmpty ? nil : context.cwd
                )
                steps.append(.externalTab(request, hostPid: host.pid, bundleURL: host.bundleURL))
            }
        }
        let editorURL: URL? = {
            if let host = context.host, host.isEditor { return host.bundleURL }
            if context.isVSCodeExtension { return context.fallbackEditorURL }
            return nil
        }()
        // Opening a folder brings the editor window that has it open to the
        // front, but opens a new window for any other folder. The extension
        // runs in its workspace's root; a CLI in the editor's terminal may
        // have been started in a subfolder, so its workspace root is opened
        // instead, and without one the editor is only activated (BHV-8).
        let folder = context.isVSCodeExtension ? context.cwd : (context.workspaceRoot ?? "")
        if let editorURL, !folder.isEmpty, !context.isInTmux {
            steps.append(.openFolder(appURL: editorURL, folder: folder))
        }
        if let host = context.host {
            steps.append(.activate(pid: host.pid, bundleURL: host.bundleURL))
        }
        return steps
    }
}

@MainActor
final class SessionFocusService {
    static let shared = SessionFocusService()

    private static var logger: Logger { EngineLog.logger("Focus") }

    private init() {}

    /// Selects a Ghostty / cmux tab; from `ClaudeControlConfiguration`.
    private var externalTabFocus: (@Sendable (ClaudeExternalTabRequest) -> Bool)? {
        AppIdentity.configuration.externalTabFocus
    }

    // MARK: - API

    /// Whether some strategy can plausibly focus this session (shows or
    /// hides the focus button). Answers from memory: the host-app cache,
    /// the running-apps index and one `kill(pid, 0)`; an unknown host is
    /// looked up in the background and assumed focusable meanwhile.
    func canFocus(_ session: SessionState) -> Bool {
        // Sealed fixtures: "focusing" only marks the session reviewed.
        if DevFlags.isSealed { return true }
        guard let pid = session.pid, ProcessID.isRunning(pid) else {
            // A VS Code extension session without a live pid can still be
            // brought up by opening its folder in a running editor.
            return session.entrypoint?.lowercased() == "claude-vscode" && RunningApps.shared.runningEditorURL != nil
        }
        if session.isInTmux { return true }
        guard let cached = SessionHostCache.shared.host(forPid: pid) else { return true }
        return !FocusPlanner.plan(context(for: session, host: cached)).isEmpty
    }

    /// Focuses the session's terminal or editor. Marks the session reviewed
    /// when it succeeds. Returns whether anything was focused.
    @discardableResult
    func focus(_ session: SessionState) async -> Bool {
        // Sealed fixtures carry made-up pids: never script or activate anything.
        if DevFlags.isSealed {
            ClaudeSessionMonitor.shared.markReviewed(sessionId: session.sessionId)
            return true
        }
        var host: HostApp?
        if let pid = session.pid, ProcessID.isRunning(pid) {
            host = await SessionHostCache.shared.resolve(pid: pid)
        }
        let steps = FocusPlanner.plan(context(for: session, host: host))
        if steps.isEmpty {
            Self.logger.info("Nothing can focus session \(session.sessionId, privacy: .public)")
        }

        var focused = false
        for step in steps {
            if await perform(step, session: session) {
                focused = true
                break
            }
        }
        if focused {
            ClaudeSessionMonitor.shared.markReviewed(sessionId: session.sessionId)
        }
        return focused
    }

    /// The app hosting the session's terminal, when known (resolved on demand).
    func hostApp(for session: SessionState) async -> HostApp? {
        guard let pid = session.pid, ProcessID.isRunning(pid) else { return nil }
        return await SessionHostCache.shared.resolve(pid: pid)
    }

    // MARK: - Planning

    private func context(for session: SessionState, host: HostApp?) -> FocusContext {
        FocusContext(
            pid: session.pid,
            isInTmux: session.isInTmux,
            tty: session.tty,
            entrypoint: session.entrypoint,
            cwd: session.cwd,
            host: host,
            fallbackEditorURL: session.entrypoint?.lowercased() == "claude-vscode" ? RunningApps.shared.runningEditorURL : nil,
            hasExternalTabFocus: externalTabFocus != nil,
            workspaceRoot: host?.isEditor == true ? WorkspaceRoot.find(from: session.cwd) : nil
        )
    }

    // MARK: - Steps

    private func perform(_ step: FocusStep, session: SessionState) async -> Bool {
        switch step {
        case .tmux:
            return await focusTmux(session: session)
        case .scriptedTab(let terminal, let tty):
            return await focusTab(terminal, tty: tty)
        case .externalTab(let request, let hostPid, let bundleURL):
            guard await selectExternalTab(request) else { return false }
            return await activate(pid: hostPid, bundleURL: bundleURL)
        case .openFolder(let appURL, let folder):
            return await openFolder(folder, with: appURL)
        case .activate(let pid, let bundleURL):
            return await activate(pid: pid, bundleURL: bundleURL)
        }
    }

    /// Selects Claude's pane, then brings up a terminal showing that tmux
    /// session: its tab in iTerm2 / Terminal (by the client's TTY) or through
    /// the host's tab focus (Ghostty, cmux), else yabai, else the client's app.
    private func focusTmux(session: SessionState) async -> Bool {
        guard let pid = session.pid,
              let target = await TmuxController.shared.findTmuxTarget(forClaudePid: pid) else {
            return false
        }
        _ = await TmuxController.shared.switchToPane(target: target)

        let clients = await TmuxController.shared.clients(ofSession: target.session)
        var hosts: [(client: TmuxClient, host: HostApp)] = []
        for client in clients {
            if let host = await SessionHostCache.shared.resolve(pid: client.pid) {
                hosts.append((client, host))
            }
        }

        // The exact tab first.
        for (client, host) in hosts {
            if let terminal = host.scriptableTerminal, await focusTab(terminal, tty: client.tty) {
                return true
            }
            if host.usesExternalTabFocus, let clientPid = Int32(exactly: client.pid) {
                let request = ClaudeExternalTabRequest(
                    bundleID: host.bundleIdentifier, pid: clientPid, tty: client.tty, cwd: nil
                )
                if await selectExternalTab(request), await activate(pid: host.pid, bundleURL: host.bundleURL) {
                    return true
                }
            }
        }
        // yabai knows which window shows tmux.
        if await YabaiController.shared.focusWindow(forTmuxTarget: target) {
            return true
        }
        // Otherwise just the client's app.
        for (_, host) in hosts {
            if await activate(pid: host.pid, bundleURL: host.bundleURL) {
                return true
            }
        }
        return false
    }

    private func focusTab(_ terminal: ScriptableTerminal, tty: String) async -> Bool {
        guard let script = TerminalScript.focus(terminal, tty: tty) else { return false }
        let outcome = await TerminalScriptRunner.shared.run(script, label: "focus \(terminal.displayName)")
        return outcome == .succeeded
    }

    /// The host's tab focus runs AppleScript or a CLI synchronously: off
    /// the main actor.
    private func selectExternalTab(_ request: ClaudeExternalTabRequest) async -> Bool {
        guard let select = externalTabFocus else { return false }
        let selected = await Self.callOffMain(select, request)
        if !selected {
            Self.logger.info("The host couldn't select the \(request.bundleID ?? "?", privacy: .public) tab")
        }
        return selected
    }

    @concurrent
    nonisolated private static func callOffMain(
        _ select: @Sendable (ClaudeExternalTabRequest) -> Bool,
        _ request: ClaudeExternalTabRequest
    ) async -> Bool {
        select(request)
    }

    private func openFolder(_ folder: String, with appURL: URL) async -> Bool {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: folder, isDirectory: &isDirectory), isDirectory.boolValue else {
            return false
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        configuration.addsToRecentItems = false
        do {
            _ = try await NSWorkspace.shared.open(
                [URL(fileURLWithPath: folder, isDirectory: true)],
                withApplicationAt: appURL,
                configuration: configuration
            )
            return true
        } catch {
            Self.logger.error("Opening the session folder in the editor failed: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }

    /// Activates an app, bringing up its frontmost window (not every window:
    /// raising all of a terminal's windows buries whatever else was open).
    /// Cooperative activation (macOS 14+) can refuse when this accessory app
    /// isn't active; LaunchServices then brings it up.
    private func activate(pid: Int32, bundleURL: URL?) async -> Bool {
        guard let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated else { return false }
        NSApp.yieldActivation(to: app)
        if app.activate(options: []) {
            return true
        }
        guard let bundleURL else { return false }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        do {
            _ = try await NSWorkspace.shared.openApplication(at: bundleURL, configuration: configuration)
            return true
        } catch {
            Self.logger.error("Activating \(bundleURL.lastPathComponent, privacy: .public) failed: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }
}
