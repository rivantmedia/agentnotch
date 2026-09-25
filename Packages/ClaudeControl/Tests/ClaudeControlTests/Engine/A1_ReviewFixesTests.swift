import Foundation
import Testing
@testable import ClaudeControl

/// Regressions found reviewing the session core: a pipeline that can start
/// again after `stop`, unobserved completions only for turns that ended
/// while the app was down, subagents never ending the main turn, requests
/// matched to no call settled by the registry even while hooks keep coming,
/// review marks at the click, and bounded agent bookkeeping.
@Suite(.serialized)
struct A1_ReviewFixesTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-a1-review")
        transcript = account.transcript("s1")
    }

    private var configDir: String { account.configDir.path }

    private func makeStore(
        effects: SessionStoreEffects = .none,
        startedAt: Date = Date()
    ) -> SessionStore {
        SessionStore(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            parser: ConversationParser(),
            publishInterval: 0,
            effects: effects,
            completionTiming: .immediate,
            startedAt: startedAt
        )
    }

    private func hook(
        _ name: String, status: String, at date: Date = Date(), session: String = "s1",
        agentId: String? = nil, tool: String? = nil, toolUseId: String? = nil,
        source: String? = nil, stopError: String? = nil, synthetic: Bool = false
    ) -> HookEvent {
        var event = HookEvent(
            sessionId: session, event: name, status: status, cwd: "/tmp/proj", transcriptPath: transcript,
            attended: true, entrypoint: "cli", agentId: agentId, tool: tool,
            toolInput: tool == nil ? nil : [:], toolUseId: toolUseId, stopError: stopError,
            source: source, receivedAt: date
        )
        event.hasSyntheticToolUseId = synthetic
        return event
    }

    private func registry(_ status: String, at date: Date, waitingFor: String? = nil) -> SessionEvent {
        .registrySnapshot(configDir: configDir, entries: [
            SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: status, waitingFor: waitingFor, statusUpdatedAt: date),
        ])
    }

    private func eventually(timeout: TimeInterval = 10, _ condition: () async -> Bool) async throws -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while await !condition() {
            guard Date() < deadline else { return false }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        return true
    }

    // MARK: - Restarting the pipeline

    /// A cancelled consumer ends its AsyncStream for good; `stop` then
    /// `start` (the hub stopped and started again) must still apply events.
    @Test func thePipelineAppliesEventsAfterARestart() async throws {
        let store = makeStore()
        let pipeline = HookEventPipeline(store: store)
        pipeline.start(effects: .none)
        pipeline.yield(.socket(.hook(hook("UserPromptSubmit", status: "processing", session: "before"))))
        #expect(try await eventually { await store.session(for: "before") != nil })

        pipeline.stop()
        pipeline.start(effects: .none)
        pipeline.yield(.socket(.hook(hook("UserPromptSubmit", status: "processing", session: "after"))))
        #expect(try await eventually { await store.session(for: "after") != nil })
        #expect(try await eventually { pipeline.backlog == 0 })
        pipeline.stop()
    }

    @Test func inputsYieldedWhileStoppedWaitForTheNextStart() async throws {
        let store = makeStore()
        let pipeline = HookEventPipeline(store: store)
        pipeline.start(effects: .none)
        pipeline.stop()
        pipeline.yield(.socket(.hook(hook("UserPromptSubmit", status: "processing", session: "queued"))))
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(await store.session(for: "queued") == nil)

        pipeline.start(effects: .none)
        #expect(try await eventually { await store.session(for: "queued") != nil })
        #expect(try await eventually { pipeline.backlog == 0 })
        pipeline.stop()
    }

    @MainActor
    @Test func aMonitorStoppedAndStartedAgainStillListens() async throws {
        let socketPath = "/tmp/agentnotch-a1r-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
        let store = makeStore()
        let monitor = ClaudeSessionMonitor(store: store, server: HookSocketServer(socketPath: socketPath))
        monitor.startSessionPipeline()
        monitor.stop()
        #expect(!FileManager.default.fileExists(atPath: socketPath))

        monitor.startSessionPipeline()
        defer { monitor.stop() }
        #expect(try await eventually { FileManager.default.fileExists(atPath: socketPath) })
        monitor.enqueue(.socket(.hook(hook("UserPromptSubmit", status: "processing"))))
        #expect(try await eventually { await store.session(for: "s1")?.attention == .working })
    }

    // MARK: - Completions nobody saw

    /// A review file whose previous run was last alive `ago` seconds back.
    private func previousRun(aliveSecondsAgo ago: TimeInterval) throws -> Date {
        let aliveAt = Date().addingTimeInterval(-ago)
        let contents: [String: Any] = ["version": 2, "lastAliveAt": aliveAt.timeIntervalSince1970, "sessions": [String: Any]()]
        try JSONSerialization.data(withJSONObject: contents).write(to: account.reviewFile)
        return try #require(ReviewStateStore(fileURL: account.reviewFile, createsFolder: false).previousRunAliveAt)
    }

    /// An account tracked hours into a run: its idle sessions finished while
    /// the app was up (just not following them), not while it was down, so
    /// their old replies don't flood the review queue.
    @Test func sessionsFoundIdleLongAfterLaunchAreNotInferredDone() async throws {
        let aliveAt = try previousRun(aliveSecondsAgo: 4 * 3600)
        let launchedAt = aliveAt.addingTimeInterval(60)
        try TranscriptLines.append([
            TranscriptLines.user("refactor", at: launchedAt.addingTimeInterval(600)),
            TranscriptLines.assistantText("Refactored.", at: launchedAt.addingTimeInterval(900)),
        ], to: transcript)

        let store = makeStore(startedAt: launchedAt)
        await store.process(registry("idle", at: launchedAt.addingTimeInterval(901)))
        try await Task.sleep(nanoseconds: 400_000_000)
        #expect(await store.session(for: "s1")?.completedAt == nil)
        #expect(await store.session(for: "s1")?.attention == .idle)
    }

    @Test func sessionsThatWentIdleWhileTheAppWasDownAreInferredDone() async throws {
        let aliveAt = try previousRun(aliveSecondsAgo: 4 * 3600)
        let launchedAt = aliveAt.addingTimeInterval(3600)
        try TranscriptLines.append([
            TranscriptLines.user("migrate", at: aliveAt.addingTimeInterval(-60)),
            TranscriptLines.assistantText("Migrated.", at: aliveAt.addingTimeInterval(600)),
        ], to: transcript)

        let store = makeStore(startedAt: launchedAt)
        await store.process(registry("idle", at: aliveAt.addingTimeInterval(601)))
        #expect(try await eventually { await store.session(for: "s1")?.attention == .readyForReview })
    }

    // MARK: - Subagents and the main turn

    @Test func aSubagentsStopDoesNotEndTheMainTurn() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        let t0 = Date()
        await store.process(.hookReceived(hook("UserPromptSubmit", status: "processing", at: t0, source: "user")))
        await store.process(.hookReceived(hook("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(1), tool: "Bash", toolUseId: "toolu_main")))
        await store.process(.hookReceived(hook("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(2), agentId: "teammate-1")))
        #expect(await store.session(for: "s1")?.phase == .processing)
        #expect(await store.session(for: "s1")?.attention == .working)

        // Nor does it drop the main session's request.
        await store.process(.hookReceived(hook("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(3), tool: "Bash", toolUseId: "toolu_main")))
        await store.process(.hookReceived(hook("StopFailure", status: "waiting_for_input", at: t0.addingTimeInterval(4), agentId: "teammate-1", stopError: "rate_limit")))
        #expect(await store.session(for: "s1")?.activePermission?.toolUseId == "toolu_main")
        #expect(recorder.cancelledPermissions.isEmpty)

        // The main Stop does end it.
        await store.process(.hookReceived(hook("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(5))))
        #expect(await store.session(for: "s1")?.activePermission == nil)
        #expect(recorder.cancelledPermissions == ["toolu_main"])
    }

    // MARK: - Requests matched to no call

    /// Answered in the terminal while parallel calls kept firing hooks: the
    /// registry's "busy" arrives after those hooks, and still settles it.
    @Test func aSyntheticRequestIsSettledByTheRegistryEvenAfterLaterHooks() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        let t0 = Date()
        await store.process(.hookReceived(hook("UserPromptSubmit", status: "processing", at: t0, source: "user")))
        await store.process(registry("busy", at: t0.addingTimeInterval(0.1)))
        await store.process(.hookReceived(hook("PreToolUse", status: "running_tool", at: t0.addingTimeInterval(0.5), tool: "Bash", toolUseId: "toolu_ls")))
        await store.process(.hookReceived(hook("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(1), tool: "Bash", toolUseId: "permission-x", synthetic: true)))
        // The parallel call finishes after the user answered at t0+2.
        await store.process(.hookReceived(hook("PostToolUse", status: "processing", at: t0.addingTimeInterval(3), tool: "Bash", toolUseId: "toolu_ls")))
        #expect(await store.session(for: "s1")?.activePermission?.toolUseId == "permission-x")

        await store.process(registry("busy", at: t0.addingTimeInterval(2)))
        #expect(await store.session(for: "s1")?.activePermission == nil)
        #expect(await store.session(for: "s1")?.attention == .working)
        #expect(recorder.cancelledPermissions == ["permission-x"])
    }

    /// A background agent's request shows no dialog while our hook waits,
    /// so the main session's registry status says nothing about it.
    @Test func aBackgroundAgentsSyntheticRequestIgnoresTheRegistry() async throws {
        let recorder = EffectsRecorder()
        let store = makeStore(effects: recorder.effects)
        let t0 = Date()
        await store.process(.hookReceived(hook("UserPromptSubmit", status: "processing", at: t0, source: "user")))
        await store.process(.hookReceived(hook("PermissionRequest", status: "waiting_for_approval", at: t0.addingTimeInterval(1), agentId: "bg-1", tool: "Bash", toolUseId: "permission-bg", synthetic: true)))
        await store.process(registry("busy", at: t0.addingTimeInterval(4)))
        #expect(await store.session(for: "s1")?.activePermission?.toolUseId == "permission-bg")
        #expect(recorder.cancelledPermissions.isEmpty)
    }

    // MARK: - Review marks

    @Test func markAllReviewedKeepsWhatFinishedAfterTheClick() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(.hookReceived(hook("UserPromptSubmit", status: "processing", at: t0, source: "user")))
        await store.process(.hookReceived(hook("Stop", status: "waiting_for_input", at: t0.addingTimeInterval(2))))
        #expect(await store.session(for: "s1")?.attention == .readyForReview)

        // Clicked before the turn finished; processed after.
        await store.process(.markAllReviewed(at: t0.addingTimeInterval(1)))
        #expect(await store.session(for: "s1")?.attention == .readyForReview)

        await store.process(.markAllReviewed(at: t0.addingTimeInterval(3)))
        #expect(await store.session(for: "s1")?.attention == .idle)
        // An older click never moves a review back.
        await store.process(.markAllReviewed(at: t0))
        #expect(await store.session(for: "s1")?.reviewedAt == t0.addingTimeInterval(3))
    }

    // MARK: - Failures

    @Test func theErrorKindGoesWithTheFailure() async throws {
        let store = makeStore()
        let t0 = Date()
        await store.process(.hookReceived(hook("UserPromptSubmit", status: "processing", at: t0, source: "user")))
        await store.process(.hookReceived(hook("StopFailure", status: "waiting_for_input", at: t0.addingTimeInterval(1), stopError: "rate_limit")))
        #expect(await store.session(for: "s1")?.stopErrorKind == .rateLimit)

        // A late main-session event settles the needs-you state; the kind goes with it.
        await store.process(.hookReceived(hook("PostToolUse", status: "processing", at: t0.addingTimeInterval(2), tool: "Bash", toolUseId: "toolu_late")))
        let state = try #require(await store.session(for: "s1"))
        #expect(!state.hasFailedTurn)
        #expect(state.stopErrorKind == nil)
    }

    // MARK: - Agent bookkeeping

    @Test func settledAgentsAreBounded() {
        var state = SessionParseState(filePath: "/tmp/none.jsonl")
        for index in 0..<5000 {
            state.settle("toolu_agent_\(index)")
        }
        #expect(state.settledAgents.count <= SessionParseState.maxSettledAgents * 2)
        #expect(state.settledAgents.contains("toolu_agent_4999"))
        #expect(!state.settledAgents.contains("toolu_agent_0"))
    }
}
