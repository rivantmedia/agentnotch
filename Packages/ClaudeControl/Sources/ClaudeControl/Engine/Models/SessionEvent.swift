//
//  SessionEvent.swift
//  ClaudeControl
//
//  Everything that can change session state. All state changes flow through
//  SessionStore.process(event); the ones that come from outside the UI
//  (hooks, status line, registry, interrupts, permission outcomes) arrive in
//  order through HookEventPipeline.
//

import Foundation

/// All events that can affect session state
/// This is the single entry point for state mutations
enum SessionEvent: Sendable {
    // MARK: - Hook Events (from HookSocketServer)

    /// A hook event was received from Claude Code
    case hookReceived(HookEvent)

    /// A status line update arrived from a live terminal session
    case statusLineReceived(StatusLineMessage)

    // MARK: - Registry Events (from SessionRegistryScanner)

    /// Live interactive sessions listed in `<configDir>/sessions/*.json`
    case registrySnapshot(configDir: String, entries: [SessionRegistryEntry])

    // MARK: - Review Events (user actions)

    /// The user looked at a session (chat opened, terminal focused, "mark
    /// reviewed") at `at`: completions after that moment stay unreviewed.
    case markReviewed(sessionId: String, at: Date = Date())

    /// The user saw the completion at `completedAt` finish (its terminal was
    /// frontmost). Reviews that completion only, never a later one.
    case markViewed(sessionId: String, completedAt: Date)

    /// The Hooks switch went off: no session hears from hooks any more, so
    /// each falls back to the registry and its transcript for "done" (a
    /// registry idle is then a finished turn to check, not an interrupt).
    case hooksTurnedOff

    /// The user forgot an account: its sessions go (their hooks are removed
    /// with it, so they would only freeze where they stand; BHV-3).
    case dropAccountSessions(accountId: String)

    /// The user dismissed a failed turn (rate limit, overload, sign-in) at
    /// `at`: it stops counting as failed until the next StopFailure.
    case dismissFailure(sessionId: String, at: Date = Date())

    /// The user cleared the whole review queue at `at`: turns that complete
    /// after the click stay unreviewed.
    case markAllReviewed(at: Date)

    // MARK: - Permission Events (user actions)

    /// User approved a permission request
    case permissionApproved(sessionId: String, toolUseId: String)

    /// User denied a permission request
    case permissionDenied(sessionId: String, toolUseId: String, reason: String?)

    /// Permission socket failed (connection died before response)
    case permissionSocketFailed(sessionId: String, toolUseId: String)

    // MARK: - Transcript Events

    /// The session's transcript grew (or its agents did)
    case fileUpdated(FileUpdatePayload)

    /// The interrupt watcher saw the user interrupt Claude at `at`.
    case interruptDetected(sessionId: String, at: Date)

    /// A Stop's completion may be confirmed now (timer fallback for sessions
    /// whose registry doesn't say when the turn is over).
    case completionCheck(sessionId: String, stopAt: Date)

    /// A turn waiting on background agents (since `since`) may be done now
    /// (see `BackgroundWork.decide`).
    case backgroundWaitCheck(sessionId: String, since: Date)

    // MARK: - Chat History

    /// A chat opened: read and keep the session's whole history
    case loadHistory(sessionId: String, cwd: String)

    /// The chat closed: keep only the newest items again
    case releaseHistory(sessionId: String)
}

/// Everything a transcript sync read, prepared before the event is processed
/// so SessionStore applies it without awaiting (no actor re-entrancy between
/// reading a session and writing it back).
nonisolated struct FileUpdatePayload: Sendable {
    let sessionId: String
    /// Transcript the payload was parsed from
    let transcriptPath: String
    /// New messages (only the newest ones unless the chat is open).
    var messages: [ChatMessage] = []
    /// The transcript was cleared (/clear) or rewritten: items it no longer
    /// has are dropped.
    var replacesHistory = false
    /// Tools whose results arrived, among the ones in `messages` or running.
    var completedToolIds: Set<String> = []
    var toolResults: [String: ConversationParser.ToolResult] = [:]
    var structuredResults: [String: ToolResultData] = [:]
    /// Title, last message, usage, context estimate, how the last turn ended.
    let conversationInfo: ConversationInfo
    /// Subagent tool lists that changed, keyed by the Agent call's tool_use_id.
    var subagentTools: [String: [SubagentToolInfo]] = [:]
    /// Task list rebuilt from the transcript. Set only on the first sync of a
    /// session discovered mid-flight (app restart, registry, status line).
    var reconstructedTasks: SessionTaskList?
    /// The task list as the transcript has it now, on every sync. Sessions no
    /// hook reports take it as their list; hook-driven ones ignore it.
    var transcriptTasks: SessionTaskList?

    init(
        sessionId: String,
        transcriptPath: String,
        conversationInfo: ConversationInfo,
        messages: [ChatMessage] = [],
        replacesHistory: Bool = false,
        completedToolIds: Set<String> = [],
        toolResults: [String: ConversationParser.ToolResult] = [:],
        structuredResults: [String: ToolResultData] = [:],
        subagentTools: [String: [SubagentToolInfo]] = [:],
        reconstructedTasks: SessionTaskList? = nil,
        transcriptTasks: SessionTaskList? = nil
    ) {
        self.sessionId = sessionId
        self.transcriptPath = transcriptPath
        self.conversationInfo = conversationInfo
        self.messages = messages
        self.replacesHistory = replacesHistory
        self.completedToolIds = completedToolIds
        self.toolResults = toolResults
        self.structuredResults = structuredResults
        self.subagentTools = subagentTools
        self.reconstructedTasks = reconstructedTasks
        self.transcriptTasks = transcriptTasks
    }
}

/// A session's whole transcript, for its chat.
nonisolated struct HistoryPayload: Sendable {
    let sessionId: String
    let transcriptPath: String
    let history: ConversationParser.HistoryResult
}

/// Result of a tool completion detected from JSONL
nonisolated struct ToolCompletionResult: Sendable {
    let status: ToolStatus
    let result: String?
    let structuredResult: ToolResultData?

    nonisolated static func from(parserResult: ConversationParser.ToolResult?, structuredResult: ToolResultData?) -> ToolCompletionResult {
        let status: ToolStatus
        if parserResult?.isInterrupted == true {
            status = .interrupted
        } else if parserResult?.isError == true {
            status = .error
        } else {
            status = .success
        }

        var resultText: String? = nil
        if let r = parserResult {
            if !r.isInterrupted {
                if let stdout = r.stdout, !stdout.isEmpty {
                    resultText = stdout
                } else if let stderr = r.stderr, !stderr.isEmpty {
                    resultText = stderr
                } else if let content = r.content, !content.isEmpty {
                    resultText = content
                }
            }
        }

        return ToolCompletionResult(status: status, result: resultText, structuredResult: structuredResult)
    }
}

// MARK: - Hook Event Extensions

extension HookEvent {
    /// Target phase for this event, or nil when the event must not change the
    /// phase (informational notifications, SessionEnd which removes the session).
    /// Late-event protection and completion tracking live in SessionStore.
    nonisolated func determinePhase() -> SessionPhase? {
        switch event {
        case "PreCompact":
            return .compacting
        case "PostCompact":
            // A manual /compact runs while the prompt is idle; an automatic one mid-turn.
            return trigger == "manual" ? .waitingForInput : .processing
        case "PermissionRequest":
            return .waitingForApproval(PermissionContext(
                toolUseId: toolUseId ?? "",
                toolName: tool ?? "unknown",
                toolInput: toolInput,
                receivedAt: receivedAt,
                permissionSuggestions: permissionSuggestions,
                hasSyntheticToolUseId: hasSyntheticToolUseId,
                agentId: isSubagentEvent ? agentId : nil
            ))
        case "Notification":
            // idle_prompt fires ~60 s after Claude went idle: the turn is over.
            // Every other notification only sets or clears a needs-input reason.
            return notificationType == "idle_prompt" ? .waitingForInput : nil
        case "SessionStart" where source == "compact":
            // Fired after every compaction, including an automatic one in the
            // middle of a turn: the turn goes on, so the phase must not move
            // to waitingForInput (the turn's Stop would then not count as a
            // completion). PreCompact / PostCompact drive the phase instead.
            return nil
        case "SessionEnd":
            return nil
        default:
            break
        }

        switch status {
        case "waiting_for_input":
            return .waitingForInput
        case "running_tool", "processing", "starting":
            return .processing
        case "compacting":
            return .compacting
        case "waiting_for_approval":
            return nil  // Only PermissionRequest carries an approval context
        default:
            return nil
        }
    }

    /// Events that start or continue a turn of the main session. Every other
    /// event that maps to `.processing` (the PostToolUse of a backgrounded Bash,
    /// a SubagentStop, TaskCompleted, events from background subagents) can
    /// land after Stop and must not drag a finished session back to processing.
    /// An automatic compaction never starts a turn: it happens inside one
    /// (the session is processing already), or in a background agent or
    /// teammate after the main Stop (PreCompact/PostCompact carry no agent_id).
    nonisolated var resumesTurn: Bool {
        guard !isSubagentEvent else { return false }
        switch event {
        case "UserPromptSubmit", "PreToolUse":
            return true
        case "PreCompact", "PostCompact":
            return trigger != "auto"
        default:
            return false
        }
    }

    /// Whether this event should trigger a file sync
    nonisolated var shouldSyncFile: Bool {
        switch event {
        case "UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "StopFailure", "SubagentStop":
            return true
        default:
            return false
        }
    }

    /// The main session's turn ended (Stop or StopFailure without agent_id).
    nonisolated var endsMainTurn: Bool {
        !isSubagentEvent && (event == "Stop" || event == "StopFailure")
    }
}

// MARK: - Debug Description

extension SessionEvent: CustomStringConvertible {
    nonisolated var description: String {
        switch self {
        case .hookReceived(let event):
            return "hookReceived(\(event.event), session: \(event.sessionId.prefix(8)))"
        case .statusLineReceived(let message):
            return "statusLineReceived(session: \(message.sessionId.prefix(8)))"
        case .registrySnapshot(let configDir, let entries):
            return "registrySnapshot(\(AccountPaths.shortName(forConfigDir: configDir)), entries: \(entries.count))"
        case .markReviewed(let sessionId, _):
            return "markReviewed(session: \(sessionId.prefix(8)))"
        case .markViewed(let sessionId, _):
            return "markViewed(session: \(sessionId.prefix(8)))"
        case .markAllReviewed:
            return "markAllReviewed"
        case .hooksTurnedOff:
            return "hooksTurnedOff"
        case .dropAccountSessions(let accountId):
            return "dropAccountSessions(\(AccountPaths.shortName(forConfigDir: accountId)))"
        case .dismissFailure(let sessionId, _):
            return "dismissFailure(session: \(sessionId.prefix(8)))"
        case .permissionApproved(let sessionId, let toolUseId):
            return "permissionApproved(session: \(sessionId.prefix(8)), tool: \(toolUseId.prefix(12)))"
        case .permissionDenied(let sessionId, let toolUseId, _):
            return "permissionDenied(session: \(sessionId.prefix(8)), tool: \(toolUseId.prefix(12)))"
        case .permissionSocketFailed(let sessionId, let toolUseId):
            return "permissionSocketFailed(session: \(sessionId.prefix(8)), tool: \(toolUseId.prefix(12)))"
        case .fileUpdated(let payload):
            return "fileUpdated(session: \(payload.sessionId.prefix(8)), messages: \(payload.messages.count))"
        case .interruptDetected(let sessionId, _):
            return "interruptDetected(session: \(sessionId.prefix(8)))"
        case .completionCheck(let sessionId, _):
            return "completionCheck(session: \(sessionId.prefix(8)))"
        case .backgroundWaitCheck(let sessionId, _):
            return "backgroundWaitCheck(session: \(sessionId.prefix(8)))"
        case .loadHistory(let sessionId, _):
            return "loadHistory(session: \(sessionId.prefix(8)))"
        case .releaseHistory(let sessionId):
            return "releaseHistory(session: \(sessionId.prefix(8)))"
        }
    }
}
