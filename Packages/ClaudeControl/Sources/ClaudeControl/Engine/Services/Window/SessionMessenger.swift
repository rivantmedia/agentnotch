//
//  SessionMessenger.swift
//  ClaudeControl
//
//  Sends a chat message to a session's terminal, addressed by its TTY:
//  tmux panes via `send-keys`, iTerm2 sessions via `write text`, and
//  Terminal.app tabs via `do script … in <tab>`. Never System Events
//  keystrokes, which type into whatever happens to be frontmost.
//
//  Typing is only safe when Claude Code's prompt is what reads the text:
//  Return answers whatever dialog the terminal shows (a permission the app
//  doesn't hold, a sandbox network prompt, an elicitation) with its
//  highlighted option, and a suspended claude hands the text to the shell.
//  So before every send the live process is checked: still running on the
//  session's TTY, not stopped, in the foreground of that terminal, and the
//  session isn't waiting on a dialog.
//
//  Dialogs open mid-tool (every permission, question or plan prompt follows
//  a PreToolUse whose hook the engine has already heard), so a reply is
//  held while a tool is in flight (without hooks, while Claude works at
//  all), for a few seconds, then refused with the draft kept. And the check
//  is repeated at the last moment: immediately before each osascript run
//  (after any script queued ahead of it) and before each tmux keystroke,
//  and between iTerm2's text and its Return. Before the first script to a
//  terminal, macOS's Automation prompt is raised by a harmless query, so
//  the typing itself never waits on it.
//

import Foundation

/// How a message can reach a session's terminal.
nonisolated enum MessageRoute: Equatable, Sendable {
    case tmux
    case scripted(ScriptableTerminal)

    /// Which route applies: tmux wins (the process's TTY is the pane's),
    /// then iTerm2 / Terminal.app when that's the hosting app. Nil when
    /// neither applies or the TTY is unknown.
    static func route(isInTmux: Bool, tty: String?, host: HostApp?) -> MessageRoute? {
        guard let tty, TerminalScript.devicePath(forTTY: tty) != nil else { return nil }
        if isInTmux { return .tmux }
        if let terminal = host?.scriptableTerminal { return .scripted(terminal) }
        return nil
    }
}

/// Whether a message may be typed into a session's terminal right now.
nonisolated enum MessageAvailability: Equatable, Sendable {
    case available(MessageRoute)
    /// Why not; short and user-facing ("Answer in the terminal: …").
    case unavailable(String)

    var route: MessageRoute? {
        if case .available(let route) = self { return route }
        return nil
    }
}

/// The safety rules, pure so they are unit-tested.
nonisolated enum MessageSafety {
    /// Why typing into the terminal would not reach Claude's prompt, or nil
    /// when it would.
    /// - Parameters:
    ///   - attention: the session's attention now.
    ///   - expectedTTY: the TTY the session reported.
    ///   - process: the live process's kernel status; nil when it is gone.
    static func blockReason(attention: SessionAttention, expectedTTY: String?, process: ProcessStatus?) -> String? {
        if let reason = attention.needsInputReason, !reason.isError {
            // A dialog is open in the terminal; Return would answer it.
            switch reason {
            case .question: return "Answer the question in the terminal or above"
            case .planApproval: return "Approve or reject the plan first"
            case .permission: return "Answer the permission prompt first"
            case .elicitation, .dialog, .error: return "Answer in the terminal: Claude is showing a prompt"
            }
        }
        guard let process else { return "The session's process has ended" }
        guard let expected = expectedTTY.flatMap(TerminalScript.devicePath(forTTY:)),
              let actual = process.tty.flatMap(TerminalScript.devicePath(forTTY:)),
              expected == actual else {
            return "The session's terminal can't be confirmed"
        }
        if process.isStopped { return "Claude is suspended in its terminal (Ctrl+Z)" }
        if !process.isForeground { return "Claude isn't in the foreground of its terminal" }
        return nil
    }

    /// Why typing must wait for Claude, or nil. A permission, question or
    /// plan dialog only ever opens while a tool call is in flight (its
    /// PreToolUse has already been heard), so with hooks a reply waits out
    /// the main session's tool calls, and any call while the turn runs.
    /// Without hooks nothing announces a tool, so it waits out the whole turn.
    static func busyReason(attention: SessionAttention, isHookBacked: Bool, tools: ToolTracker) -> String? {
        let working = attention == .working
        if !isHookBacked {
            return working ? "Claude is working: send when it's done" : nil
        }
        let mainTool = tools.inProgress.values.contains { $0.agentId == nil }
        if mainTool || (working && !tools.inProgress.isEmpty) {
            return "Claude is running a tool: send when it's done"
        }
        return nil
    }

    /// Both checks, on the session as the engine has it now: what the
    /// last-moment recheck asks before each keystroke.
    static func reason(for session: SessionState, process: ProcessStatus?) -> String? {
        blockReason(attention: session.attention, expectedTTY: session.tty, process: process)
            ?? busyReason(attention: session.attention, isHookBacked: session.isHookBacked, tools: session.toolTracker)
    }
}

@MainActor
enum SessionMessenger {
    /// Whether a message can be sent now, and how. Judged on the session as
    /// the engine knows it now, not on the caller's copy, which may predate a
    /// dialog that just opened.
    static func availability(for session: SessionState) async -> MessageAvailability {
        // Sealed fixtures have no terminal to type into.
        if DevFlags.isSealed { return .unavailable("Not available with sample sessions") }
        let session = current(session)
        guard let tty = session.tty, TerminalScript.devicePath(forTTY: tty) != nil else {
            return .unavailable("The session's terminal isn't known")
        }
        guard let pid = session.pid else { return .unavailable("The session's process isn't known") }
        let process = await Self.status(ofPid: pid)
        // Not `busy`: the composer stays while a tool runs; a send made
        // then is held (`deliver`).
        if let reason = MessageSafety.blockReason(attention: session.attention, expectedTTY: tty, process: process) {
            return .unavailable(reason)
        }
        let host = session.isInTmux ? nil : await SessionFocusService.shared.hostApp(for: session)
        guard let route = MessageRoute.route(isInTmux: session.isInTmux, tty: tty, host: host) else {
            return .unavailable("Replies can be typed into tmux, iTerm2 and Terminal sessions")
        }
        return .available(route)
    }

    /// The route for a session, nil when it has none or typing isn't safe now.
    static func route(for session: SessionState) async -> MessageRoute? {
        await availability(for: session).route
    }

    /// What became of a message.
    enum Delivery: Equatable, Sendable {
        case delivered
        /// Not typed at all, for this reason (the user's words); the draft
        /// should be kept.
        case refused(String)
        /// iTerm2 only: the text was typed but Return was not pressed, for
        /// this reason; the text waits in Claude's prompt.
        case typedNotSubmitted(String)
        /// The terminal or tmux could not be reached.
        case failed
    }

    /// How long a send made while a tool runs waits for it to finish.
    nonisolated static let holdLimit: TimeInterval = 10
    nonisolated static let holdPoll: TimeInterval = 0.25

    /// Types `text` plus Return into the session's terminal. Returns whether
    /// it was delivered.
    @discardableResult
    static func send(_ text: String, to session: SessionState) async -> Bool {
        await deliver(text, to: session) == .delivered
    }

    /// Types `text` plus Return into the session's terminal, after checking
    /// again that Claude's prompt is what will read it — and again right
    /// before every keystroke that leaves the app. While a tool is in flight
    /// the message is held for up to `holdLimit`, then refused.
    static func deliver(_ text: String, to session: SessionState) async -> Delivery {
        let message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty else { return .refused("Nothing to send") }
        switch await availability(for: session) {
        case .unavailable(let reason): return .refused(reason)
        case .available(let route):
            guard let tty = current(session).tty else { return .refused("The session's terminal isn't known") }
            let check = precondition(for: session.sessionId, tty: tty)
            if let reason = await waitUntilFree(sessionId: session.sessionId, tty: tty) {
                return .refused(reason)
            }
            return await type(message, route: route, tty: tty, check: check)
        }
    }

    /// Waits while Claude is busy (a tool in flight), up to `holdLimit`.
    /// Nil once typing may go ahead; otherwise why not.
    static func waitUntilFree(sessionId: String, tty: String,
                              limit: TimeInterval = holdLimit, poll: TimeInterval = holdPoll,
                              recheck: ((String, String) async -> String?)? = nil,
                              isBusy: ((String) -> Bool)? = nil) async -> String? {
        let deadline = Date().addingTimeInterval(limit)
        while true {
            let reason = if let recheck { await recheck(sessionId, tty) } else { await Self.recheck(sessionId: sessionId, tty: tty) }
            guard let reason else { return nil }
            // Anything but a running tool (a dialog, a stopped process) is final.
            let busy = isBusy.map { $0(sessionId) } ?? Self.isBusy(sessionId: sessionId)
            guard busy, Date() < deadline else { return reason }
            try? await Task.sleep(for: .seconds(poll))
        }
    }

    private static func isBusy(sessionId: String) -> Bool {
        guard let session = ClaudeSessionMonitor.shared.instances.first(where: { $0.sessionId == sessionId }) else { return false }
        return MessageSafety.busyReason(attention: session.attention, isHookBacked: session.isHookBacked,
                                        tools: session.toolTracker) != nil
    }

    private static func type(_ message: String, route: MessageRoute, tty: String,
                             check: @escaping TerminalScriptRunner.Precondition) async -> Delivery {
        switch route {
        case .tmux:
            guard let target = await TmuxController.shared.findTmuxTarget(forTTY: tty) else { return .failed }
            return await TmuxMessageSender.shared.send(TerminalScript.singleLine(message), to: target, precondition: check)
                ? .delivered : .failed
        case .scripted(let terminal):
            guard let script = TerminalScript.send(message, to: terminal, tty: tty) else { return .failed }
            // Raise macOS's Automation prompt with a query that changes
            // nothing, so the typing script never sits on it while Claude
            // moves on underneath.
            guard await ensureAutomation(for: terminal) else { return .failed }
            switch await TerminalScriptRunner.shared.run(script, label: "send to \(terminal.displayName)", precondition: check) {
            case .succeeded: break
            case .blocked(let reason): return .refused(reason)
            case .notFound, .failed: return .failed
            }
            // iTerm2: Return as its own write, a moment later, checked again.
            guard let submit = TerminalScript.submit(to: terminal, tty: tty) else { return .delivered }
            try? await Task.sleep(for: .seconds(TerminalScript.submitDelay))
            switch await TerminalScriptRunner.shared.run(submit, label: "submit in \(terminal.displayName)", precondition: check) {
            case .succeeded: return .delivered
            case .blocked(let reason): return .typedNotSubmitted(reason)
            case .notFound, .failed: return .failed
            }
        }
    }

    /// Asks again, on the engine's latest state and the live process, why
    /// typing must not happen now (nil: go ahead).
    static func recheck(sessionId: String, tty: String) async -> String? {
        guard let session = ClaudeSessionMonitor.shared.instances.first(where: { $0.sessionId == sessionId }) else {
            return "The session has ended"
        }
        guard session.tty.flatMap(TerminalScript.devicePath(forTTY:)) == TerminalScript.devicePath(forTTY: tty) else {
            return "The session's terminal can't be confirmed"
        }
        guard let pid = session.pid else { return "The session's process isn't known" }
        let process = await Self.status(ofPid: pid)
        return MessageSafety.reason(for: session, process: process)
    }

    private static func precondition(for sessionId: String, tty: String) -> TerminalScriptRunner.Precondition {
        { await SessionMessenger.recheck(sessionId: sessionId, tty: tty) }
    }

    /// Whether Automation access to `terminal` is granted, asking for it
    /// first (by reading the selected tab's TTY, which changes nothing) when
    /// it hasn't been decided.
    private static func ensureAutomation(for terminal: ScriptableTerminal) async -> Bool {
        if await AutomationPermission.isGranted(bundleIdentifier: terminal.bundleIdentifier) { return true }
        _ = await TerminalScriptRunner.shared.run(TerminalScript.selectedTTY(terminal), label: "ask for Automation access to \(terminal.displayName)")
        return await AutomationPermission.isGranted(bundleIdentifier: terminal.bundleIdentifier)
    }

    /// The engine's latest state of `session` (its attention, pid and TTY
    /// move on while a view holds an older copy); the copy itself for a
    /// session the engine no longer tracks.
    static func current(_ session: SessionState) -> SessionState {
        ClaudeSessionMonitor.shared.instances.first { $0.sessionId == session.sessionId } ?? session
    }

    @concurrent
    nonisolated private static func status(ofPid pid: Int) async -> ProcessStatus? {
        ProcessTreeBuilder.shared.status(ofPid: pid)
    }
}
