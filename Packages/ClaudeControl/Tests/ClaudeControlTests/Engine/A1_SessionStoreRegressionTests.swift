import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// Regression tests for the session-core review findings: false "done" from
/// blocking Stop hooks, background agents' approvals at the main Stop,
/// prompt sources, registry reconciliation, restarts, failures, retention.
struct A1_SessionStoreRegressionTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-a1-store")
        transcript = account.transcript("s1")
    }

    private var configDir: String { account.configDir.path }

    private func makeStore(
        effects: SessionStoreEffects = .none,
        timing: TurnCompletion.Timing = .immediate,
        reviews: ReviewStateStore? = nil
    ) -> SessionStore {
        SessionStore.forTests(
            reviewStore: reviews ?? ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: effects,
            completionTiming: timing
        )
    }

    private func event(
        _ name: String, status: String, at date: Date = Date(), entrypoint: String = "cli",
        agentId: String? = nil, tool: String? = nil, toolUseId: String? = nil, source: String? = nil,
        prompt: String? = nil, notificationType: String? = nil, message: String? = nil,
        lastAssistantMessage: String? = nil, stopError: String? = nil, stopHookActive: Bool? = nil,
        backgroundTaskTypes: [String]? = nil, sessionCronCount: Int? = nil, trigger: String? = nil,
        synthetic: Bool = false
    ) -> SessionEvent {
        var hookEvent = HookEvent(
            sessionId: "s1", event: name, status: status, cwd: "/tmp/proj", transcriptPath: transcript,
            attended: true, entrypoint: entrypoint, agentId: agentId, tool: tool,
            toolInput: tool == nil ? nil : [:], toolUseId: toolUseId, notificationType: notificationType,
            message: message, lastAssistantMessage: lastAssistantMessage,
            backgroundTaskCount: backgroundTaskTypes?.count, backgroundTaskTypes: backgroundTaskTypes,
            sessionCronCount: sessionCronCount, stopHookActive: stopHookActive, stopError: stopError,
            source: source, prompt: prompt, trigger: trigger, receivedAt: date
        )
        hookEvent.hasSyntheticToolUseId = synthetic
        return .hookReceived(hookEvent)
    }

    private func registry(_ status: String, at date: Date, waitingFor: String? = nil, session: String = "s1", pid: Int = Int(getpid())) -> SessionEvent {
        .registrySnapshot(configDir: configDir, entries: [
            SessionRegistryEntry(pid: pid, sessionId: session, cwd: "/tmp/proj", status: status, waitingFor: waitingFor, statusUpdatedAt: date),
        ])
    }

    private func state(_ store: SessionStore, _ id: String = "s1") async -> SessionState? {
        await store.session(for: id)
    }

    private func eventually(timeout: TimeInterval = 30, _ condition: () async -> Bool) async throws -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while await !condition() {
            guard Date() < deadline else { return false }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        return true
    }

    // MARK: - Stop hooks that continue the turn (/goal)

    @Test func aStopIsDoneOnlyWhenTheRegistrySaysTheTurnEnded() async throws {
        let recorder = EffectsRecorder()
        let slow = TurnCompletion.Timing(fallbackDelay: 60, registryTimeout: 60, clockTolerance: 1)
        let store = makeStore(effects: recorder.effects, timing: slow)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0, source: "user"))
        await store.process(registry("busy", at: t0.addingTimeInterval(0.05)))

        // Our Stop hook ran; a blocking Stop hook (/goal) is still deciding.
        await store.process(event("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(2), lastAssistantMessage: "Step one done"))
        var current = try #require(await state(store))
        #expect(current.attention == .working)
        #expect(current.completedAt == nil)
        #expect(recorder.rescannedDirs == [AccountPaths.normalize(configDir)])

        // It blocked: Claude continues without a prompt.
        await store.process(event("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(3), tool: "Bash", toolUseId: "toolu_c1"))
        #expect(await state(store)?.completionPendingSince == nil)
        #expect(await state(store)?.attention == .working)

        // The continuation ends; this time nothing blocks and Claude Code goes idle.
        let finalStop = t0.addingTimeInterval(5)
        await store.process(event("Stop", status: "waiting_for_input", at: finalStop, lastAssistantMessage: "All steps done", stopHookActive: true))
        #expect(await state(store)?.attention == .working)
        await store.process(registry("idle", at: finalStop.addingTimeInterval(0.1)))
        current = try #require(await state(store))
        #expect(current.attention == .readyForReview)
        #expect(current.completedAt == finalStop)
        #expect(current.lastAssistantMessage == "All steps done")
    }

    @Test func aTextOnlyContinuationEndsWithTheLastStop() async throws {
        let slow = TurnCompletion.Timing(fallbackDelay: 60, registryTimeout: 60, clockTolerance: 1)
        let store = makeStore(timing: slow)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0, source: "user"))
        await store.process(registry("busy", at: t0.addingTimeInterval(0.05)))
        await store.process(event("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(1)))
        // A blocking Stop hook made Claude write more text, then stop again:
        // no event in between, the second Stop carries stop_hook_active.
        let second = t0.addingTimeInterval(4)
        await store.process(event("Stop", status: "waiting_for_input", at: second, lastAssistantMessage: "Goal met", stopHookActive: true))
        #expect(await state(store)?.attention == .working)
        #expect(await state(store)?.completionPendingSince == second)

        await store.process(registry("idle", at: second.addingTimeInterval(0.2)))
        #expect(await state(store)?.attention == .readyForReview)
        #expect(await state(store)?.completedAt == second)
    }

    @Test func withoutARegistryAStopCompletesAfterAQuietMoment() async throws {
        let quick = TurnCompletion.Timing(fallbackDelay: 0.2, registryTimeout: 60, clockTolerance: 1)
        let store = makeStore(timing: quick)
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        let stopAt = Date()
        await store.process(event("Stop", status: "waiting_for_input", at: stopAt))
        #expect(try await eventually { await state(store)?.attention == .readyForReview })
        #expect(await state(store)?.completedAt == stopAt)
    }

    @Test func idleNotificationConfirmsAPendingStop() async throws {
        let slow = TurnCompletion.Timing(fallbackDelay: 60, registryTimeout: 60, clockTolerance: 1)
        let store = makeStore(timing: slow)
        await store.process(event("UserPromptSubmit", status: "processing"))
        await store.process(event("Stop", status: "waiting_for_input"))
        #expect(await state(store)?.attention == .working)
        await store.process(event("Notification", status: "waiting_for_input", notificationType: "idle_prompt"))
        #expect(await state(store)?.attention == .readyForReview)
    }

    @Test func decisionTable() {
        let stop = Date(timeIntervalSince1970: 1_000)
        let timing = TurnCompletion.Timing(fallbackDelay: 4, registryTimeout: 90, clockTolerance: 1)
        func decide(_ status: String?, _ changed: TimeInterval?, now: TimeInterval) -> TurnCompletion.Decision {
            TurnCompletion.decide(stopAt: stop, turnStartedAt: stop.addingTimeInterval(-10), registryStatus: status,
                                  registryChangedAt: changed.map { stop.addingTimeInterval($0) }, now: stop.addingTimeInterval(now), timing: timing)
        }
        #expect(decide("idle", 0.2, now: 0.2) == .confirm)          // idle after the Stop
        #expect(decide("busy", -9.9, now: 1) == .wait)              // busy since the turn began
        #expect(decide("busy", -9.9, now: 91) == .confirm)          // ...but not forever
        #expect(decide("waiting", 0.5, now: 1) == .wait)            // a dialog after the Stop
        #expect(decide("idle", -60, now: 1) == .wait)               // stale idle: registry not following
        #expect(decide("idle", -60, now: 4) == .confirm)            // ...falls back to the quiet period
        #expect(decide(nil, nil, now: 3.9) == .wait)
        #expect(decide(nil, nil, now: 4) == .confirm)
    }

    // MARK: - Background agents and the main Stop

    @Test func backgroundAgentRequestsOutliveTheMainStop() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", agentId: "bg-1", tool: "Bash", toolUseId: "toolu_bg"))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", tool: "Edit", toolUseId: "toolu_main"))
        await store.process(event("Stop", status: "waiting_for_input", backgroundTaskTypes: ["subagent"]))

        var current = try #require(await state(store))
        // The main request is over and its socket closed; the agent's stays.
        #expect(recorder.cancelledPermissions == ["toolu_main"])
        #expect(current.activePermission?.toolUseId == "toolu_bg")
        #expect(current.queuedApprovals.isEmpty)
        #expect(current.attention == .needsInput(.permission(tool: "Bash")))
        #expect(current.completedAt != nil)

        // A wake-up turn (a sibling agent finished) keeps it too...
        await store.process(event("UserPromptSubmit", status: "processing", source: "system"))
        #expect(await state(store)?.activePermission?.toolUseId == "toolu_bg")
        // ...and so does an interrupt of the main turn.
        await store.process(.interruptDetected(sessionId: "s1", at: Date()))
        #expect(await state(store)?.activePermission?.toolUseId == "toolu_bg")

        // Answered: back to the finished turn.
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_bg"))
        current = try #require(await state(store))
        #expect(current.activePermission == nil)
        #expect(current.phase == .idle)
    }

    @Test func mainRequestsEndWithTheTurnAndCloseTheirSockets() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        await store.process(event("PreToolUse", status: "running_tool", tool: "Bash", toolUseId: "toolu_1"))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", tool: "Bash", toolUseId: "toolu_1"))
        await store.process(event("StopFailure", status: "waiting_for_input", stopError: "overloaded"))
        #expect(recorder.cancelledPermissions == ["toolu_1"])
        let current = try #require(await state(store))
        #expect(current.activePermission == nil)
        #expect(current.attention == .needsInput(.error("Overloaded")))
        #expect(current.stopErrorKind == .overloaded)
    }

    // MARK: - What it is doing now

    @Test func runningToolsEndWithTheirCallOrTheMainTurn() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0, source: "user"))
        await store.process(event("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(1), tool: "Read", toolUseId: "toolu_read"))
        await store.process(event("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(2), agentId: "bg-1", tool: "Grep", toolUseId: "toolu_agent"))
        await store.process(event("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(3), tool: "Bash", toolUseId: "toolu_bash"))
        var tracker = try #require(await state(store)).toolTracker
        #expect(tracker.newest?.name == "Bash")
        #expect(Set(tracker.inProgress.keys) == ["toolu_read", "toolu_agent", "toolu_bash"])

        await store.process(event("PermissionRequest", status: "waiting_for_approval", tool: "Bash", toolUseId: "toolu_bash"))
        #expect(try #require(await state(store)).toolTracker.inProgress["toolu_bash"]?.phase == .pendingApproval)
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_bash"))
        #expect(try #require(await state(store)).toolTracker.inProgress["toolu_bash"]?.phase == .running)
        await store.process(event("PostToolUse", status: "processing", tool: "Bash", toolUseId: "toolu_bash"))
        tracker = try #require(await state(store)).toolTracker
        #expect(tracker.newest?.name == "Grep")

        // The main turn ends: its calls are over (the Read never reported
        // back); the background agent's call goes on.
        await store.process(event("Stop", status: "waiting_for_input", backgroundTaskTypes: ["subagent"]))
        tracker = try #require(await state(store)).toolTracker
        #expect(Array(tracker.inProgress.keys) == ["toolu_agent"])
    }

    @Test func theToolTrackerKeepsOnlyTheNewestCalls() {
        var tracker = ToolTracker()
        let t0 = Date()
        for index in 0..<(ToolTracker.maxInProgress + 10) {
            tracker.startTool(id: "t\(index)", name: "Read", agentId: "a", at: t0.addingTimeInterval(Double(index)))
        }
        #expect(tracker.inProgress.count == ToolTracker.maxInProgress)
        #expect(tracker.inProgress["t0"] == nil)
        #expect(tracker.newest?.id == "t\(ToolTracker.maxInProgress + 9)")
        // A repeated start doesn't move a call.
        tracker.startTool(id: "t20", name: "Edit", at: t0.addingTimeInterval(999))
        #expect(tracker.inProgress["t20"]?.name == "Read")
    }

    // MARK: - Click-time identity

    @Test func answersNameTheRequestTheUserSaw() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(1), tool: "Bash", toolUseId: "toolu_a"))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(2), tool: "Bash", toolUseId: "toolu_b"))
        var current = try #require(await state(store))
        #expect(current.activePermission?.activatedAt == t0.addingTimeInterval(1))
        #expect(current.pendingPermission(toolUseId: "toolu_b") != nil)

        // Allow A, then a second click on A (it landed after the row swapped).
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_a"))
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_a"))
        current = try #require(await state(store))
        #expect(current.activePermission?.toolUseId == "toolu_b")
        #expect(current.pendingPermission(toolUseId: "toolu_a") == nil)
        // B was promoted when A was answered (not when B arrived): the UI
        // ignores clicks for a moment after this.
        let activated = try #require(current.activePermission?.activatedAt)
        #expect(activated != current.activePermission?.receivedAt)
        #expect(activated >= t0)
    }

    @Test func aLateOutcomeDoesNotReviveAFinishedTool() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing"))
        await store.process(event("PreToolUse", status: "running_tool", tool: "Bash", toolUseId: "toolu_1"))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", tool: "Bash", toolUseId: "toolu_1"))
        await store.process(event("PostToolUse", status: "processing", tool: "Bash", toolUseId: "toolu_1"))
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_1"))
        let item = try #require(await state(store)?.chatItems.first { $0.id == "toolu_1" })
        guard case .toolCall(let tool) = item.type else { Issue.record("not a tool"); return }
        #expect(tool.status == .success)
    }

    @Test func anInterruptSeenAfterTheNextPromptIsIgnored() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(event("UserPromptSubmit", status: "processing", at: t0.addingTimeInterval(2), source: "user"))
        await store.process(.interruptDetected(sessionId: "s1", at: t0.addingTimeInterval(1)))
        #expect(await state(store)?.attention == .working)
        await store.process(.interruptDetected(sessionId: "s1", at: t0.addingTimeInterval(3)))
        #expect(await state(store)?.phase == .idle)
    }

    @Test func aSealedAnswerChangesTheFixtureOnly() async throws {
        let store = makeStore()
        let socketPath = "/tmp/agentnotch-a1-sealed-\(getpid()).sock"
        let monitor = ClaudeSessionMonitor(store: store, server: HookSocketServer(socketPath: socketPath), sealed: true)
        let request = PermissionContext(toolUseId: "toolu_fixture", toolName: "Bash", toolInput: [:], receivedAt: Date())
        await store.replaceAllWithFixtures([SessionState(sessionId: "s1", cwd: "/tmp/proj", phase: .waitingForApproval(request))])
        monitor.approvePermission(sessionId: "s1", toolUseId: "toolu_fixture")
        #expect(try await eventually { await self.state(store)?.activePermission == nil })
        #expect(await state(store)?.phase == .processing)
        #expect(!FileManager.default.fileExists(atPath: socketPath))
    }

    // MARK: - Prompt sources

    @Test func aVSCodePromptReviewsButATaskNotificationDoesNot() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing", entrypoint: "claude-vscode", source: "sdk", prompt: "fix the build"))
        await store.process(event("Stop", status: "waiting_for_input"))
        #expect(await state(store)?.attention == .readyForReview)

        // A background agent's result wakes Claude: not the user looking.
        await store.process(event("UserPromptSubmit", status: "processing", entrypoint: "claude-vscode", source: "sdk",
                                  prompt: "<task-notification><task-id>a1</task-id> completed</task-notification>"))
        await store.process(.interruptDetected(sessionId: "s1", at: Date()))
        #expect(await state(store)?.attention == .readyForReview)

        // The user types the next prompt in VS Code: reviewed.
        await store.process(event("UserPromptSubmit", status: "processing", entrypoint: "claude-vscode", source: "sdk", prompt: "now the docs"))
        await store.process(.interruptDetected(sessionId: "s1", at: Date()))
        #expect(await state(store)?.attention == .idle)

        // An SDK script (not an editor) never counts.
        let sdkEvent = HookEvent(sessionId: "s1", event: "UserPromptSubmit", status: "processing", cwd: "/tmp",
                                 entrypoint: "sdk-ts", source: "sdk", prompt: "hi")
        #expect(!sdkEvent.isUserAuthoredPrompt)
        let systemEvent = HookEvent(sessionId: "s1", event: "UserPromptSubmit", status: "processing", cwd: "/tmp", source: "system")
        #expect(!systemEvent.isUserAuthoredPrompt)
    }

    // MARK: - Quiet completions

    @Test func turnsWaitingOnAgentsOrLoopsAreQuiet() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        await store.process(event("Stop", status: "waiting_for_input", backgroundTaskTypes: ["subagent", "shell"]))
        var current = try #require(await state(store))
        // The subagent will wake Claude: not done yet (see BackgroundWaitTests).
        #expect(current.attention == .working)
        #expect(current.backgroundTaskCount == 2)
        #expect(current.backgroundAgentCount == 1)
        #expect(current.completionIsQuiet)

        // A dev server alone isn't waited on: that turn is done.
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        await store.process(event("Stop", status: "waiting_for_input", backgroundTaskTypes: ["shell"]))
        #expect(await state(store)?.completionIsQuiet == false)
        #expect(await state(store)?.attention == .readyForReview)

        // A /loop tick.
        await store.process(event("UserPromptSubmit", status: "processing", source: "loop_wakeup"))
        await store.process(event("Stop", status: "waiting_for_input", sessionCronCount: 1))
        current = try #require(await state(store))
        #expect(current.completionIsQuiet)
        #expect(current.scheduledWakeupCount == 1)
    }

    // MARK: - Compaction, agent view, failures

    @Test func backgroundCompactionDoesNotReopenAFinishedTurn() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing", source: "user"))
        await store.process(event("Stop", status: "waiting_for_input"))
        await store.process(event("PreCompact", status: "compacting", trigger: "auto"))
        await store.process(event("PostCompact", status: "processing", trigger: "auto"))
        #expect(await state(store)?.attention == .readyForReview)

        // The user's own /compact at the prompt still shows.
        await store.process(event("PreCompact", status: "compacting", trigger: "manual"))
        #expect(await state(store)?.phase == .compacting)
        await store.process(event("PostCompact", status: "waiting_for_input", trigger: "manual"))
        #expect(await state(store)?.phase == .waitingForInput)
    }

    @Test func agentViewAnnouncementsAreAboutOtherSessions() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing"))
        await store.process(event("Notification", status: "notification", notificationType: "agent_needs_input", message: "worker-3 needs your input: pick a db"))
        await store.process(event("Notification", status: "notification", notificationType: "agent_completed", message: "worker-3 finished"))
        #expect(await state(store)?.needsInputReason == nil)
        await store.process(event("Notification", status: "notification", notificationType: "agent_needs_input", message: "Choose how to set up teammates"))
        #expect(await state(store)?.needsInputReason == .dialog("Choose how to set up teammates"))
    }

    @Test func aFailureKeepsTheLastReplyAndSurvivesARestart() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        let store = makeStore(reviews: reviews)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(event("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(1), lastAssistantMessage: "Real answer"))
        await store.process(event("UserPromptSubmit", status: "processing", at: t0.addingTimeInterval(2), source: "loop_wakeup"))
        await store.process(event("StopFailure", status: "waiting_for_input", at: t0.addingTimeInterval(3),
                                  lastAssistantMessage: "API Error: Rate limit reached", stopError: "rate_limit"))
        let failed = try #require(await state(store))
        #expect(failed.lastAssistantMessage == "Real answer")
        #expect(failed.stopErrorKind == .rateLimit)
        #expect(failed.hasFailedTurn)
        #expect(failed.attention.isError)
        reviews.flush()

        // Next run: rediscovered through the registry, still failed.
        let second = makeStore()
        await second.process(registry("idle", at: t0.addingTimeInterval(4)))
        let restored = try #require(await state(second))
        #expect(restored.attention == .needsInput(.error("Rate limited")))
        #expect(restored.stopErrorCode == "rate_limit")
    }

    @Test func aRestoredFailureClearsOnceTheUserMovedOn() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        let t0 = Date(timeIntervalSinceNow: -60)
        reviews.update(sessionId: "s1", record: ReviewRecord(stopError: "Rate limited", stopErrorCode: "rate_limit", failedAt: t0, updatedAt: t0), urgent: true)
        reviews.flush()
        try TranscriptLines.append([TranscriptLines.user("try again", at: t0.addingTimeInterval(30))], to: transcript)

        let store = makeStore()
        await store.process(event("SessionStart", status: "waiting_for_input", source: "resume"))
        #expect(try await eventually { await state(store)?.hasFailedTurn == false })
    }

    // MARK: - Registry reconciliation

    @Test func statusLineUpdatesDoNotHideARegistryCorrection() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(registry("busy", at: t0.addingTimeInterval(0.1)))
        let interruptedAt = t0.addingTimeInterval(1)
        let status = try #require(StatusLineMessage(json: [
            "event": "StatusLine", "session_id": "s1", "transcript_path": transcript,
            "status_line": ["context_window": ["used_percentage": 12]],
        ], receivedAt: t0.addingTimeInterval(2)))
        await store.process(.statusLineReceived(status))
        await store.process(registry("idle", at: interruptedAt))
        #expect(await state(store)?.phase == .idle)
    }

    @Test func aSyntheticRequestAnsweredInTheTerminalIsResolvedByTheRegistry() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(registry("busy", at: t0.addingTimeInterval(0.1)))
        await store.process(event("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(1), tool: "Bash", toolUseId: "permission-x", synthetic: true))
        await store.process(registry("waiting", at: t0.addingTimeInterval(1.2), waitingFor: "permission prompt"))
        #expect(await state(store)?.activePermission?.toolUseId == "permission-x")

        await store.process(registry("busy", at: t0.addingTimeInterval(5)))
        #expect(await state(store)?.activePermission == nil)
        #expect(await state(store)?.attention == .working)
        #expect(recorder.cancelledPermissions == ["permission-x"])
    }

    @Test func aSessionWithoutHooksCompletesFromItsTranscript() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(registry("busy", at: t0))
        #expect(await state(store)?.attention == .working)
        try TranscriptLines.append([
            TranscriptLines.user("go", at: t0.addingTimeInterval(0.5)),
            TranscriptLines.assistantText("Finished the refactor.", at: t0.addingTimeInterval(2)),
        ], to: transcript)
        await store.process(registry("idle", at: t0.addingTimeInterval(3)))
        #expect(try await eventually { await state(store)?.attention == .readyForReview })
        let current = try #require(await state(store))
        #expect(current.lastAssistantMessage == "Finished the refactor.")
        #expect(current.completedAt == ConversationParser.parseDate(TranscriptLines.iso(t0.addingTimeInterval(2))))
    }

    @Test func anInterruptedSessionWithoutHooksIsNotDone() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(registry("busy", at: t0))
        try TranscriptLines.append([
            TranscriptLines.assistantText("Starting…", at: t0.addingTimeInterval(1)),
            TranscriptLines.user("[Request interrupted by user]", at: t0.addingTimeInterval(2)),
        ], to: transcript)
        await store.process(registry("idle", at: t0.addingTimeInterval(3)))
        try await Task.sleep(nanoseconds: 600_000_000)
        #expect(await state(store)?.completedAt == nil)
        #expect(await state(store)?.attention == .idle)
    }

    @Test func aPidNowNamingAnotherSessionDropsTheOldOne() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(registry("idle", at: t0, session: "old"))
        #expect(await state(store, "old") != nil)
        await store.process(registry("idle", at: t0.addingTimeInterval(1), session: "new"))
        #expect(await state(store, "old") == nil)
        #expect(await state(store, "new") != nil)
    }

    @Test func aHookSessionIsNotDroppedByAStaleRegistryEntry() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(registry("idle", at: t0, session: "s1"))
        // /clear: the new session's hooks arrive before the registry catches up.
        var builder = HookEventBuilder(event: "SessionStart", status: "waiting_for_input", transcriptPath: transcript)
        builder.sessionId = "s2"
        let start = builder.build()
        await store.process(.hookReceived(HookEvent(
            sessionId: "s2", event: "SessionStart", status: "waiting_for_input", cwd: start.cwd, pid: Int(getpid()),
            transcriptPath: transcript, attended: true, entrypoint: "cli", source: "clear", receivedAt: t0.addingTimeInterval(2)
        )))
        await store.process(registry("idle", at: t0.addingTimeInterval(0.5), session: "s1"))
        #expect(await state(store, "s2") != nil)
    }

    // MARK: - Restarts

    /// A review file whose previous run was last alive `ago` seconds back.
    private func previousRun(aliveSecondsAgo ago: TimeInterval) throws -> Date {
        let aliveAt = Date().addingTimeInterval(-ago)
        let contents: [String: Any] = ["version": 2, "lastAliveAt": aliveAt.timeIntervalSince1970, "sessions": [String: Any]()]
        try JSONSerialization.data(withJSONObject: contents).write(to: account.reviewFile)
        return try #require(ReviewStateStore(fileURL: account.reviewFile, createsFolder: false).previousRunAliveAt)
    }

    @Test func aTurnThatFinishedWhileTheAppWasDownIsReadyForReview() async throws {
        let aliveAt = try previousRun(aliveSecondsAgo: 600)  // quit ten minutes ago
        try TranscriptLines.append([
            TranscriptLines.user("long job", at: aliveAt.addingTimeInterval(-60)),
            TranscriptLines.assistantText("All done while you were away.", at: aliveAt.addingTimeInterval(5)),
        ], to: transcript)

        let store = makeStore()
        await store.process(registry("idle", at: aliveAt.addingTimeInterval(6)))
        #expect(try await eventually { await state(store)?.attention == .readyForReview })
        #expect(await state(store)?.lastAssistantMessage == "All done while you were away.")
    }

    @Test func aReplyWrittenMidTurnIsNotTakenForAFinishedTurn() async throws {
        let aliveAt = try previousRun(aliveSecondsAgo: 600)
        // Claude is mid-turn: it wrote text and is about to call a tool.
        try TranscriptLines.append([
            TranscriptLines.user("big refactor", at: aliveAt.addingTimeInterval(-60)),
            TranscriptLines.assistantText("Now updating the call sites.", at: aliveAt.addingTimeInterval(5)),
        ], to: transcript)

        // Found through its status line (no registry status yet).
        let store = makeStore()
        let status = try #require(StatusLineMessage(json: [
            "event": "StatusLine", "session_id": "s1", "transcript_path": transcript, "status_line": [:],
        ]))
        await store.process(.statusLineReceived(status))
        try await Task.sleep(nanoseconds: 500_000_000)
        #expect(await state(store)?.completedAt == nil)
        #expect(await state(store)?.attention == .idle)
    }

    @Test func noCompletionIsInferredOnAFirstRunOrFromOldWork() async throws {
        let t0 = Date()
        try TranscriptLines.append([
            TranscriptLines.user("job", at: t0.addingTimeInterval(-120)),
            TranscriptLines.assistantText("Done long ago.", at: t0.addingTimeInterval(-100)),
        ], to: transcript)

        // First run ever: no heartbeat, nothing inferred.
        let firstRun = makeStore()
        await firstRun.process(registry("idle", at: t0))
        try await Task.sleep(nanoseconds: 500_000_000)
        #expect(await state(firstRun)?.attention == .idle)

        // A heartbeat newer than the reply: the app saw that turn end.
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        reviews.stopHeartbeat()
        let later = makeStore()
        await later.process(registry("idle", at: t0, session: "s1"))
        try await Task.sleep(nanoseconds: 500_000_000)
        #expect(await state(later)?.attention == .idle)
    }

    @Test func completionsAreWrittenAtOnce() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 30, createsFolder: false)
        let store = makeStore(reviews: reviews)
        await store.process(event("UserPromptSubmit", status: "processing"))
        await store.process(event("Stop", status: "waiting_for_input", lastAssistantMessage: "Saved"))
        // No flush: the urgent write happens on its own.
        #expect(try await eventually { ReviewStateStore(fileURL: account.reviewFile, createsFolder: false).record(for: "s1")?.completedAt != nil })
    }

    @Test func markViewedReviewsOnlyThatCompletion() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0))
        await store.process(event("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(1)))
        await store.process(.markViewed(sessionId: "s1", completedAt: t0))
        #expect(await state(store)?.attention == .readyForReview)
        await store.process(.markViewed(sessionId: "s1", completedAt: t0.addingTimeInterval(1)))
        #expect(await state(store)?.attention == .idle)
    }

    @Test func aWaitStartsWhenItsEventArrived() async throws {
        let store = makeStore()
        let t0 = Date().addingTimeInterval(-30)
        await store.process(event("UserPromptSubmit", status: "processing", at: t0, source: "user"))
        await store.process(event("Notification", status: "notification", at: t0.addingTimeInterval(5),
                                  notificationType: "elicitation_dialog", message: "Pick a file"))
        #expect(await state(store)?.waitingSince == t0.addingTimeInterval(5))
        // Another reason while still waiting keeps the original time.
        await store.process(event("Notification", status: "notification", at: t0.addingTimeInterval(9),
                                  notificationType: "agent_needs_input", message: "Choose one"))
        #expect(await state(store)?.waitingSince == t0.addingTimeInterval(5))
        await store.process(event("StopFailure", status: "waiting_for_input", at: t0.addingTimeInterval(20), stopError: "rate_limit"))
        #expect(await state(store)?.waitingSince == t0.addingTimeInterval(5))
    }

    @Test func aReviewMarkCoversOnlyWhatFinishedBeforeTheClick() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(event("UserPromptSubmit", status: "processing", at: t0, source: "user"))
        await store.process(event("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(2)))
        // Clicked before the Stop was processed: that turn is still unseen.
        await store.process(.markReviewed(sessionId: "s1", at: t0.addingTimeInterval(1)))
        #expect(await state(store)?.attention == .readyForReview)
        await store.process(.markReviewed(sessionId: "s1", at: t0.addingTimeInterval(3)))
        #expect(await state(store)?.attention == .idle)
        // A later, earlier-dated mark never takes a review back.
        await store.process(.markReviewed(sessionId: "s1", at: t0))
        #expect(await state(store)?.reviewedAt == t0.addingTimeInterval(3))
    }

    // MARK: - Process identity

    @Test func aReusedPidEndsTheSession() async throws {
        let store = makeStore()
        var stale = SessionState(sessionId: "reused", cwd: "/tmp/proj", pid: Int(getpid()))
        stale.pidStartedAt = Date(timeIntervalSince1970: 1_000)
        var current = SessionState(sessionId: "current", cwd: "/tmp/proj", pid: Int(getpid()))
        current.pidStartedAt = ProcessInspector.startDate(pid: Int(getpid()))
        await store.replaceAllWithFixtures([stale, current])
        await store.recheckAllSessions()
        #expect(await state(store, "reused") == nil)
        #expect(await state(store, "current") != nil)
    }

    @Test func processFactsComeFromTheKernel() throws {
        let info = try #require(ProcessInspector.info(pid: Int(getpid())))
        #expect(info.parentPid == Int(getppid()))
        #expect(!info.command.isEmpty)
        #expect(info.startedAt < Date())
        #expect(ProcessInspector.info(pid: Int(Int32.max)) == nil)
        #expect(ProcessInspector.terminalName(device: 0) == nil)
        #expect(ProcessInspector.terminalName(device: UInt32.max) == nil)
        #expect(SessionStore.isSameProcessRunning(pid: Int(getpid()), startedAt: info.startedAt))
        #expect(!SessionStore.isSameProcessRunning(pid: Int(getpid()), startedAt: info.startedAt.addingTimeInterval(-3600)))
    }

    // MARK: - Memory

    @Test func aClosedChatKeepsOnlyTheNewestItemsAndRunningTools() async throws {
        let store = makeStore()
        await store.process(event("UserPromptSubmit", status: "processing"))
        await store.process(event("PreToolUse", status: "running_tool", tool: "Bash", toolUseId: "toolu_long"))
        for index in 0..<200 {
            await store.process(event("PreToolUse", status: "running_tool", tool: "Read", toolUseId: "toolu_\(index)"))
            await store.process(event("PostToolUse", status: "processing", tool: "Read", toolUseId: "toolu_\(index)"))
        }
        let items = try #require(await state(store)?.chatItems)
        #expect(items.count <= SessionStore.retainedChatItems + 1)
        #expect(items.contains { $0.id == "toolu_long" })
        #expect(items.last?.id == "toolu_199")
    }

    @Test func anOpenChatKeepsEverythingUntilReleased() async throws {
        var lines: [[String: Any]] = []
        for index in 0..<120 {
            lines.append(TranscriptLines.assistantText("line \(index)"))
        }
        try TranscriptLines.append(lines, to: transcript)
        let store = makeStore()
        await store.process(event("SessionStart", status: "waiting_for_input", source: "startup"))
        await store.process(.loadHistory(sessionId: "s1", cwd: "/tmp/proj"))
        #expect(await state(store)?.chatItems.count == 120)
        #expect(await store.isHistoryOpen(sessionId: "s1"))
        await store.process(.releaseHistory(sessionId: "s1"))
        #expect(await state(store)?.chatItems.count == SessionStore.retainedChatItems)
    }

    @Test func anEndedSessionLeavesNoParserState() async throws {
        let parser = ConversationParser()
        let store = SessionStore.forTests(reviewFile: account.reviewFile, parser: parser)
        try TranscriptLines.append([TranscriptLines.assistantText("hi")], to: transcript)
        await store.process(event("UserPromptSubmit", status: "processing"))
        #expect(try await eventually { await parser.trackedSessionIds.contains("s1") })
        await store.process(event("SessionEnd", status: "ended"))
        #expect(try await eventually { await parser.trackedSessionIds.isEmpty })
    }

    // MARK: - Publishing

    @Test func aBurstIsPublishedInAFewCoalescedUpdates() async throws {
        let store = SessionStore(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            parser: ConversationParser(),
            publishInterval: 0.05,
            effects: .none,
            completionTiming: .immediate
        )
        for index in 0..<300 {
            await store.process(event("PreToolUse", status: "running_tool", tool: "Read", toolUseId: "toolu_\(index)"))
        }
        try await Task.sleep(nanoseconds: 200_000_000)
        let published = await store.publishCount
        #expect(published >= 1)
        #expect(published < 60)
        #expect(store.sessionsPublisher.value.first?.chatItems.last?.id == "toolu_299")

        // An unchanged state isn't published again.
        await store.process(.markAllReviewed(at: Date()))
        try await Task.sleep(nanoseconds: 150_000_000)
        #expect(await store.publishCount == published)
    }
}

private extension AnyPublisher where Output == [SessionState], Failure == Never {
    /// The current value of the store's CurrentValueSubject-backed publisher.
    var value: [SessionState] {
        var result: [SessionState] = []
        let cancellable = sink { result = $0 }
        cancellable.cancel()
        return result
    }
}
