//
//  SessionStore.swift
//  ClaudeControl
//
//  Central state manager for all Claude sessions.
//  Single source of truth - all state mutations flow through process().
//
//  Hook, status line, registry, interrupt and permission-outcome messages
//  arrive through HookEventPipeline's single consumer, so they are processed
//  strictly in order. Handlers never await between reading a session and
//  writing it back: file I/O happens before the event is processed (see
//  FileUpdatePayload), which keeps the actor's re-entrancy from losing
//  updates or resurrecting removed sessions.
//
//  Every time the store records (turn start, completion, review, failure) is
//  the time the socket server received the event, not the time it was
//  processed, so a backlog can't reorder them against the user's actions.
//
//  Publishing is coalesced: a burst of events costs one publish (and one
//  render) per `publishInterval`, and an unchanged state isn't published.
//

import Combine
import Foundation
import os.log

/// Central state manager for all Claude sessions
/// Uses Swift actor for thread-safe state mutations
actor SessionStore {
    static let shared = SessionStore()

    /// Logger for session store (nonisolated static for cross-context access)
    nonisolated static var logger: Logger { EngineLog.logger("Session") }

    /// Claude Code's user message after an automatic compaction; a Stop right
    /// after it is not a finished task.
    static let contextResumePrefix = "This session is being continued from a previous conversation"

    // MARK: - Tuning

    /// Chat items kept for a session whose chat isn't open (plus any tool
    /// still running or waiting for approval).
    static let retainedChatItems = 40
    /// A Stop becomes a completion this long after it when the session
    /// registry can't say when the turn really ended.
    static let completionFallbackDelay: TimeInterval = 4
    /// ...and this long when the registry follows the session but never
    /// reported it idle (a safety net; normally the registry settles it).
    static let completionRegistryTimeout: TimeInterval = 90
    /// Registry timestamps and hook times come from different clocks and
    /// writers; this much disagreement is tolerated.
    static let registryClockTolerance: TimeInterval = 1

    /// Background task types a turn waits for; see `BackgroundWork.awaitedTypes`.
    static let wakingBackgroundTaskTypes = BackgroundWork.awaitedTypes

    // MARK: - State

    /// All sessions keyed by sessionId
    private var sessions: [String: SessionState] = [:]

    /// Sessions that ended recently; late status line / registry data must not revive them.
    private var recentlyEnded: [String: Date] = [:]

    /// Sessions first seen mid-flight whose task list should be rebuilt from the transcript
    private var needsTaskReconstruction: Set<String> = []

    /// Sessions whose chat is open: their whole history is kept.
    private var openHistories: Set<String> = []

    /// Pending file syncs (debounced)
    private var pendingSyncs: [String: Task<Void, Never>] = [:]

    /// Sessions whose file sync is running. A sync requested meanwhile runs
    /// after it, so transcript chunks are applied in the order they were read.
    private var syncsInFlight: Set<String> = []
    private var syncsRequestedInFlight: Set<String> = []

    /// Sync debounce interval (100ms)
    private let syncDebounceNs: UInt64 = 100_000_000

    /// Periodic status check task
    private var statusCheckTask: Task<Void, Never>?

    /// Status check interval (3 seconds)
    private let statusCheckIntervalSeconds: UInt64 = 3

    /// A session without a pid (status line only) is dropped after this long without news.
    private let pidlessSessionTimeout: TimeInterval = 15 * 60

    /// How long ended session ids are remembered.
    private let endedSessionMemory: TimeInterval = 10 * 60

    private let reviewStore: ReviewStateStore
    private let parser: ConversationParser
    private let publishInterval: TimeInterval
    private let effects: SessionStoreEffects
    private let completionTiming: TurnCompletion.Timing
    private let backgroundWaitTiming: BackgroundWork.WaitTiming

    /// When the previous app run was last known alive (nil on a first run).
    /// Completions after it happened while nobody was watching.
    private let previousRunAliveAt: Date?
    /// When this store (this run of the app) started. Only a session whose
    /// registry went idle before then (give or take `launchGrace`) can have
    /// finished while the app was down.
    private let startedAt: Date
    /// Accounts' registries are read within moments of launch; a turn that
    /// ended in between still counts as unobserved.
    static let launchGrace: TimeInterval = 5

    // MARK: - Published State (for UI)

    /// Publisher for session state changes (nonisolated for Combine subscription from any context)
    private nonisolated(unsafe) let sessionsSubject = CurrentValueSubject<[SessionState], Never>([])
    private var publishScheduled = false
    private var lastPublished: [SessionState] = []
    private(set) var publishCount = 0

    /// Public publisher for UI subscription
    nonisolated var sessionsPublisher: AnyPublisher<[SessionState], Never> {
        sessionsSubject.eraseToAnyPublisher()
    }

    // MARK: - Initialization

    /// - Parameters:
    ///   - publishInterval: publishes are coalesced to one per interval (0: publish after every event).
    ///   - effects: what the store asks of the rest of the app (closing held
    ///     permission sockets, rescanning a session registry).
    ///   - completionTiming: how long a Stop waits to be confirmed as the end of its turn.
    ///   - backgroundWaitTiming: when a turn waiting on background agents
    ///     gives up waiting without being woken.
    init(
        reviewStore: ReviewStateStore = .shared,
        parser: ConversationParser = .shared,
        publishInterval: TimeInterval = 0.05,
        effects: SessionStoreEffects = .live,
        completionTiming: TurnCompletion.Timing = .standard,
        backgroundWaitTiming: BackgroundWork.WaitTiming = .standard,
        startedAt: Date = Date()
    ) {
        self.reviewStore = reviewStore
        self.parser = parser
        self.publishInterval = publishInterval
        self.effects = effects
        self.completionTiming = completionTiming
        self.backgroundWaitTiming = backgroundWaitTiming
        self.previousRunAliveAt = reviewStore.previousRunAliveAt
        self.startedAt = startedAt
    }

    // MARK: - Event Processing

    /// Process any session event - the ONLY way to mutate state
    func process(_ event: SessionEvent) async {
        Self.logger.debug("Processing: \(String(describing: event), privacy: .public)")

        switch event {
        case .hookReceived(let hookEvent):
            processHookEvent(hookEvent)

        case .statusLineReceived(let message):
            processStatusLine(message)

        case .registrySnapshot(let configDir, let entries):
            processRegistrySnapshot(configDir: configDir, entries: entries)

        case .markReviewed(let sessionId, let at):
            markReviewed(sessionId: sessionId, at: at)

        case .markViewed(let sessionId, let completedAt):
            markViewed(sessionId: sessionId, completedAt: completedAt)

        case .markAllReviewed(let at):
            markAllReviewed(at: at)

        case .dismissFailure(let sessionId, let at):
            dismissFailure(sessionId: sessionId, at: at)

        case .dropAccountSessions(let accountId):
            for (sessionId, session) in sessions where session.accountId == accountId {
                removeSession(sessionId)
            }

        case .hooksTurnedOff:
            for sessionId in Array(sessions.keys) {
                sessions[sessionId]?.lastHookEventAt = nil
            }

        case .permissionApproved(let sessionId, let toolUseId):
            processPermissionResolved(sessionId: sessionId, toolUseId: toolUseId, toolStatus: .running)

        case .permissionDenied(let sessionId, let toolUseId, _):
            processPermissionResolved(sessionId: sessionId, toolUseId: toolUseId, toolStatus: .error)

        case .permissionSocketFailed(let sessionId, let toolUseId):
            processSocketFailure(sessionId: sessionId, toolUseId: toolUseId)

        case .fileUpdated(let payload):
            processFileUpdate(payload)

        case .interruptDetected(let sessionId, let at):
            processInterrupt(sessionId: sessionId, at: at)

        case .completionCheck(let sessionId, let stopAt):
            withSession(sessionId) { session in
                guard session.completionPendingSince == stopAt else { return }
                settlePendingCompletion(&session, now: Date())
                if session.completionPendingSince != nil {
                    scheduleCompletionCheck(for: session)
                }
            }

        case .backgroundWaitCheck(let sessionId, let since):
            withSession(sessionId) { session in
                guard session.backgroundWaitSince == since else { return }
                settleBackgroundWait(&session, now: Date())
            }

        case .loadHistory(let sessionId, let cwd):
            await loadHistoryFromFile(sessionId: sessionId, cwd: cwd)

        case .releaseHistory(let sessionId):
            openHistories.remove(sessionId)
            withSession(sessionId) { trimChat(&$0) }
        }

        publishState()
    }

    /// Mutates a session in place. The value is taken out of the dictionary
    /// first, so changing its chat items doesn't copy them (copy-on-write).
    private func withSession(_ sessionId: String, _ body: (inout SessionState) -> Void) {
        guard var session = sessions.removeValue(forKey: sessionId) else { return }
        let before = ReviewSnapshot(session)
        body(&session)
        sessions[sessionId] = session
        persistReviewIfChanged(session, before: before)
    }

    // MARK: - Hook Event Processing

    private func processHookEvent(_ event: HookEvent) {
        let sessionId = event.sessionId
        let now = event.receivedAt

        if event.event == "SessionEnd" {
            removeSession(sessionId)
            return
        }

        let isNew = sessions[sessionId] == nil
        var session = sessions.removeValue(forKey: sessionId) ?? createSession(sessionId: sessionId, cwd: event.cwd, now: now)
        if isNew {
            recentlyEnded.removeValue(forKey: sessionId)
            // A session first seen mid-flight: its progress so far is only in the transcript.
            let startsFresh = event.event == "SessionStart" && (event.source == "startup" || event.source == "clear")
            if !startsFresh {
                needsTaskReconstruction.insert(sessionId)
            }
        }
        let reviewBefore = ReviewSnapshot(session)

        applyMetadata(from: event, to: &session)
        session.lastActivity = now
        session.lastEventAt = now
        session.lastHookEventAt = now

        if !event.isSubagentEvent, event.resumesTurn || event.event == "StopFailure" {
            // Claude went on (a Stop hook continued the turn, a wake-up, a
            // new prompt): the Stop before this wasn't the end of the turn.
            session.completionPendingSince = nil
        }

        let previousPhase = session.phase
        if let target = event.determinePhase() {
            applyPhase(target, for: event, to: &session, isNew: isNew)
        }

        applyLifecycle(event: event, previousPhase: isNew ? nil : previousPhase, session: &session, now: now)
        applyNeedsInput(event: event, session: &session)

        if event.event == "PermissionRequest", let toolUseId = event.toolUseId {
            updateToolStatus(in: &session, toolId: toolUseId, status: .waitingForApproval)
            session.toolTracker.setPhase(.pendingApproval, id: toolUseId)
        }

        session.tasks.apply(event)
        processToolTracking(event: event, session: &session)
        processSubagentTracking(event: event, session: &session)

        sessions[sessionId] = session
        persistReviewIfChanged(session, before: reviewBefore)

        if event.shouldSyncFile || isNew {
            scheduleFileSync(sessionId: sessionId)
        }
    }

    /// Identity, account and descriptive fields carried by every event.
    private func applyMetadata(from event: HookEvent, to session: inout SessionState) {
        if let pid = event.pid, pid > 0, session.pid != pid {
            adopt(pid: pid, into: &session)
        }
        if let tty = event.tty {
            session.tty = tty.replacingOccurrences(of: "/dev/", with: "")
        }
        if !event.cwd.isEmpty {
            session.currentCwd = event.cwd
        }
        if let transcriptPath = event.transcriptPath, !transcriptPath.isEmpty,
           !event.isSubagentEvent || session.transcriptPath == nil {
            session.transcriptPath = transcriptPath
        }
        if event.transcriptPath != nil || event.configDirEnv != nil || session.accountId == nil {
            session.accountId = event.resolvedConfigDir
        }
        if let configDirEnv = event.configDirEnv, !configDirEnv.isEmpty {
            session.configDirEnv = configDirEnv
        }
        if let entrypoint = event.entrypoint, !entrypoint.isEmpty {
            session.entrypoint = entrypoint
        }
        if !event.isSubagentEvent, let mode = event.permissionMode, !mode.isEmpty {
            session.permissionMode = mode
        }
        if let model = event.model, !model.isEmpty {
            session.model = model
        }
        session.applyTitle(event.sessionTitle, source: .hook)
    }

    /// Records a (new) Claude process: its start time (to tell a reused pid
    /// apart), its terminal and whether it runs in tmux, all from the kernel
    /// in microseconds (no `ps`).
    private func adopt(pid: Int, into session: inout SessionState) {
        session.pid = pid
        guard let info = ProcessInspector.info(pid: pid) else {
            session.pidStartedAt = nil
            return
        }
        session.pidStartedAt = info.startedAt
        if let tty = info.tty {
            session.tty = tty
        }
        session.isInTmux = ProcessInspector.isInTmux(pid: pid)
    }

    /// Moves the phase, protecting finished sessions and pending approvals from
    /// events that don't concern them.
    private func applyPhase(_ target: SessionPhase, for event: HookEvent, to session: inout SessionState, isNew: Bool) {
        if case .waitingForApproval(let context) = target {
            enqueueApproval(context, session: &session, isNew: isNew)
            return
        }
        if event.isSubagentEvent, !isNew {
            switch target {
            case .waitingForInput, .idle, .ended:
                // A subagent (or teammate) finishing its own work doesn't end
                // the main turn, nor the main session's pending requests.
                return
            default:
                break
            }
        }

        switch (session.phase, target) {
        case (.waitingForInput, .processing), (.idle, .processing),
             (.waitingForInput, .compacting), (.idle, .compacting):
            // A turn only resumes on a user prompt, a fresh main-session tool
            // call or a manual compaction. Late PostToolUse / SubagentStop /
            // Task* events, background subagents and their automatic
            // compactions land after Stop and would otherwise leave the
            // session "processing" forever.
            if !isNew && !event.resumesTurn {
                Self.logger.debug("Ignoring late \(event.event, privacy: .public) on a finished session")
                return
            }
        case (.waitingForApproval, .processing), (.waitingForApproval, .compacting):
            // Parallel tool calls: another tool finishing doesn't answer this
            // request. The request's own PostToolUse / PermissionDenied is
            // handled by resolveApproval (which promotes the next queued one).
            guard event.event == "UserPromptSubmit", !event.isSubagentEvent else { return }
            // A new prompt: the main session's requests are over (the user
            // typed past them); background agents' still wait.
            dropMainSessionApprovals(from: &session, then: target)
            return
        case (.waitingForApproval, .waitingForInput) where event.event == "Notification":
            // An idle notification says nothing about a request whose hook is
            // still waiting; its own events (or the hook going away) resolve it.
            return
        case (.waitingForApproval, _):
            // The main turn ended (Stop, StopFailure, SessionStart): its own
            // requests are over; background agents' requests outlive it.
            dropMainSessionApprovals(from: &session, then: target)
            return
        default:
            break
        }

        if session.phase.canTransition(to: target) {
            session.phase = target
        } else {
            let from = session.phase.description
            Self.logger.debug("Invalid transition: \(from, privacy: .public) -> \(target.description, privacy: .public), ignoring")
        }
    }

    /// Turn boundaries: completion, review, errors, background work.
    /// `previousPhase` is nil for a session first seen with this event.
    private func applyLifecycle(event: HookEvent, previousPhase: SessionPhase?, session: inout SessionState, now: Date) {
        guard !event.isSubagentEvent else { return }

        switch event.event {
        case "UserPromptSubmit":
            session.turnStartedAt = now
            session.toolTracker.endMainTurn()
            clearFailure(&session)
            // What was already out when this turn began (BHV-1).
            session.agentsAtTurnStart = session.knownWakingAgents
            session.wakeupsAtTurnStart = session.knownWakeups
            session.backgroundTaskCount = 0
            session.backgroundAgentCount = 0
            // A background wait stands until this turn's Stop says what is
            // still running: Esc or a failed turn leaves the agents running.
            session.scheduledWakeupCount = 0
            session.lastPromptSource = event.source
            session.lastPromptWasUserAuthored = event.isUserAuthoredPrompt
            session.completionCheckSince = nil
            // Loop, cron, system and task-notification turns don't mean the
            // user looked at the result; a prompt they typed does (including
            // one sent from VS Code, whose source is "sdk").
            if event.isUserAuthoredPrompt {
                session.reviewedAt = now
            }

        case "Stop":
            // A Stop ends a turn Claude worked on. A session first seen with
            // its Stop (e.g. the app started mid-turn) finished work too, and
            // so did a Stop ending a continuation a blocking Stop hook forced
            // (`stop_hook_active`), even with no event in between.
            // So did the turn an agent's result woke Claude for, even when no
            // prompt or tool of it reached us (the SDK can wake Claude
            // without a UserPromptSubmit).
            let wasWorking: Bool
            switch previousPhase {
            case nil, .processing?, .compacting?, .waitingForApproval?:
                wasWorking = true
            default:
                wasWorking = event.stopHookActive == true || session.backgroundWaitSince != nil
            }
            // The hook's last_assistant_message is exact. The transcript's last
            // message is only a fallback for hooks that don't send it: it can
            // be a sync behind, still showing the preamble of an automatic
            // compaction earlier in this very turn.
            let finalMessage = event.lastAssistantMessage.flatMap { $0.isEmpty ? nil : $0 }
                ?? session.conversationInfo.lastMessage
            let isContextResume = finalMessage?.hasPrefix(Self.contextResumePrefix) == true
            if let message = event.lastAssistantMessage, !message.isEmpty {
                session.lastAssistantMessage = message
            }
            let types = event.backgroundTaskTypes ?? []
            let awaited = types.filter { Self.wakingBackgroundTaskTypes.contains($0) }
            session.backgroundTaskCount = max(event.backgroundTaskCount ?? types.count, 0)
            session.backgroundAgentCount = awaited.count
            session.backgroundAgentTypes = awaited
            // Agents and workflows still running will wake Claude when they
            // finish: until then the work isn't done (the session shows as
            // working, not ready for review).
            session.backgroundWaitSince = awaited.isEmpty ? nil : now
            session.scheduledWakeupCount = max(event.sessionCronCount ?? 0, 0)
            session.knownWakingAgents = session.backgroundAgentCount
            session.knownWakeups = session.scheduledWakeupCount
            clearFailure(&session)
            session.subagentState = SubagentState()
            session.toolTracker.endMainTurn()
            if wasWorking && !isContextResume {
                // Claude Code still runs the other Stop hooks; a blocking one
                // (/goal) continues the turn. The registry going idle, or a
                // quiet moment, confirms the turn is over.
                session.completionPendingSince = now
                settlePendingCompletion(&session, now: now)
                if session.completionPendingSince != nil {
                    scheduleCompletionCheck(for: session)
                }
            }
            if let configDir = session.accountId {
                effects.rescanRegistry(configDir)
            }
            settleBackgroundWait(&session, now: now)

        case "StopFailure":
            let message = NeedsInputReason.humanizedStopError(event.stopError)
            session.stopError = message
            session.stopErrorCode = event.stopError
            session.stopErrorAt = now
            // last_assistant_message of a StopFailure is the API error text;
            // the preview keeps the last real reply.
            session.backgroundTaskCount = 0
            session.backgroundAgentCount = 0
            // The turn failed; the agents it waited on didn't (the wait
            // stands, and the registry can end it).
            session.subagentState = SubagentState()
            session.toolTracker.endMainTurn()
            settleBackgroundWait(&session, now: now)

        case "SessionStart":
            if event.source == "clear" {
                session.reviewedAt = now
            }

        case "Notification" where event.notificationType == "idle_prompt":
            // Claude has sat idle for a minute: whatever the Stop hooks did,
            // the turn is over.
            if session.completionPendingSince != nil {
                confirmPendingCompletion(&session)
            }

        default:
            break
        }
    }

    private func clearFailure(_ session: inout SessionState) {
        session.stopError = nil
        session.stopErrorCode = nil
        session.stopErrorAt = nil
    }

    /// Sets or clears `needsInputReason` for this event.
    private func applyNeedsInput(event: HookEvent, session: inout SessionState) {
        switch event.event {
        case "StopFailure" where !event.isSubagentEvent:
            session.setNeedsInput(.error(NeedsInputReason.humanizedStopError(event.stopError)), at: event.receivedAt)

        case "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "PermissionRequest":
            // Main-session activity means whatever was asked has been answered.
            // Background subagents keep working after a failed turn, so their
            // events only settle a terminal permission prompt (theirs), never
            // an error or a dialog of the main session.
            if !event.isSubagentEvent {
                session.needsInputReason = nil
            } else if case .permission = session.needsInputReason {
                session.needsInputReason = nil
            }

        case "Notification":
            // Agent view announces background sessions ("<label> needs your
            // input", "<label> finished") on the session hosting it; they are
            // about another session.
            guard !event.isAgentViewAnnouncement else { return }
            switch event.notificationType {
            case "elicitation_dialog", "elicitation_url_dialog":
                session.setNeedsInput(.elicitation(event.message), at: event.receivedAt)
            case "agent_needs_input", "worker_permission_prompt":
                session.setNeedsInput(.dialog(event.message ?? event.title), at: event.receivedAt)
            case "permission_prompt":
                // The terminal is asking. With a pending PermissionRequest the
                // approval already shows; otherwise (no hook socket) flag it.
                if !session.phase.isWaitingForApproval {
                    let tool = Self.toolName(fromPermissionPrompt: event.message) ?? event.title ?? ""
                    session.setNeedsInput(.permission(tool: tool), at: event.receivedAt)
                }
            case "elicitation_complete", "elicitation_response":
                if case .elicitation = session.needsInputReason {
                    session.needsInputReason = nil
                }
            default:
                break
            }

        default:
            break
        }
    }

    /// "Claude needs your permission to use Bash" → "Bash".
    static func toolName(fromPermissionPrompt message: String?) -> String? {
        guard let message, let range = message.range(of: "permission to use ") else { return nil }
        let rest = message[range.upperBound...]
        let name = rest.prefix { !$0.isWhitespace && $0 != "," && $0 != "." }
        return name.isEmpty ? nil : String(name)
    }

    private func createSession(sessionId: String, cwd: String, now: Date) -> SessionState {
        var session = SessionState(
            sessionId: sessionId,
            cwd: cwd,
            projectName: URL(fileURLWithPath: cwd).lastPathComponent,
            isInTmux: false,
            phase: .idle,
            lastActivity: now,
            createdAt: now
        )
        if let record = reviewStore.record(for: sessionId) {
            session.completedAt = record.completedAt
            session.reviewedAt = record.reviewedAt
            session.lastAssistantMessage = record.lastAssistantMessage
            // Still waiting on background agents when we last looked; the
            // registry says whether they still run.
            session.backgroundWaitSince = record.backgroundWaitSince
            session.backgroundAgentTypes = record.backgroundAgentTypes ?? []
            session.backgroundAgentCount = session.backgroundAgentTypes.count
            if let error = record.stopError {
                // The turn failed while we last looked; the first transcript
                // sync clears it if the user has moved on since.
                session.stopError = error
                session.stopErrorCode = record.stopErrorCode
                session.stopErrorAt = record.failedAt
                session.setNeedsInput(.error(error), at: record.failedAt ?? now)
            }
        }
        Self.logger.info("Tracking session \(sessionId.prefix(8), privacy: .public)")
        return session
    }

    // MARK: - Turn completion

    /// Confirms a pending Stop as the end of the turn once the session
    /// registry agrees; see `TurnCompletion`.
    private func settlePendingCompletion(_ session: inout SessionState, now: Date) {
        guard let stopAt = session.completionPendingSince else { return }
        let decision = TurnCompletion.decide(
            stopAt: stopAt,
            turnStartedAt: session.turnStartedAt,
            registryStatus: session.registryStatus,
            registryChangedAt: session.registryStatusChangedAt,
            now: now,
            timing: completionTiming
        )
        if decision == .confirm {
            confirmPendingCompletion(&session)
        }
    }

    private func confirmPendingCompletion(_ session: inout SessionState) {
        guard let stopAt = session.completionPendingSince else { return }
        session.completionPendingSince = nil
        session.completedAt = stopAt
        let sessionId = session.sessionId
        Self.logger.debug("Turn of \(sessionId.prefix(8), privacy: .public) completed")
    }

    /// Re-checks a pending Stop when the fallback delay has passed.
    private func scheduleCompletionCheck(for session: SessionState) {
        guard let stopAt = session.completionPendingSince else { return }
        let delay = TurnCompletion.checkDelay(
            stopAt: stopAt,
            turnStartedAt: session.turnStartedAt,
            registryStatus: session.registryStatus,
            registryChangedAt: session.registryStatusChangedAt,
            now: Date(),
            timing: completionTiming
        )
        let sessionId = session.sessionId
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            await self?.process(.completionCheck(sessionId: sessionId, stopAt: stopAt))
        }
    }

    // MARK: - Background wait

    /// Ends the wait of a turn that left background agents running once
    /// none is left, when Claude wasn't woken to say so (see
    /// `BackgroundWork.decide`); otherwise checks again when the answer can
    /// change by the clock alone.
    private func settleBackgroundWait(_ session: inout SessionState, now: Date) {
        guard let since = session.backgroundWaitSince else { return }
        let decision = BackgroundWork.decide(
            waitSince: since,
            registryStatus: session.registryStatus,
            registryChangedAt: session.registryStatusChangedAt,
            lastHookEventAt: session.lastHookEventAt,
            now: now,
            timing: backgroundWaitTiming
        )
        switch decision {
        case .end(let endedAt):
            endBackgroundWait(&session, at: endedAt)
        case .keep(let recheckIn?):
            let sessionId = session.sessionId
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(recheckIn, 0.05)))
                await self?.process(.backgroundWaitCheck(sessionId: sessionId, since: since))
            }
        case .keep(nil):
            break
        }
    }

    /// The agents are gone without waking Claude (stopped, or their
    /// notification never came): the turn's work was done at `at`.
    private func endBackgroundWait(_ session: inout SessionState, at: Date) {
        session.backgroundWaitSince = nil
        session.backgroundTaskCount = max(session.backgroundTaskCount - session.backgroundAgentCount, 0)
        session.backgroundAgentCount = 0
        session.backgroundAgentTypes = []
        session.knownWakingAgents = 0
        // The work is done now, whatever became of the turn that waited:
        // its Stop may never have been confirmed (the agents kept the
        // registry busy), or a later turn was interrupted.
        session.completionPendingSince = nil
        session.completedAt = max(session.completedAt ?? at, at)
        let sessionId = session.sessionId
        Self.logger.debug("Background wait of \(sessionId.prefix(8), privacy: .public) ended without a wake-up")
    }

    // MARK: - Approvals

    /// Makes `context` the active approval, or queues it behind the active one.
    private func enqueueApproval(_ context: PermissionContext, session: inout SessionState, isNew: Bool) {
        var context = context
        if case .waitingForApproval(let active) = session.phase {
            guard active.toolUseId != context.toolUseId else {
                context.activatedAt = active.activatedAt
                session.phase = .waitingForApproval(context)
                return
            }
            if !session.queuedApprovals.contains(where: { $0.toolUseId == context.toolUseId }) {
                session.queuedApprovals.append(context)
            }
            return
        }
        let target = SessionPhase.waitingForApproval(context)
        if session.phase.canTransition(to: target) {
            // A main-session request always follows its PreToolUse, which put
            // the session in processing. A finished session is being asked by
            // a background agent: answering must not restart the turn.
            switch session.phase {
            case .waitingForInput, .idle:
                session.phaseAfterApprovals = isNew ? .processing : session.phase
            default:
                session.phaseAfterApprovals = .processing
            }
            context.activatedAt = context.receivedAt
            session.phase = .waitingForApproval(context)
        }
    }

    /// Removes an approval that was answered; the next queued one becomes active,
    /// else the session returns to the phase it had before (see `phaseAfterApprovals`).
    private func resolveApproval(toolUseId: String, session: inout SessionState, at now: Date = Date()) {
        session.queuedApprovals.removeAll { $0.toolUseId == toolUseId }
        guard case .waitingForApproval(let active) = session.phase, active.toolUseId == toolUseId else { return }
        activateNextApproval(in: &session, orReturnTo: session.phaseAfterApprovals, at: now)
    }

    /// Shows the next queued request (clicks right after the swap are for
    /// the old one: the UI ignores them for a moment after `activatedAt`),
    /// else moves to `phase`.
    private func activateNextApproval(in session: inout SessionState, orReturnTo phase: SessionPhase, at now: Date) {
        if !session.queuedApprovals.isEmpty {
            var next = session.queuedApprovals.removeFirst()
            next.activatedAt = now
            session.phase = .waitingForApproval(next)
        } else if session.phase.canTransition(to: phase) {
            session.phase = phase
        }
    }

    /// The main turn is over (Stop, StopFailure, a new prompt, an interrupt,
    /// the registry going idle): the main session's requests go, their held
    /// sockets are closed (so a hook never waits on a request nobody sees),
    /// and background agents' requests stay. With none left the session
    /// moves to `target`; else the agents' requests stay shown and the
    /// session goes to `target` once they are answered.
    private func dropMainSessionApprovals(from session: inout SessionState, then target: SessionPhase) {
        let pending = session.pendingPermissions
        let dropped = pending.filter { !$0.isFromSubagent }.map(\.toolUseId)
        let kept = pending.filter(\.isFromSubagent)
        effects.cancelPermissions(dropped)
        for toolUseId in dropped {
            updateToolStatus(in: &session, toolId: toolUseId, status: .interrupted, onlyIfPending: true)
        }

        guard var first = kept.first else {
            session.queuedApprovals.removeAll()
            if session.phase.canTransition(to: target) || session.phase.isWaitingForApproval {
                session.phase = target
            }
            return
        }
        let stillActive = session.activePermission?.toolUseId == first.toolUseId
        if !stillActive {
            first.activatedAt = Date()
        }
        session.queuedApprovals = Array(kept.dropFirst())
        session.phase = .waitingForApproval(first)
        switch target {
        case .processing, .compacting:
            session.phaseAfterApprovals = target
        default:
            session.phaseAfterApprovals = target == .idle ? .idle : .waitingForInput
        }
    }

    private func processPermissionResolved(sessionId: String, toolUseId: String, toolStatus: ToolStatus) {
        withSession(sessionId) { session in
            // A late outcome must not flip a tool that already finished (its
            // PostToolUse may have been processed first) back to running.
            updateToolStatus(in: &session, toolId: toolUseId, status: toolStatus, onlyIfPending: true)
            if toolStatus == .running {
                session.toolTracker.setPhase(.running, id: toolUseId)
            } else {
                session.toolTracker.completeTool(id: toolUseId, success: false)
            }
            // Approved: the tool runs. Denied: Claude continues with the denial.
            resolveApproval(toolUseId: toolUseId, session: &session)
            // Settles any prompt; a failed turn's error stays (the request can be
            // a background agent's, after StopFailure).
            if session.needsInputReason?.isError != true {
                session.needsInputReason = nil
            }
            session.lastEventAt = Date()
        }
    }

    /// The hook holding the request went away (the terminal answered first,
    /// or it timed out): the app can no longer answer it. Claude either
    /// continues or is still asking in the terminal; the registry and the
    /// permission_prompt notification tell which.
    private func processSocketFailure(sessionId: String, toolUseId: String) {
        withSession(sessionId) { session in
            resolveApproval(toolUseId: toolUseId, session: &session)
        }
    }

    // MARK: - Tool Tracking

    private func processToolTracking(event: HookEvent, session: inout SessionState) {
        switch event.event {
        case "PreToolUse":
            guard let toolUseId = event.toolUseId, let toolName = event.tool else { return }
            session.toolTracker.startTool(
                id: toolUseId,
                name: toolName,
                agentId: event.isSubagentEvent ? event.agentId : nil,
                at: event.receivedAt
            )
            // Skip creating top-level placeholder for subagent tools
            // They'll appear under their parent Task instead
            let isSubagentTool = event.isSubagentEvent
                || (session.subagentState.hasActiveSubagent && !ToolCallItem.isSubagentContainerName(toolName))
            guard !isSubagentTool, !session.chatItems.contains(where: { $0.id == toolUseId }) else { return }
            session.chatItems.append(ChatHistoryItem(
                id: toolUseId,
                type: .toolCall(ToolCallItem(
                    name: toolName,
                    input: event.flatToolInput,
                    status: .running,
                    result: nil,
                    structuredResult: nil,
                    subagentTools: []
                )),
                timestamp: event.receivedAt
            ))
            trimChatIfClosed(&session)

        case "PostToolUse", "PostToolUseFailure", "PermissionDenied":
            guard let toolUseId = event.toolUseId else { return }
            let status: ToolStatus
            switch event.event {
            case "PostToolUse": status = .success
            case "PostToolUseFailure": status = event.isInterrupt == true ? .interrupted : .error
            default: status = .error
            }
            // The tool completed (possibly approved in the terminal).
            session.toolTracker.completeTool(id: toolUseId, success: status == .success)
            updateToolStatus(in: &session, toolId: toolUseId, status: status, onlyIfPending: true)
            // A request for this call was answered in the terminal (or denied by a rule).
            resolveApproval(toolUseId: toolUseId, session: &session, at: event.receivedAt)

        default:
            break
        }
    }

    private func processSubagentTracking(event: HookEvent, session: inout SessionState) {
        switch event.event {
        case "PreToolUse":
            if ToolCallItem.isSubagentContainerName(event.tool), let toolUseId = event.toolUseId, !event.isSubagentEvent {
                let description = event.toolInput?["description"]?.value as? String
                session.subagentState.startTask(taskToolId: toolUseId, description: description)
            } else if let toolName = event.tool,
                      let toolUseId = event.toolUseId,
                      session.subagentState.hasActiveSubagent {
                // A subagent's inner tool is starting: show it under its
                // Task/Agent live (not only after the Agent completes).
                let subagentTool = SubagentToolCall(
                    id: toolUseId,
                    name: toolName,
                    input: event.flatToolInput,
                    status: .running,
                    timestamp: event.receivedAt
                )
                session.subagentState.addSubagentTool(subagentTool)
                syncSubagentToolsToChatItems(session: &session)
            }

        case "PostToolUse":
            if ToolCallItem.isSubagentContainerName(event.tool), let toolUseId = event.toolUseId, !event.isSubagentEvent {
                // Agent tool returned — the subagent has finished. Stop
                // tracking so subsequent tools in the parent turn don't get
                // attached to this dead task.
                session.subagentState.stopTask(taskToolId: toolUseId)
            } else if let toolUseId = event.toolUseId,
                      session.subagentState.hasActiveSubagent {
                session.subagentState.updateSubagentToolStatus(toolId: toolUseId, status: .success)
                syncSubagentToolsToChatItems(session: &session)
            }

        default:
            break
        }
    }

    /// Push the current subagent tool lists from subagentState into the
    /// corresponding ChatHistoryItem.subagentTools so the UI renders them live.
    private func syncSubagentToolsToChatItems(session: inout SessionState) {
        for (taskToolId, context) in session.subagentState.activeTasks where !context.subagentTools.isEmpty {
            guard let index = session.chatItems.lastIndex(where: { $0.id == taskToolId }),
                  case .toolCall(var tool) = session.chatItems[index].type,
                  tool.subagentTools != context.subagentTools else { continue }
            tool.subagentTools = context.subagentTools
            session.chatItems[index] = ChatHistoryItem(
                id: taskToolId,
                type: .toolCall(tool),
                timestamp: session.chatItems[index].timestamp
            )
        }
    }

    // MARK: - Status Line

    private func processStatusLine(_ message: StatusLineMessage) {
        let sessionId = message.sessionId
        guard recentlyEnded[sessionId] == nil else { return }
        let now = message.update.receivedAt
        let update = message.update

        let isNew = sessions[sessionId] == nil
        // The status line runs only in live terminal sessions, so an unknown
        // session is real: track it (idle until a hook says otherwise).
        var session = sessions.removeValue(forKey: sessionId) ?? createSession(sessionId: sessionId, cwd: message.cwd ?? "", now: now)
        if isNew {
            needsTaskReconstruction.insert(sessionId)
        }

        if let cwd = message.cwd, !cwd.isEmpty {
            session.currentCwd = cwd
        }
        if let transcriptPath = message.transcriptPath, !transcriptPath.isEmpty {
            session.transcriptPath = transcriptPath
        }
        if message.transcriptPath != nil || message.configDirEnv != nil || session.accountId == nil {
            session.accountId = SessionFilter.configDir(transcriptPath: message.transcriptPath, configDirEnv: message.configDirEnv)
        }
        if let configDirEnv = message.configDirEnv, !configDirEnv.isEmpty {
            session.configDirEnv = configDirEnv
        }
        if let percent = update.contextUsedPercent {
            session.contextUsedPercent = min(max(percent, 0), 100)
            session.statusLineUpdatedAt = now
        }
        if let size = update.contextWindowSize, size > 0 {
            session.contextWindowSize = size
        }
        if let model = update.modelDisplayName ?? update.modelId {
            session.model = model
        }
        if let cost = update.costUSD {
            session.costUSD = cost
        }
        session.applyName(update.sessionName)
        // Not `lastHookEventAt`: a status line says nothing about the turn,
        // so it must not hide a later registry correction.
        session.lastEventAt = max(session.lastEventAt, now)

        sessions[sessionId] = session
        if isNew {
            scheduleFileSync(sessionId: sessionId)
        }
    }

    // MARK: - Session Registry

    private func processRegistrySnapshot(configDir: String, entries: [SessionRegistryEntry]) {
        let account = AccountPaths.normalize(configDir)
        // One pid, one registry file, one current session: a session of
        // this account whose pid now names another session was left by
        // /clear or /resume without a SessionEnd reaching us.
        let sessionByPid = Dictionary(entries.map { ($0.pid, $0) }, uniquingKeysWith: { first, _ in first })
        for (sessionId, session) in sessions where session.accountId == account {
            guard let pid = session.pid, let entry = sessionByPid[pid], entry.sessionId != sessionId else { continue }
            let entryTime = entry.statusChangedAt ?? .distantFuture
            if let hookAt = session.lastHookEventAt, hookAt > entryTime {
                continue  // hooks spoke after the registry: don't fight a race
            }
            Self.logger.info("Session \(sessionId.prefix(8), privacy: .public) was replaced by \(entry.sessionId.prefix(8), privacy: .public) in pid \(pid)")
            removeSession(sessionId)
        }

        for entry in entries {
            guard recentlyEnded[entry.sessionId] == nil else { continue }

            guard var session = sessions.removeValue(forKey: entry.sessionId) else {
                let created = createSession(fromRegistry: entry, configDir: configDir)
                sessions[entry.sessionId] = created
                needsTaskReconstruction.insert(entry.sessionId)
                scheduleFileSync(sessionId: entry.sessionId)
                continue
            }
            let reviewBefore = ReviewSnapshot(session)

            if session.pid != entry.pid {
                adopt(pid: entry.pid, into: &session)
            }
            if session.entrypoint == nil { session.entrypoint = entry.entrypoint }
            if session.accountId == nil { session.accountId = account }
            session.applyName(entry.name, isDerived: entry.isNameDerived)

            if let changedAt = entry.statusChangedAt, changedAt != session.registryStatusChangedAt || entry.status != session.registryStatus {
                let previousStatus = session.registryStatus
                session.registryStatus = entry.status
                session.registryStatusChangedAt = changedAt
                // No hook can settle a request matched to no call, so this
                // one doesn't wait for the hooks to go quiet.
                settleAnsweredSyntheticRequest(&session, registryStatus: entry.status, changedAt: changedAt)
                // Hooks are the primary source; the registry only corrects
                // state no hook reported (interrupts, dialogs, sessions
                // without hooks). Status line updates don't count as hooks.
                if changedAt > (session.lastHookEventAt ?? .distantPast) {
                    reconcile(&session, with: entry, previousStatus: previousStatus, at: changedAt)
                    session.lastEventAt = max(session.lastEventAt, changedAt)
                }
                settlePendingCompletion(&session, now: Date())
                settleBackgroundWait(&session, now: Date())
            }
            sessions[entry.sessionId] = session
            persistReviewIfChanged(session, before: reviewBefore)
        }
    }

    private func reconcile(_ session: inout SessionState, with entry: SessionRegistryEntry, previousStatus: String?, at changedAt: Date) {
        switch entry.status {
        case "idle", "shell":
            switch session.phase {
            case .processing, .compacting:
                if session.isHookBacked {
                    // Idle without a Stop: the turn was interrupted.
                    session.phase = .idle
                    session.needsInputReason = nil
                } else {
                    // No hooks report this session: the registry going idle is
                    // how its turns end. The transcript tells a finished turn
                    // from an interrupted one.
                    session.phase = .waitingForInput
                    session.completionCheckSince = session.turnStartedAt ?? changedAt
                    scheduleFileSync(sessionId: session.sessionId)
                }
            case .waitingForApproval:
                // The dialog is gone and Claude is idle.
                dropMainSessionApprovals(from: &session, then: .idle)
            default:
                if case .dialog = session.needsInputReason {
                    session.needsInputReason = nil
                }
            }
        case "waiting":
            switch session.phase {
            case .processing, .compacting:
                session.setNeedsInput(.dialog(entry.waitingFor), at: changedAt)
            default:
                break
            }
        case "busy":
            switch session.phase {
            case .idle, .waitingForInput:
                // Busy with the agents a background wait is on isn't a turn.
                if session.completionPendingSince == nil && session.backgroundWaitSince == nil {
                    session.phase = .processing
                    if !session.isHookBacked {
                        session.turnStartedAt = changedAt
                    }
                }
            default:
                break
            }
            if case .dialog = session.needsInputReason {
                session.needsInputReason = nil
            }
        default:
            break
        }
    }

    /// A main-session request matched to no call (another hook rewrote its
    /// input while several calls of that tool ran) that the registry says
    /// was answered in the terminal: Claude Code's dialog closed and it went
    /// on after the request arrived. Its hook may live on, so it is closed
    /// too. Background agents' requests show no dialog while our hook
    /// waits, so the registry says nothing about them.
    private func settleAnsweredSyntheticRequest(_ session: inout SessionState, registryStatus: String?, changedAt: Date) {
        guard registryStatus == "busy",
              case .waitingForApproval(let active) = session.phase,
              active.hasSyntheticToolUseId, !active.isFromSubagent,
              changedAt > active.receivedAt else { return }
        effects.cancelPermissions([active.toolUseId])
        resolveApproval(toolUseId: active.toolUseId, session: &session)
    }

    private func createSession(fromRegistry entry: SessionRegistryEntry, configDir: String) -> SessionState {
        let changedAt = entry.statusChangedAt ?? Date()
        var session = createSession(sessionId: entry.sessionId, cwd: entry.cwd ?? "", now: changedAt)
        adopt(pid: entry.pid, into: &session)
        session.accountId = AccountPaths.normalize(configDir)
        session.entrypoint = entry.entrypoint
        session.applyName(entry.name, isDerived: entry.isNameDerived)
        session.lastEventAt = changedAt
        session.registryStatus = entry.status
        session.registryStatusChangedAt = entry.statusChangedAt
        switch entry.status {
        case "busy" where session.backgroundWaitSince != nil:
            // Busy with the agents it was waiting on; a turn of its own
            // would be reported by its hooks.
            session.phase = .waitingForInput
        case "busy":
            session.phase = .processing
            session.turnStartedAt = changedAt
        case "waiting":
            session.phase = .waitingForInput
            session.setNeedsInput(.dialog(entry.waitingFor), at: changedAt)
        default:
            session.phase = .idle
            // A turn may have finished while the app wasn't running; the
            // first sync checks the transcript (see `inferCompletion`). Not
            // for a session that went idle while this run was up and simply
            // wasn't followed (an account tracked only now): its replies
            // were never "while the app was down", and flagging them would
            // flood the review queue with work the user has seen.
            if let changedAt = entry.statusChangedAt, changedAt < startedAt.addingTimeInterval(Self.launchGrace) {
                session.completionCheckSince = previousRunAliveAt
            }
        }
        settleBackgroundWait(&session, now: Date())
        return session
    }

    // MARK: - Review

    private func markReviewed(sessionId: String, at date: Date) {
        withSession(sessionId) { session in
            session.reviewedAt = max(date, session.reviewedAt ?? .distantPast)
        }
    }

    private func markViewed(sessionId: String, completedAt: Date) {
        withSession(sessionId) { session in
            guard session.completedAt == completedAt, (session.reviewedAt ?? .distantPast) < completedAt else { return }
            session.reviewedAt = completedAt
        }
    }

    /// A failed turn the user dismissed: no longer failed (nor restored as
    /// failed after a relaunch, since the review record drops it), until the
    /// next StopFailure. A failure newer than the click stays.
    private func dismissFailure(sessionId: String, at date: Date) {
        withSession(sessionId) { session in
            guard session.hasFailedTurn, (session.stopErrorAt ?? .distantPast) <= date else { return }
            clearFailure(&session)
            session.needsInputReason = nil
            session.reviewedAt = max(date, session.reviewedAt ?? .distantPast)
        }
    }

    private func markAllReviewed(at date: Date) {
        for sessionId in Array(sessions.keys) where sessions[sessionId]?.isReadyForReview == true {
            withSession(sessionId) { session in
                session.reviewedAt = max(date, session.reviewedAt ?? .distantPast)
            }
        }
    }

    /// The fields persisted in review-state.json.
    private struct ReviewSnapshot: Equatable {
        let completedAt: Date?
        let reviewedAt: Date?
        let lastAssistantMessage: String?
        let stopError: String?
        let stopErrorCode: String?
        let failedAt: Date?
        let backgroundWaitSince: Date?
        let backgroundAgentTypes: [String]?

        init(_ session: SessionState) {
            completedAt = session.completedAt
            reviewedAt = session.reviewedAt
            lastAssistantMessage = session.lastAssistantMessage
            backgroundWaitSince = session.backgroundWaitSince
            backgroundAgentTypes = session.backgroundWaitSince == nil ? nil : session.backgroundAgentTypes
            stopError = session.hasFailedTurn ? session.stopError : nil
            stopErrorCode = session.hasFailedTurn ? session.stopErrorCode : nil
            failedAt = session.hasFailedTurn ? session.stopErrorAt : nil
        }
    }

    private func persistReviewIfChanged(_ session: SessionState, before: ReviewSnapshot) {
        let after = ReviewSnapshot(session)
        guard after != before else { return }
        // Completions and failures are rare and must survive a crash or
        // SIGTERM right after them; review marks can wait for the debounce.
        let urgent = after.completedAt != before.completedAt || after.stopError != before.stopError
        reviewStore.update(
            sessionId: session.sessionId,
            record: ReviewRecord(
                completedAt: after.completedAt,
                reviewedAt: after.reviewedAt,
                lastAssistantMessage: after.lastAssistantMessage,
                stopError: after.stopError,
                stopErrorCode: after.stopErrorCode,
                failedAt: after.failedAt,
                backgroundWaitSince: after.backgroundWaitSince,
                backgroundAgentTypes: after.backgroundAgentTypes,
                updatedAt: Date()
            ),
            urgent: urgent
        )
    }

    // MARK: - Tool Completion Processing

    /// Applies a tool completion found in the transcript: the authoritative
    /// signal that a tool finished.
    private func applyToolCompletion(at index: Int, result: ToolCompletionResult, session: inout SessionState) {
        guard case .toolCall(var tool) = session.chatItems[index].type,
              tool.status == .running || tool.status == .waitingForApproval else { return }
        let toolUseId = session.chatItems[index].id
        session.toolTracker.completeTool(id: toolUseId, success: result.status == .success)
        tool.status = result.status
        tool.result = result.result
        tool.structuredResult = result.structuredResult ?? tool.structuredResult
        session.chatItems[index] = ChatHistoryItem(
            id: toolUseId,
            type: .toolCall(tool),
            timestamp: session.chatItems[index].timestamp
        )
        // If the completed tool was the one awaiting approval, it was answered elsewhere.
        resolveApproval(toolUseId: toolUseId, session: &session)
    }

    // MARK: - File Update Processing

    private func processFileUpdate(_ payload: FileUpdatePayload) {
        withSession(payload.sessionId) { session in
            applyConversationInfo(payload.conversationInfo, to: &session)
            if session.transcriptPath == nil {
                session.transcriptPath = payload.transcriptPath
            }
            if let reconstructed = payload.reconstructedTasks {
                // History from the transcript, then everything hooks reported since.
                session.tasks = session.tasks.merged(intoReconstructed: reconstructed)
                reviewRestoredState(of: &session, turn: payload.conversationInfo.lastTurn)
            }
            inferCompletion(of: &session, turn: payload.conversationInfo.lastTurn)

            if payload.replacesHistory {
                // /clear (or a rewritten transcript): drop what the transcript
                // no longer has, but keep hook placeholders of the last moments.
                let keptIds = Set(payload.messages.flatMap { Self.itemIds(of: $0) })
                let cutoff = Date().addingTimeInterval(-2)
                session.chatItems.removeAll { !keptIds.contains($0.id) && $0.timestamp <= cutoff }
                session.toolTracker = ToolTracker()
                session.subagentState = SubagentState()
                session.tasks.reset()
            }

            mergeMessages(
                payload.messages,
                into: &session,
                completedTools: payload.completedToolIds,
                toolResults: payload.toolResults,
                structuredResults: payload.structuredResults,
                sortsByTime: payload.replacesHistory
            )
            completeFinishedTools(in: &session, payload: payload)
            applySubagentTools(payload.subagentTools, structuredResults: payload.structuredResults, session: &session)
            trimChatIfClosed(&session)
        }
    }

    /// Tools shown as running whose result the transcript now has.
    private func completeFinishedTools(in session: inout SessionState, payload: FileUpdatePayload) {
        guard !payload.completedToolIds.isEmpty else { return }
        for index in session.chatItems.indices {
            let id = session.chatItems[index].id
            guard payload.completedToolIds.contains(id),
                  case .toolCall(let tool) = session.chatItems[index].type,
                  tool.status == .running || tool.status == .waitingForApproval else { continue }
            let result = ToolCompletionResult.from(
                parserResult: payload.toolResults[id],
                structuredResult: payload.structuredResults[id]
            )
            applyToolCompletion(at: index, result: result, session: &session)
        }
    }

    /// A review state restored from disk predates whatever happened while the
    /// app wasn't watching: a prompt typed after the recorded completion means
    /// the user already saw that result (and moved on), and one typed after a
    /// recorded failure means it was dealt with.
    private func reviewRestoredState(of session: inout SessionState, turn: TranscriptTurn) {
        if let failedAt = session.stopErrorAt, session.hasFailedTurn {
            let movedOn = [turn.humanPromptAt, turn.replyAt].compactMap { $0 }.contains { $0 > failedAt }
            if movedOn {
                clearFailure(&session)
                session.needsInputReason = nil
            }
        }
        guard let completedAt = session.completedAt, let promptAt = turn.humanPromptAt, promptAt > completedAt,
              (session.reviewedAt ?? .distantPast) < promptAt else { return }
        session.reviewedAt = promptAt
    }

    /// A turn that ended where no hook said so: a session without hooks whose
    /// registry went idle, or one that finished while the app wasn't running
    /// (its registry entry idle at discovery, its reply newer than the last
    /// time the app was alive). Only a transcript that ends with Claude's
    /// reply counts: an interrupt, a prompt or a tool call after it doesn't.
    private func inferCompletion(of session: inout SessionState, turn: TranscriptTurn) {
        guard let since = session.completionCheckSince else { return }
        session.completionCheckSince = nil
        // Only Claude Code's own registry saying "idle" tells a finished turn
        // from a reply written mid-turn (text before the next tool call).
        guard session.registryStatus == "idle" || session.registryStatus == "shell",
              session.phase == .idle || session.phase == .waitingForInput,
              session.completionPendingSince == nil else { return }
        let bound = [since, session.completedAt, session.reviewedAt].compactMap { $0 }.max()
        guard let finished = turn.finishedTurn(after: bound) else { return }
        session.completedAt = finished.at
        if let text = finished.text, !text.isEmpty {
            session.lastAssistantMessage = String(text.prefix(ReviewStateStore.maxMessageLength))
        }
        let sessionId = session.sessionId
        Self.logger.info("Turn of \(sessionId.prefix(8), privacy: .public) finished while unobserved")
    }

    /// Transcript-derived fields: conversation info, title fallback, context estimate.
    private func applyConversationInfo(_ info: ConversationInfo, to session: inout SessionState) {
        session.conversationInfo = info
        session.applyTitle(info.title ?? info.summary, source: .transcript)
        if session.model == nil, let model = info.lastModel {
            session.model = model
        }
        // The status line is exact; estimate only for sessions without one.
        if session.statusLineUpdatedAt == nil, let tokens = info.lastContextTokens, tokens > 0 {
            session.contextUsedPercent = ContextUsageEstimator.percent(
                contextTokens: tokens,
                statusLineWindowSize: session.contextWindowSize,
                modelIds: [session.model, info.lastModel]
            )
        }
    }

    /// Chat item ids a message produces (see `createChatItem`).
    private static func itemIds(of message: ChatMessage) -> [String] {
        message.content.enumerated().map { blockIndex, block in
            if case .toolUse(let tool) = block { return tool.id }
            return "\(message.id)-\(block.typePrefix)-\(blockIndex)"
        }
    }

    /// Adds new messages as chat items, updating tool items that already
    /// exist (hook placeholders). Items are found through an id index, so a
    /// long history costs one pass, not one search per tool call.
    private func mergeMessages(
        _ messages: [ChatMessage],
        into session: inout SessionState,
        completedTools: Set<String>,
        toolResults: [String: ConversationParser.ToolResult],
        structuredResults: [String: ToolResultData],
        sortsByTime: Bool
    ) {
        guard !messages.isEmpty else { return }
        var indexById: [String: Int] = [:]
        indexById.reserveCapacity(session.chatItems.count)
        for (index, item) in session.chatItems.enumerated() {
            indexById[item.id] = index
        }
        var appended = false

        for message in messages {
            for (blockIndex, block) in message.content.enumerated() {
                if case .toolUse(let tool) = block, let index = indexById[tool.id] {
                    guard case .toolCall(let existingTool) = session.chatItems[index].type else { continue }
                    let updated = ToolCallItem(
                        name: tool.name,
                        input: tool.input.isEmpty ? existingTool.input : tool.input,
                        status: existingTool.status,
                        result: existingTool.result,
                        structuredResult: existingTool.structuredResult,
                        subagentTools: existingTool.subagentTools
                    )
                    if updated != existingTool {
                        session.chatItems[index] = ChatHistoryItem(id: tool.id, type: .toolCall(updated), timestamp: message.timestamp)
                    }
                    continue
                }

                guard let item = Self.createChatItem(
                    from: block,
                    message: message,
                    blockIndex: blockIndex,
                    completedTools: completedTools,
                    toolResults: toolResults,
                    structuredResults: structuredResults
                ), indexById[item.id] == nil else { continue }
                indexById[item.id] = session.chatItems.count
                session.chatItems.append(item)
                appended = true
            }
        }
        if sortsByTime && appended {
            session.chatItems.sort { $0.timestamp < $1.timestamp }
        }
    }

    /// Attach subagent tools (read from agent transcripts) to their Task/Agent
    /// items. Only the agents whose lists changed are in `subagentTools`, and
    /// an item whose list is already the same is left untouched.
    private func applySubagentTools(
        _ subagentTools: [String: [SubagentToolInfo]],
        structuredResults: [String: ToolResultData],
        session: inout SessionState
    ) {
        let ids = Set(subagentTools.keys).union(structuredResults.keys)
        guard !ids.isEmpty else { return }
        for index in session.chatItems.indices where ids.contains(session.chatItems[index].id) {
            guard case .toolCall(var tool) = session.chatItems[index].type, tool.isSubagentContainer else { continue }
            let taskToolId = session.chatItems[index].id
            let taskResult: TaskResult?
            if case .task(let result)? = tool.structuredResult ?? structuredResults[taskToolId] { taskResult = result } else { taskResult = nil }
            if let agentId = taskResult?.agentId, !agentId.isEmpty {
                // agentId → description, for AgentOutputTool rows.
                let description = session.subagentState.activeTasks[taskToolId]?.description ?? tool.input["description"]
                if let description {
                    session.subagentState.agentDescriptions[agentId] = description
                }
            }

            guard let infos = subagentTools[taskToolId], !infos.isEmpty else { continue }
            let calls = infos.map { info in
                SubagentToolCall(
                    id: info.id,
                    name: info.name,
                    input: info.input,
                    status: info.isCompleted ? .success : .running,
                    timestamp: info.timestamp ?? session.chatItems[index].timestamp
                )
            }
            guard calls != tool.subagentTools else { continue }
            tool.subagentTools = calls
            session.chatItems[index] = ChatHistoryItem(
                id: taskToolId,
                type: .toolCall(tool),
                timestamp: session.chatItems[index].timestamp
            )
        }
    }

    /// The chat item for one block of a transcript message.
    private static func createChatItem(
        from block: MessageBlock,
        message: ChatMessage,
        blockIndex: Int,
        completedTools: Set<String>,
        toolResults: [String: ConversationParser.ToolResult],
        structuredResults: [String: ToolResultData]
    ) -> ChatHistoryItem? {
        let itemId = "\(message.id)-\(block.typePrefix)-\(blockIndex)"
        switch block {
        case .text(let text):
            // Skip empty text blocks — assistant turns with only tool calls
            // produce empty text blocks that would render as orphan dots/gaps.
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            let type: ChatHistoryItemType = message.role == .user ? .user(text) : .assistant(text)
            return ChatHistoryItem(id: itemId, type: type, timestamp: message.timestamp)

        case .toolUse(let tool):
            let completion = completedTools.contains(tool.id)
                ? ToolCompletionResult.from(parserResult: toolResults[tool.id], structuredResult: structuredResults[tool.id])
                : nil
            return ChatHistoryItem(
                id: tool.id,
                type: .toolCall(ToolCallItem(
                    name: tool.name,
                    input: tool.input,
                    status: completion?.status ?? .running,
                    result: completion?.result,
                    structuredResult: completion?.structuredResult,
                    subagentTools: []
                )),
                timestamp: message.timestamp
            )

        case .thinking(let text):
            // Skip empty thinking blocks — streaming can briefly produce empty
            // ones that would render as orphan grey dots.
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            return ChatHistoryItem(id: itemId, type: .thinking(text), timestamp: message.timestamp)

        case .image(let imageBlock):
            return ChatHistoryItem(id: itemId, type: .image(imageBlock), timestamp: message.timestamp)

        case .interrupted:
            return ChatHistoryItem(id: itemId, type: .interrupted, timestamp: message.timestamp)
        }
    }

    /// Sets a tool item's status. With `onlyIfPending`, a tool that already
    /// finished keeps its final status.
    private func updateToolStatus(in session: inout SessionState, toolId: String, status: ToolStatus, onlyIfPending: Bool = false) {
        guard let index = session.chatItems.lastIndex(where: { $0.id == toolId }),
              case .toolCall(var tool) = session.chatItems[index].type else { return }
        if onlyIfPending && tool.status != .running && tool.status != .waitingForApproval { return }
        guard tool.status != status else { return }
        tool.status = status
        session.chatItems[index] = ChatHistoryItem(
            id: toolId,
            type: .toolCall(tool),
            timestamp: session.chatItems[index].timestamp
        )
    }

    // MARK: - Chat retention

    private func trimChatIfClosed(_ session: inout SessionState) {
        guard !openHistories.contains(session.sessionId) else { return }
        trimChat(&session)
    }

    /// Keeps the newest `retainedChatItems` items, plus any tool still
    /// running or waiting for approval (approvals and completions need them).
    private func trimChat(_ session: inout SessionState) {
        let limit = Self.retainedChatItems
        guard session.chatItems.count > limit else { return }
        let cut = session.chatItems.count - limit
        var kept: [ChatHistoryItem] = []
        kept.reserveCapacity(limit + 8)
        for (index, item) in session.chatItems.enumerated() {
            if index >= cut {
                kept.append(item)
            } else if case .toolCall(let tool) = item.type, tool.status == .running || tool.status == .waitingForApproval {
                kept.append(item)
            }
        }
        session.chatItems = kept
    }

    // MARK: - Interrupt Processing

    private func processInterrupt(sessionId: String, at detectedAt: Date) {
        withSession(sessionId) { session in
            // A prompt typed after the interrupt started a new turn; the
            // interrupt (seen late) is about the old one.
            if let turnStartedAt = session.turnStartedAt, turnStartedAt > detectedAt { return }

            session.subagentState = SubagentState()
            session.toolTracker.endMainTurn()
            session.completionPendingSince = nil
            if session.needsInputReason?.isError != true {
                session.needsInputReason = nil
            }

            // Mark running tools as interrupted
            for index in session.chatItems.indices {
                guard case .toolCall(var tool) = session.chatItems[index].type, tool.status == .running else { continue }
                tool.status = .interrupted
                session.chatItems[index] = ChatHistoryItem(
                    id: session.chatItems[index].id,
                    type: .toolCall(tool),
                    timestamp: session.chatItems[index].timestamp
                )
            }

            if session.phase.isWaitingForApproval {
                dropMainSessionApprovals(from: &session, then: .idle)
            } else if session.phase.canTransition(to: .idle) {
                session.phase = .idle
            }
            // Esc stops the turn, not the agents a background wait is on.
            settleBackgroundWait(&session, now: Date())
        }
    }

    // MARK: - Session End Processing

    private func removeSession(_ sessionId: String) {
        let removed = sessions.removeValue(forKey: sessionId)
        recentlyEnded[sessionId] = Date()
        needsTaskReconstruction.remove(sessionId)
        openHistories.remove(sessionId)
        cancelPendingSync(sessionId: sessionId)
        effects.cancelSessionPermissions(sessionId)
        let transcriptPath = removed?.transcriptPath
        let parser = self.parser
        Task {
            await parser.forget(sessionId: sessionId, transcriptPath: transcriptPath)
        }
    }

    // MARK: - History Loading

    private func loadHistoryFromFile(sessionId: String, cwd: String) async {
        // Sealed fixtures have no transcript, and nothing under ~/.claude* is read.
        guard !DevFlags.isSealed, sessions[sessionId] != nil else { return }
        openHistories.insert(sessionId)
        guard let transcriptPath = resolveTranscriptPath(sessionId: sessionId, fallbackCwd: cwd),
              let history = await parser.fullHistory(sessionId: sessionId, transcriptPath: transcriptPath) else { return }
        processHistoryLoaded(HistoryPayload(sessionId: sessionId, transcriptPath: transcriptPath, history: history))
    }

    private func processHistoryLoaded(_ payload: HistoryPayload) {
        guard openHistories.contains(payload.sessionId) else { return }
        withSession(payload.sessionId) { session in
            applyConversationInfo(payload.history.conversationInfo, to: &session)
            if session.transcriptPath == nil {
                session.transcriptPath = payload.transcriptPath
            }
            mergeMessages(
                payload.history.messages,
                into: &session,
                completedTools: payload.history.completedToolIds,
                toolResults: payload.history.toolResults,
                structuredResults: payload.history.structuredResults,
                sortsByTime: true
            )
            applySubagentTools(payload.history.subagentTools, structuredResults: payload.history.structuredResults, session: &session)
        }
    }

    /// Whether the session's whole history is kept (its chat is open).
    func isHistoryOpen(sessionId: String) -> Bool {
        openHistories.contains(sessionId)
    }

    // MARK: - File Sync Scheduling

    /// Transcript of a session: the hook's path, else located from cwd + account.
    private func resolveTranscriptPath(sessionId: String, fallbackCwd: String? = nil) -> String? {
        guard let session = sessions[sessionId] else { return nil }
        if let path = session.transcriptPath, FileManager.default.fileExists(atPath: path) {
            return path
        }
        let configDir = session.accountId ?? AccountPaths.defaultConfigDir
        let cwd = session.cwd.isEmpty ? (fallbackCwd ?? session.currentCwd) : session.cwd
        // Its own folder first, then ~/.claude; a history shared between
        // them (Claude Parallel Profiles) is read once.
        let located = TranscriptLocator.transcriptPath(
            sessionId: sessionId,
            cwd: cwd,
            configDirs: [configDir, AccountPaths.defaultConfigDir],
            hint: session.transcriptPath
        )
        if let located, session.transcriptPath.map({ !TranscriptLocator.isSameFile($0, located) }) ?? true {
            sessions[sessionId]?.transcriptPath = located
        }
        return located
    }

    private func scheduleFileSync(sessionId: String) {
        // Cancel existing sync
        cancelPendingSync(sessionId: sessionId)

        // Schedule new debounced sync
        pendingSyncs[sessionId] = Task { [weak self, syncDebounceNs] in
            try? await Task.sleep(nanoseconds: syncDebounceNs)
            guard !Task.isCancelled, let self else { return }
            await self.runFileSync(sessionId: sessionId)
        }
    }

    /// One sync per session at a time: `performFileSync` awaits the parser, and
    /// two overlapping syncs could apply their chunks out of order.
    private func runFileSync(sessionId: String) async {
        guard !syncsInFlight.contains(sessionId) else {
            syncsRequestedInFlight.insert(sessionId)
            return
        }
        pendingSyncs.removeValue(forKey: sessionId)
        syncsInFlight.insert(sessionId)
        await performFileSync(sessionId: sessionId)
        syncsInFlight.remove(sessionId)
        if syncsRequestedInFlight.remove(sessionId) != nil, sessions[sessionId] != nil {
            scheduleFileSync(sessionId: sessionId)
        }
    }

    /// Reads what the session's transcript (and its agents) gained, then
    /// applies it in one synchronous step (`.fileUpdated`). Does nothing when
    /// nothing changed, which is what a working session's periodic check
    /// costs: a stat of the transcript and of its running agents.
    private func performFileSync(sessionId: String) async {
        guard let session = sessions[sessionId],
              let transcriptPath = resolveTranscriptPath(sessionId: sessionId) else { return }
        let reconstruct = needsTaskReconstruction.contains(sessionId)
        var wanted: Set<String> = []
        for item in session.chatItems {
            if case .toolCall(let tool) = item.type, tool.status == .running || tool.status == .waitingForApproval {
                wanted.insert(item.id)
            }
        }
        let request = ConversationParser.SyncRequest(
            sessionId: sessionId,
            transcriptPath: transcriptPath,
            keepsFullHistory: openHistories.contains(sessionId),
            wantedToolIds: wanted
        )
        let result = await parser.sync(request)

        // The session may have ended while the parser ran; its parser state
        // (recreated by this sync) must not outlive it.
        guard sessions[sessionId] != nil else {
            await parser.forget(sessionId: sessionId, transcriptPath: transcriptPath)
            return
        }
        guard let result else { return }
        let needsCheck = sessions[sessionId]?.completionCheckSince != nil
        if result.isEmpty && !reconstruct && !needsCheck {
            return
        }
        needsTaskReconstruction.remove(sessionId)

        let payload = FileUpdatePayload(
            sessionId: sessionId,
            transcriptPath: transcriptPath,
            conversationInfo: result.conversationInfo,
            messages: result.newMessages,
            replacesHistory: result.clearDetected || result.didReset,
            completedToolIds: result.completedToolIds,
            toolResults: result.toolResults,
            structuredResults: result.structuredResults,
            subagentTools: result.subagentTools,
            reconstructedTasks: reconstruct ? result.transcriptTasks : nil
        )
        await process(.fileUpdated(payload))
    }

    private func cancelPendingSync(sessionId: String) {
        pendingSyncs[sessionId]?.cancel()
        pendingSyncs.removeValue(forKey: sessionId)
    }

    // MARK: - Periodic Status Check

    /// Start periodic status checking for all sessions
    func startPeriodicStatusCheck() {
        guard statusCheckTask == nil else { return }

        let intervalSeconds = statusCheckIntervalSeconds
        statusCheckTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: intervalSeconds * 1_000_000_000)
                guard !Task.isCancelled else { break }
                await self?.recheckAllSessions()
            }
        }
        Self.logger.info("Started periodic status check (every \(intervalSeconds)s)")
    }

    /// Stop periodic status checking
    func stopPeriodicStatusCheck() {
        statusCheckTask?.cancel()
        statusCheckTask = nil
        Self.logger.info("Stopped periodic status check")
    }

    /// Drops sessions whose process is gone (or whose pid now belongs to
    /// another process) and re-syncs the working ones. Runs every 3 s.
    func recheckAllSessions() {
        let now = Date()
        var removedSession = false

        recentlyEnded = recentlyEnded.filter { now.timeIntervalSince($0.value) < endedSessionMemory }

        for (sessionId, session) in Array(sessions) {
            if session.phase == .ended {
                removeSession(sessionId)
                removedSession = true
                continue
            }

            if let pid = session.pid {
                if !Self.isSameProcessRunning(pid: pid, startedAt: session.pidStartedAt) {
                    Self.logger.info("Process \(pid) of \(sessionId.prefix(8), privacy: .public) ended")
                    removeSession(sessionId)
                    removedSession = true
                    continue
                }
            } else if now.timeIntervalSince(session.lastEventAt) > pidlessSessionTimeout {
                // Known only from the status line and silent since: gone.
                removeSession(sessionId)
                removedSession = true
                continue
            }

            switch session.phase {
            case .processing, .waitingForApproval:
                scheduleFileSync(sessionId: sessionId)
            default:
                break
            }
        }

        if removedSession {
            publishState()
        }
    }

    /// The process is running and is the one first seen with this pid (a
    /// pid reused by an unrelated process started later).
    nonisolated static func isSameProcessRunning(pid: Int, startedAt: Date?) -> Bool {
        guard ProcessID.isRunning(pid) else { return false }
        guard let startedAt else { return true }
        guard let current = ProcessInspector.startDate(pid: pid) else { return true }
        return abs(current.timeIntervalSince(startedAt)) < 1
    }

    // MARK: - State Publishing

    /// Publishes the sessions, coalesced to one publish per `publishInterval`.
    private func publishState() {
        guard !publishScheduled else { return }
        publishScheduled = true
        guard publishInterval > 0 else {
            flushPublish()
            return
        }
        let interval = publishInterval
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(interval))
            await self?.flushPublish()
        }
    }

    private func flushPublish() {
        publishScheduled = false
        let sorted = sessions.values.sorted {
            ($0.projectName, $0.sessionId) < ($1.projectName, $1.sessionId)
        }
        guard sorted != lastPublished else { return }
        lastPublished = sorted
        publishCount += 1
        sessionsSubject.send(sorted)
    }

    // MARK: - Queries

    /// Get a specific session
    func session(for sessionId: String) -> SessionState? {
        sessions[sessionId]
    }

    /// Ids of every tracked session.
    func sessionIds() -> [String] {
        Array(sessions.keys)
    }

    // MARK: - Fixtures

    /// Sealed mode: replace every session with `fixtures` and publish. Nothing
    /// is read; review changes still go to the (temporary) review store.
    /// Sealed mode: every session as the store holds it now, ahead of the
    /// coalesced publish (the sealed demo's steps build on this, so two steps
    /// in a row never build on a publish that hasn't landed yet).
    func fixtureSessions() -> [SessionState] {
        Array(sessions.values)
    }

    func replaceAllWithFixtures(_ fixtures: [SessionState]) {
        sessions = Dictionary(fixtures.map { ($0.sessionId, $0) }, uniquingKeysWith: { first, _ in first })
        recentlyEnded = [:]
        publishState()
    }
}

/// What the store asks of the rest of the app. Live: the shared socket
/// server and registry scanner; tests record or ignore.
nonisolated struct SessionStoreEffects: Sendable {
    /// Close held PermissionRequest sockets without an answer.
    var cancelPermissions: @Sendable ([String]) -> Void
    /// Close every held socket of a session (it ended).
    var cancelSessionPermissions: @Sendable (String) -> Void
    /// Read a config dir's session registry soon (a turn just stopped).
    var rescanRegistry: @Sendable (String) -> Void

    static let live = SessionStoreEffects(
        cancelPermissions: { HookSocketServer.shared.cancelPendingPermissions(toolUseIds: $0) },
        cancelSessionPermissions: { HookSocketServer.shared.cancelPendingPermissions(sessionId: $0) },
        rescanRegistry: { SessionRegistryScanner.shared.scanSoon(configDir: $0) }
    )

    static let none = SessionStoreEffects(
        cancelPermissions: { _ in },
        cancelSessionPermissions: { _ in },
        rescanRegistry: { _ in }
    )
}
