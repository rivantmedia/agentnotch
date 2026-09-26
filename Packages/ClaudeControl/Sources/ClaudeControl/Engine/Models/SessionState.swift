//
//  SessionState.swift
//  ClaudeIsland
//
//  Unified state model for a Claude session.
//  Consolidates all state that was previously spread across multiple components.
//

import Foundation

/// Complete state for a single Claude session
/// This is the single source of truth - all state reads and writes go through SessionStore
nonisolated struct SessionState: Equatable, Identifiable, Sendable {
    // MARK: - Identity

    let sessionId: String
    /// Working directory the session started in (identity; never changes).
    let cwd: String
    let projectName: String
    /// Latest working directory reported by an event, for display. Starts as `cwd`.
    var currentCwd: String

    // MARK: - Instance Metadata

    var pid: Int?
    /// When `pid` started (from the kernel), so a reused pid isn't taken for
    /// the same Claude process.
    var pidStartedAt: Date?
    var tty: String?
    var isInTmux: Bool

    // MARK: - Account & Origin

    /// Absolute transcript JSONL path (`<configDir>/projects/<slug>/<sessionId>.jsonl`).
    /// Always taken from the hook when available; computing it from `cwd` is a fallback.
    var transcriptPath: String?
    /// The account (normalized config dir) the session runs under.
    var accountId: String?
    /// Raw CLAUDE_CONFIG_DIR of the Claude process; nil for the default account.
    var configDirEnv: String?
    /// CLAUDE_CODE_ENTRYPOINT / registry entrypoint, e.g. `cli`, `claude-vscode`.
    var entrypoint: String?
    /// Claude Desktop's id for the session, when Desktop hosts it: the
    /// current process's registry entry's `hostSessionId`.
    var hostSessionId: String?

    // MARK: - Session Details

    /// Best known title: hook `session_title` > registry `name` > transcript
    /// custom/AI title > a name Claude Code derived itself. See `titleSource`.
    var sessionTitle: String?
    /// Where `sessionTitle` came from; a lower-priority source never overwrites a higher one.
    var titleSource: SessionTitleSource?
    /// The registry's `name` when Claude Code made it up (`nameSource: "derived"`,
    /// e.g. "my-project-3"), so the same string from the status line isn't
    /// mistaken for a name the user chose.
    var derivedName: String?
    /// Model id or display name (SessionStart `model`, status line `model`).
    var model: String?
    var permissionMode: String?

    // MARK: - Turn & Review Tracking

    /// Text of Claude's final reply in the last completed turn (Stop), for review previews.
    var lastAssistantMessage: String?
    /// When the current (or last) turn started (UserPromptSubmit).
    var turnStartedAt: Date?
    /// When Claude last finished a turn the user asked for (Stop).
    var completedAt: Date?
    /// When the user last looked at the session (prompt, chat opened, marked reviewed).
    var reviewedAt: Date?
    /// A main-session Stop not yet confirmed as the end of the turn: Claude
    /// Code runs Stop hooks after ours (a blocking one, like /goal, makes
    /// Claude continue), and only its session registry going idle says the
    /// turn is over. `completedAt` is set from this once confirmed.
    var completionPendingSince: Date?
    /// Humanized StopFailure error of the last turn, if it failed.
    var stopError: String?
    /// StopFailure's raw `error` code (`rate_limit`, `billing_error`, …).
    var stopErrorCode: String?
    /// When the last turn failed (StopFailure).
    var stopErrorAt: Date?
    /// Explicit reason the session is blocked on the user, beyond a pending approval.
    var needsInputReason: NeedsInputReason? {
        didSet {
            if needsInputReason == nil {
                needsInputSince = nil
            } else if oldValue == nil {
                needsInputSince = Date()
            }
        }
    }
    /// When `needsInputReason` was set (nil while there is none).
    private(set) var needsInputSince: Date?

    /// Sets the needs-input reason as of `date` (when the event saying so
    /// arrived, not when it was processed). A reason that only changes
    /// keeps its original time.
    mutating func setNeedsInput(_ reason: NeedsInputReason?, at date: Date) {
        let wasWaiting = needsInputReason != nil
        needsInputReason = reason
        if reason != nil && !wasWaiting {
            needsInputSince = date
        }
    }
    /// Background tasks (e.g. background Bash, agents) still running after the turn ended.
    var backgroundTaskCount: Int
    /// Of those, the ones the turn waits for (subagents, workflows,
    /// teammates, cloud sessions; see `BackgroundWork.awaitedTypes`): each
    /// wakes Claude for another turn when it finishes.
    var backgroundAgentCount: Int = 0
    /// Their `type` labels, for "Waiting on 1 workflow".
    var backgroundAgentTypes: [String] = []
    /// Set by a Stop that left such agents running: the work Claude started
    /// isn't done, so the session shows as working, not ready for review.
    /// The next Stop says again what is still running; until then (a wake-up
    /// or typed turn that is interrupted, or fails) the wait stands, since
    /// neither stops background agents. It also ends when the session
    /// registry shows no agent left (see `SessionStore.settleBackgroundWait`).
    var backgroundWaitSince: Date?
    /// Crons and wake-ups (/loop, ScheduleWakeup) scheduled at the last Stop.
    var scheduledWakeupCount: Int = 0
    /// `source` of the prompt that started the current or last turn.
    var lastPromptSource: String?
    /// The user typed the prompt that started the current or last turn.
    var lastPromptWasUserAuthored: Bool = false
    /// Waking agents and crons as the last Stop left them (not reset by a
    /// prompt, so a turn interrupted before its Stop keeps them)...
    var knownWakingAgents: Int = 0
    var knownWakeups: Int = 0
    /// ...and as they stood when the current or last turn started: a
    /// completion is quiet only when its own turn added one (BHV-1).
    var agentsAtTurnStart: Int = 0
    var wakeupsAtTurnStart: Int = 0

    // MARK: - Progress & Usage

    /// Task / todo progress for the session.
    var tasks: SessionTaskList
    /// Context window used, 0...100. From the status line, else estimated from the transcript.
    var contextUsedPercent: Double?
    /// Context window size in tokens, when the status line reported it.
    var contextWindowSize: Int?
    /// When the status line last reported the context window (it then wins over estimates).
    var statusLineUpdatedAt: Date?
    /// Session cost in USD from the status line.
    var costUSD: Double?

    // MARK: - State Machine

    /// Current phase in the session lifecycle
    var phase: SessionPhase

    /// Further permission requests waiting behind the one in `phase`, oldest
    /// first. Each keeps its full context so it can be answered when it comes up.
    var queuedApprovals: [PermissionContext] = []

    /// Phase to return to once the pending approvals are answered: `.processing`
    /// mid-turn, or the finished phase when a background agent asked after Stop.
    var phaseAfterApprovals: SessionPhase = .processing

    // MARK: - Chat History

    /// Chat items for this session. Only the newest ones (and any tool still
    /// running or waiting) are kept unless the chat is open; see
    /// `SessionStore.chatRetention`.
    var chatItems: [ChatHistoryItem] {
        didSet { chatRevision &+= 1 }
    }

    /// Bumped on every change to `chatItems`, so observers compare one
    /// integer instead of whole histories.
    private(set) var chatRevision: Int = 0

    // MARK: - Tool Tracking

    /// Tools the hooks reported started and not finished yet, for "what is
    /// it doing now" (the newest one).
    var toolTracker: ToolTracker

    // MARK: - Subagent State

    /// State for Task tools and their nested subagent tools
    var subagentState: SubagentState

    // MARK: - Conversation Info (from JSONL parsing)

    var conversationInfo: ConversationInfo

    // MARK: - Timestamps

    var lastActivity: Date
    var createdAt: Date
    /// Time of the last hook event, status line update or registry change applied.
    var lastEventAt: Date
    /// Time of the last hook event only. Registry corrections apply when the
    /// registry changed after it; status line updates don't move it.
    var lastHookEventAt: Date?
    /// The session registry's last status for this session
    /// (`busy`, `idle`, `shell`, `waiting`) and when it changed.
    var registryStatus: String?
    var registryStatusChangedAt: Date?
    /// A turn may have ended where no hook said so (a registry-only session
    /// went idle, or the app was not running): the next transcript sync
    /// decides whether it completed. The date is the earliest the
    /// completion can be.
    var completionCheckSince: Date?

    // MARK: - Identifiable

    var id: String { sessionId }

    // MARK: - Initialization

    nonisolated init(
        sessionId: String,
        cwd: String,
        projectName: String? = nil,
        pid: Int? = nil,
        tty: String? = nil,
        isInTmux: Bool = false,
        phase: SessionPhase = .idle,
        chatItems: [ChatHistoryItem] = [],
        toolTracker: ToolTracker = ToolTracker(),
        subagentState: SubagentState = SubagentState(),
        conversationInfo: ConversationInfo = ConversationInfo(
            summary: nil, lastMessage: nil, lastMessageRole: nil,
            lastToolName: nil, firstUserMessage: nil, lastUserMessageDate: nil
        ),
        lastActivity: Date = Date(),
        createdAt: Date = Date()
    ) {
        self.sessionId = sessionId
        self.cwd = cwd
        self.projectName = projectName ?? URL(fileURLWithPath: cwd).lastPathComponent
        self.currentCwd = cwd
        self.backgroundTaskCount = 0
        self.tasks = SessionTaskList()
        self.lastEventAt = lastActivity
        self.pid = pid
        self.tty = tty
        self.isInTmux = isInTmux
        self.phase = phase
        self.chatItems = chatItems
        self.toolTracker = toolTracker
        self.subagentState = subagentState
        self.conversationInfo = conversationInfo
        self.lastActivity = lastActivity
        self.createdAt = createdAt
    }

    // MARK: - Derived Properties

    /// The active permission context, if any
    var activePermission: PermissionContext? {
        if case .waitingForApproval(let ctx) = phase {
            return ctx
        }
        return nil
    }

    /// When the session started waiting for the user: its shown request's
    /// activation, else when the needs-input reason was set.
    var waitingSince: Date? {
        if let active = activePermission {
            return active.activatedAt ?? active.receivedAt
        }
        return needsInputSince
    }

    /// Every request waiting for an answer: the active one first, then the queue.
    var pendingPermissions: [PermissionContext] {
        (activePermission.map { [$0] } ?? []) + queuedApprovals
    }

    /// A pending request by tool_use_id, active or queued.
    func pendingPermission(toolUseId: String) -> PermissionContext? {
        pendingPermissions.first { $0.toolUseId == toolUseId }
    }

    /// Hooks report this session (not only the registry or status line).
    var isHookBacked: Bool {
        lastHookEventAt != nil
    }

    /// StopFailure's error, grouped by what the user can do about it; nil
    /// once the failure no longer blocks the session (the user moved on, or
    /// Claude did).
    var stopErrorKind: StopErrorKind? {
        guard stopError != nil, hasFailedTurn else { return nil }
        return StopErrorKind(code: stopErrorCode) ?? .other
    }

    /// The turn failed (StopFailure), as opposed to waiting on something
    /// the user can answer.
    var hasFailedTurn: Bool {
        needsInputReason?.isError == true
    }

    /// A finished turn that is not worth an alert (it stays in the review
    /// queue): a /loop or cron tick, or a turn that left Claude paused,
    /// waiting to be woken — it scheduled a new wake-up (/loop, CronCreate,
    /// ScheduleWakeup) or started agents that will wake it. Its later, final
    /// turn is announced instead.
    ///
    /// A cron or teammate that was already there when the turn began does
    /// not make it quiet: a /loop stays in the session between ticks, and
    /// every turn the user types there must still be announced (BHV-1). A
    /// turn the system started (an agent's result woke Claude) stays quiet
    /// while any waking agent is still out.
    var completionIsQuiet: Bool {
        if lastPromptSource == "loop_wakeup" || lastPromptSource == "schedule_wakeup" { return true }
        if scheduledWakeupCount > wakeupsAtTurnStart { return true }
        return lastPromptWasUserAuthored
            ? backgroundAgentCount > agentsAtTurnStart
            : backgroundAgentCount > 0
    }

    /// Display title: session title > transcript summary > first user message >
    /// derived registry name > project name
    nonisolated var displayTitle: String {
        if let sessionTitle, !sessionTitle.isEmpty, titleSource != .derivedName { return sessionTitle }
        return conversationInfo.summary ?? conversationInfo.firstUserMessage ?? sessionTitle ?? projectName
    }

    /// Project folder name of the latest working directory, for display.
    nonisolated var displayProjectName: String {
        URL(fileURLWithPath: currentCwd).lastPathComponent
    }

    // MARK: - Attention

    /// What the session needs from the user; see `SessionAttention.derive`.
    nonisolated var attention: SessionAttention {
        SessionAttention.derive(
            phase: phase,
            needsInputReason: needsInputReason,
            backgroundTaskCount: backgroundTaskCount,
            completedAt: completedAt,
            reviewedAt: reviewedAt,
            completionPending: completionPendingSince != nil,
            awaitingBackgroundAgents: backgroundWaitSince != nil
        )
    }

    /// The turn is over and the session waits on the background agents or
    /// workflows it started (not while Claude works on a turn again, woken
    /// by one of them or by a prompt: that turn's own progress shows then).
    nonisolated var isAwaitingBackgroundWork: Bool {
        backgroundWaitSince != nil && phase != .processing && phase != .compacting
    }

    /// "1 workflow", "2 background agents and 1 teammate": what the session
    /// waits on while `isAwaitingBackgroundWork`, else nil.
    nonisolated var backgroundWaitDescription: String? {
        guard isAwaitingBackgroundWork else { return nil }
        return BackgroundWork.phrase(types: backgroundAgentTypes)
    }

    /// Finished work the user hasn't looked at yet.
    nonisolated var isReadyForReview: Bool {
        attention == .readyForReview
    }

    /// Last message content
    var lastMessage: String? {
        conversationInfo.lastMessage
    }

    /// Last message role
    var lastMessageRole: String? {
        conversationInfo.lastMessageRole
    }

    /// Last tool name
    var lastToolName: String? {
        conversationInfo.lastToolName
    }

    /// Summary
    var summary: String? {
        conversationInfo.summary
    }

    /// First user message
    var firstUserMessage: String? {
        conversationInfo.firstUserMessage
    }

    /// Last user message date
    var lastUserMessageDate: Date? {
        conversationInfo.lastUserMessageDate
    }

    /// Token usage for this session
    var usage: UsageInfo {
        conversationInfo.usage
    }
}

// MARK: - Title Source

/// Where a session title came from, lowest priority first.
nonisolated enum SessionTitleSource: Int, Comparable, Sendable {
    /// A registry `name` Claude Code derived from the project folder
    /// (`nameSource: "derived"`); less telling than any transcript title.
    case derivedName = 0
    /// `summary` / `ai-title` / `custom-title` lines in the transcript.
    case transcript = 1
    /// A chosen `name` in the session registry or the status line's `session_name`.
    case registry = 2
    /// `session_title` from a SessionStart / UserPromptSubmit hook.
    case hook = 3

    static func < (lhs: SessionTitleSource, rhs: SessionTitleSource) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

extension SessionState {
    /// Sets the title unless a higher-priority source already provided one.
    /// Returns true if the title changed.
    @discardableResult
    nonisolated mutating func applyTitle(_ title: String?, source: SessionTitleSource) -> Bool {
        guard let title = title?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty else { return false }
        if let current = titleSource, current > source { return false }
        guard sessionTitle != title || titleSource != source else { return false }
        sessionTitle = title
        titleSource = source
        return true
    }

    /// Applies a registry or status line name. A name equal to the registry's
    /// derived name ranks below transcript titles, whichever source reported it.
    @discardableResult
    nonisolated mutating func applyName(_ name: String?, isDerived: Bool = false) -> Bool {
        guard let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else { return false }
        if isDerived {
            derivedName = name
            // The status line may have reported it first, as a chosen name.
            if titleSource == .registry && sessionTitle == name {
                titleSource = .derivedName
                return true
            }
        }
        let derived = isDerived || name == derivedName
        return applyTitle(name, source: derived ? .derivedName : .registry)
    }
}

// MARK: - Tool Tracker

/// Tool calls started (PreToolUse) and not finished (PostToolUse,
/// PostToolUseFailure, PermissionDenied). The main session's calls are
/// dropped when its turn ends, so a call whose end never came (an
/// interrupt, a crash) doesn't linger; background agents' calls stay until
/// they finish, and at most `maxInProgress` are kept.
nonisolated struct ToolTracker: Equatable, Sendable {
    /// Tools in progress, keyed by tool_use_id.
    var inProgress: [String: ToolInProgress]

    /// Most calls followed at once (the newest are kept).
    static let maxInProgress = 64

    nonisolated init(inProgress: [String: ToolInProgress] = [:]) {
        self.inProgress = inProgress
    }

    /// The most recently started tool still in progress.
    var newest: ToolInProgress? {
        inProgress.values.max { ($0.startTime, $0.id) < ($1.startTime, $1.id) }
    }

    /// A tool started. `agentId` is the subagent that called it (nil: the main session).
    nonisolated mutating func startTool(id: String, name: String, agentId: String? = nil, at date: Date = Date()) {
        guard inProgress[id] == nil else { return }
        inProgress[id] = ToolInProgress(id: id, name: name, startTime: date, phase: .running, agentId: agentId)
        guard inProgress.count > Self.maxInProgress else { return }
        let oldest = inProgress.values.sorted { $0.startTime < $1.startTime }.prefix(inProgress.count - Self.maxInProgress)
        for tool in oldest {
            inProgress.removeValue(forKey: tool.id)
        }
    }

    /// The tool finished (successfully or not).
    nonisolated mutating func completeTool(id: String, success: Bool = true) {
        inProgress.removeValue(forKey: id)
    }

    /// The tool waits for permission, or runs again once it was granted.
    nonisolated mutating func setPhase(_ phase: ToolInProgressPhase, id: String) {
        inProgress[id]?.phase = phase
    }

    /// The main turn ended: its calls are over; background agents' go on.
    nonisolated mutating func endMainTurn() {
        inProgress = inProgress.filter { $0.value.agentId != nil }
    }
}

/// A tool currently in progress
nonisolated struct ToolInProgress: Equatable, Sendable {
    let id: String
    let name: String
    let startTime: Date
    var phase: ToolInProgressPhase
    /// The subagent that called it; nil for the main session.
    var agentId: String? = nil
}

/// Phase of a tool in progress
nonisolated enum ToolInProgressPhase: Equatable, Sendable {
    case starting
    case running
    case pendingApproval
}

// MARK: - Subagent State

/// State for Task (subagent) tools
nonisolated struct SubagentState: Equatable, Sendable {
    /// Active Task tools, keyed by task tool_use_id
    var activeTasks: [String: TaskContext]

    /// Ordered stack of active task IDs (most recent last) - used for proper tool assignment
    /// When multiple Tasks run in parallel, we use insertion order rather than timestamps
    var taskStack: [String]

    /// Mapping of agentId to Task description (for AgentOutputTool display)
    var agentDescriptions: [String: String]

    nonisolated init(activeTasks: [String: TaskContext] = [:], taskStack: [String] = [], agentDescriptions: [String: String] = [:]) {
        self.activeTasks = activeTasks
        self.taskStack = taskStack
        self.agentDescriptions = agentDescriptions
    }

    /// Whether there's an active subagent
    nonisolated var hasActiveSubagent: Bool {
        !activeTasks.isEmpty
    }

    /// Start tracking a Task tool
    nonisolated mutating func startTask(taskToolId: String, description: String? = nil) {
        activeTasks[taskToolId] = TaskContext(
            taskToolId: taskToolId,
            startTime: Date(),
            agentId: nil,
            description: description,
            subagentTools: []
        )
    }

    /// Stop tracking a Task tool
    nonisolated mutating func stopTask(taskToolId: String) {
        activeTasks.removeValue(forKey: taskToolId)
    }

    /// Add a subagent tool to the most recent active Task
    nonisolated mutating func addSubagentTool(_ tool: SubagentToolCall) {
        // Find most recent active task (for parallel Task support)
        guard let mostRecentTaskId = activeTasks.keys.max(by: {
            (activeTasks[$0]?.startTime ?? .distantPast) < (activeTasks[$1]?.startTime ?? .distantPast)
        }) else { return }

        activeTasks[mostRecentTaskId]?.subagentTools.append(tool)
    }

    /// Update the status of a subagent tool across all active Tasks
    nonisolated mutating func updateSubagentToolStatus(toolId: String, status: ToolStatus) {
        for taskId in activeTasks.keys {
            if let index = activeTasks[taskId]?.subagentTools.firstIndex(where: { $0.id == toolId }) {
                activeTasks[taskId]?.subagentTools[index].status = status
                return
            }
        }
    }
}

/// Context for an active Task tool
nonisolated struct TaskContext: Equatable, Sendable {
    let taskToolId: String
    let startTime: Date
    var agentId: String?
    var description: String?
    var subagentTools: [SubagentToolCall]
}
