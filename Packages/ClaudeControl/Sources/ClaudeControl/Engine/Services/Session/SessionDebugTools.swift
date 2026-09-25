//
//  SessionDebugTools.swift
//  ClaudeControl
//
//  Developer aids for verifying the session pipeline without seeing the notch:
//
//  --dump-state (or AGENTNOTCH_DUMP_STATE=1): print a one-line summary per session
//    to stdout whenever the published state changes, e.g.
//      [agentnotch-state] 1f2e3d4c acct=.claude-work attn=needsInput(permission:Bash) phase=waitingForApproval(Bash) tasks=2/5 ctx=42% title="Fix login"
//
//  --dev-console (or AGENTNOTCH_DEV_CONSOLE=1): read commands from stdin and drive
//    the same APIs the UI uses:
//      approve <id-prefix> [always]      deny <id-prefix> [reason...]
//      answer <id-prefix> <question>=<label>[|<question>=<label>...]
//      review <id-prefix>                review-all
//      dump
//
//  Both are off unless explicitly requested at launch.
//

import Foundation

// MARK: - State Dump

nonisolated enum SessionDebugFlags {
    static var dumpState: Bool { DevFlags.dumpState }
    static var devConsole: Bool { DevFlags.devConsole }

    /// Writes a line to stdout unbuffered, so `grep` on a pipe sees it immediately.
    static func printLine(_ line: String) {
        FileHandle.standardOutput.write(Data((line + "\n").utf8))
    }
}

@MainActor
final class SessionStateDump {
    private var lastDump: String?
    private var publishCount = 0

    func dumpIfEnabled(_ sessions: [SessionState], force: Bool = false) {
        guard SessionDebugFlags.dumpState || force else { return }
        publishCount += 1
        let lines = sessions
            .sorted { ($0.attention.bucket, $0.sessionId) < ($1.attention.bucket, $1.sessionId) }
            .map(Self.summary)
        let dump = lines.joined(separator: "\n")
        guard force || dump != lastDump else { return }
        lastDump = dump
        SessionDebugFlags.printLine("[agentnotch-state] publish #\(publishCount): \(sessions.count) session(s)")
        for line in lines {
            SessionDebugFlags.printLine(line)
        }
    }

    /// One-line summary of a session.
    nonisolated static func summary(_ session: SessionState) -> String {
        let account = session.accountId.map(AccountPaths.shortName(forConfigDir:)) ?? "-"
        let tasks = session.tasks.totalCount > 0 ? "\(session.tasks.completedCount)/\(session.tasks.totalCount)" : "-"
        let context = session.contextUsedPercent.map { String(format: "%.0f%%", $0) } ?? "-"
        var parts = [
            "[agentnotch-state] \(session.sessionId.prefix(8))",
            "acct=\(account)",
            "attn=\(session.attention.debugDescription)",
            "phase=\(session.phase.description)",
            "tasks=\(tasks)",
            "ctx=\(context)",
        ]
        if let active = session.tasks.activeItem {
            parts.append("active=\"\(active.activeLabel)\"")
        }
        if session.completionPendingSince != nil {
            parts.append("stop=pending")
        }
        if let queued = session.queuedApprovals.count > 0 ? session.queuedApprovals.count : nil {
            parts.append("queued=\(queued)")
        }
        if session.backgroundTaskCount > 0 {
            parts.append("bg=\(session.backgroundTaskCount)")
        }
        if session.backgroundWaitSince != nil {
            parts.append("awaiting=\(session.backgroundAgentCount)")
        }
        if session.isReadyForReview && session.completionIsQuiet {
            parts.append("quiet")
        }
        if let model = session.model {
            parts.append("model=\(model)")
        }
        parts.append("title=\"\(session.displayTitle.prefix(40))\"")
        if session.isReadyForReview, let message = session.lastAssistantMessage {
            parts.append("review=\"\(message.prefix(40).replacingOccurrences(of: "\n", with: " "))\"")
        }
        return parts.joined(separator: " ")
    }
}

// MARK: - Dev Console

@MainActor
enum SessionDevConsole {
    private static var isRunning = false

    static func startIfEnabled() {
        guard SessionDebugFlags.devConsole, !isRunning else { return }
        isRunning = true
        SessionDebugFlags.printLine("[agentnotch-console] ready: approve|deny|answer|review|review-all|dump")
        let thread = Thread {
            while let line = readLine() {
                let command = line.trimmingCharacters(in: .whitespaces)
                guard !command.isEmpty else { continue }
                Task { @MainActor in
                    execute(command)
                }
            }
        }
        thread.name = "agentnotch-dev-console"
        thread.start()
    }

    private static func execute(_ command: String) {
        let monitor = ClaudeSessionMonitor.shared
        let parts = command.split(separator: " ", maxSplits: 2, omittingEmptySubsequences: true).map(String.init)
        guard let verb = parts.first else { return }

        func session(_ prefix: String?) -> SessionState? {
            guard let prefix, !prefix.isEmpty else { return nil }
            let matches = monitor.instances.filter { $0.sessionId.hasPrefix(prefix) }
            guard matches.count == 1 else {
                SessionDebugFlags.printLine("[agentnotch-console] \(matches.isEmpty ? "no" : "ambiguous") session for '\(prefix)'")
                return nil
            }
            return matches[0]
        }

        switch verb {
        case "approve":
            guard let target = session(parts[safe: 1]) else { return }
            let always = parts[safe: 2] == "always"
            monitor.approvePermission(sessionId: target.sessionId, alwaysAllow: always)
            SessionDebugFlags.printLine("[agentnotch-console] approve \(target.sessionId.prefix(8))\(always ? " (always)" : "")")
        case "deny":
            guard let target = session(parts[safe: 1]) else { return }
            monitor.denyPermission(sessionId: target.sessionId, reason: parts[safe: 2])
            SessionDebugFlags.printLine("[agentnotch-console] deny \(target.sessionId.prefix(8))")
        case "answer":
            guard let target = session(parts[safe: 1]), let spec = parts[safe: 2] else { return }
            var answers: [String: String] = [:]
            for pair in spec.split(separator: "|") {
                let kv = pair.split(separator: "=", maxSplits: 1).map(String.init)
                if kv.count == 2 { answers[kv[0]] = kv[1] }
            }
            monitor.answerQuestion(sessionId: target.sessionId, answers: answers)
            SessionDebugFlags.printLine("[agentnotch-console] answer \(target.sessionId.prefix(8)) \(answers)")
        case "review":
            guard let target = session(parts[safe: 1]) else { return }
            monitor.markReviewed(sessionId: target.sessionId)
            SessionDebugFlags.printLine("[agentnotch-console] reviewed \(target.sessionId.prefix(8))")
        case "review-all":
            monitor.markAllReviewed()
            SessionDebugFlags.printLine("[agentnotch-console] reviewed all")
        case "dump":
            SessionStateDump().dumpIfEnabled(monitor.instances, force: true)
        default:
            SessionDebugFlags.printLine("[agentnotch-console] unknown command: \(verb)")
        }
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
