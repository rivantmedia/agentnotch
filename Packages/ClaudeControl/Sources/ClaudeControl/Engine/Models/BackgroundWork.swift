//
//  BackgroundWork.swift
//  ClaudeControl
//
//  The background tasks a turn waits for, among those its Stop reports
//  (`background_tasks[].type` in the Stop hook's input), and how to name
//  them. Claude Code 2.1.x sends a friendly label for each running task
//  ("shell", "subagent", "monitor", "workflow", "MCP task", "teammate",
//  "dream", "auto-mode scan", "cloud session"), and the raw discriminant
//  for a kind it has no label for.
//

import Foundation

nonisolated enum BackgroundWork {
    /// Agents, workflows, teammates and cloud sessions: each wakes Claude
    /// for another turn when it finishes, fails or is stopped, so a turn
    /// that leaves one running isn't done. They are also what keeps Claude
    /// Code's session registry "busy" after the turn (see `decide`).
    /// Background shells, monitors and MCP tasks wake Claude too, but a
    /// shell or monitor may run for the whole session (a dev server, a file
    /// watcher), and the registry doesn't count MCP tasks, so none of them
    /// holds a session "working". "dream" and "auto-mode scan" are Claude
    /// Code's own housekeeping and never wake it.
    static let awaitedTypes: Set<String> = [
        "subagent", "agent", "local_agent",
        "workflow", "local_workflow",
        "teammate", "in_process_teammate",
        "cloud session", "remote_agent", "remote_session",
    ]

    private static let workflowTypes: Set<String> = ["workflow", "local_workflow"]
    private static let teammateTypes: Set<String> = ["teammate", "in_process_teammate"]

    /// "1 workflow", "2 background agents", "1 workflow and 1 teammate";
    /// nil for none. `types` are awaited ones (see `awaitedTypes`).
    static func phrase(types: [String]) -> String? {
        let workflows = types.filter(workflowTypes.contains).count
        let teammates = types.filter(teammateTypes.contains).count
        let agents = types.count - workflows - teammates
        let parts = [
            counted(workflows, "workflow"),
            counted(agents, "background agent"),
            counted(teammates, "teammate"),
        ].compactMap { $0 }
        guard let last = parts.last else { return nil }
        return parts.count == 1 ? last : parts.dropLast().joined(separator: ", ") + " and " + last
    }

    private static func counted(_ count: Int, _ noun: String) -> String? {
        count > 0 ? "\(count) \(noun)\(count == 1 ? "" : "s")" : nil
    }
}

// MARK: - When the wait is over

nonisolated extension BackgroundWork {
    enum WaitDecision: Equatable, Sendable {
        /// Still waiting; decide again after this long if nothing else
        /// happens (nil: only a registry change or a hook can end it).
        case keep(recheckIn: TimeInterval?)
        /// No awaited work is left (or none has shown a sign of life for a
        /// long time); it ended at `at`.
        case end(at: Date)
    }

    struct WaitTiming: Equatable, Sendable {
        /// How long the registry must report no agent work before the wait
        /// ends without Claude being woken. A finished agent's wake-up turn
        /// starts within moments and ends the wait itself, so this only
        /// matters when no wake-up comes.
        var registryGrace: TimeInterval
        /// How long without a single hook event (background agents send
        /// theirs) before giving up while the registry still says busy (a
        /// paused workflow keeps it busy and sends nothing) or doesn't
        /// follow the session at all.
        var quietTimeout: TimeInterval

        static let standard = WaitTiming(registryGrace: 10, quietTimeout: 30 * 60)
    }

    /// Whether a background wait that began at `waitSince` is over. Claude
    /// Code 2.1.x keeps the session registry "busy" while the turn runs or
    /// any agent, workflow (paused ones too), cloud session or active
    /// teammate of the session is unfinished, in the terminal and in the SDK
    /// alike; "shell" when only shells or monitors are left, and "idle" when
    /// nothing is. Idle teammates and long-running cloud sessions leave it
    /// idle, so a wait on them ends after the grace; their later reports
    /// wake Claude for a turn of their own. Pure.
    static func decide(
        waitSince: Date,
        registryStatus: String?,
        registryChangedAt: Date?,
        lastHookEventAt: Date?,
        now: Date,
        timing: WaitTiming = .standard
    ) -> WaitDecision {
        if let status = registryStatus, status == "idle" || status == "shell" {
            let quietSince = max(registryChangedAt ?? waitSince, waitSince)
            let deadline = quietSince.addingTimeInterval(timing.registryGrace)
            return now >= deadline ? .end(at: quietSince) : .keep(recheckIn: deadline.timeIntervalSince(now))
        }
        let quietSince = max(lastHookEventAt ?? waitSince, waitSince)
        let deadline = quietSince.addingTimeInterval(timing.quietTimeout)
        return now >= deadline ? .end(at: quietSince) : .keep(recheckIn: deadline.timeIntervalSince(now))
    }
}
