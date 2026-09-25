import Foundation
import Testing
@testable import ClaudeControl

/// Drives a private SessionStore with hook sequences and checks attention,
/// review tracking and ordering protections.
struct SessionStoreFlowTests {
    /// Removed with the suite instance.
    private let account: TemporaryAccount
    private let directory: URL
    private let transcript: String
    private let reviewFile: URL

    init() throws {
        account = try TemporaryAccount(prefix: "spcn-store")
        directory = account.root
        transcript = account.transcript("s1")
        reviewFile = account.reviewFile
    }

    private func makeStore() -> SessionStore {
        SessionStore.forTests(reviewFile: reviewFile)
    }

    private func hook(_ name: String, status: String, _ configure: (inout HookEventBuilder) -> Void = { _ in }) -> SessionEvent {
        var builder = HookEventBuilder(event: name, status: status, transcriptPath: transcript)
        configure(&builder)
        return .hookReceived(builder.build())
    }

    private func session(_ store: SessionStore) async -> SessionState? {
        await store.session(for: "s1")
    }

    @Test func turnCompletesIntoReviewAndPromptReviews() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "user" })
        #expect(await session(store)?.attention == .working)

        await store.process(hook("PreToolUse", status: "running_tool") { $0.tool = "Bash"; $0.toolUseId = "toolu_1" })
        await store.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Done: tests pass" })
        let finished = try #require(await session(store))
        #expect(finished.attention == .readyForReview)
        #expect(finished.lastAssistantMessage == "Done: tests pass")
        #expect(finished.completedAt != nil)

        // idle_prompt must not downgrade the review state.
        await store.process(hook("Notification", status: "waiting_for_input") { $0.notificationType = "idle_prompt" })
        #expect(await session(store)?.attention == .readyForReview)

        // A loop/cron turn is not the user looking.
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "loop_wakeup" })
        await store.process(hook("Stop", status: "waiting_for_input"))
        #expect(await session(store)?.attention == .readyForReview)

        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "user" })
        await store.process(.markReviewed(sessionId: "s1"))
        #expect(await session(store)?.attention == .working)
    }

    @Test func lateEventsDontResurrectAFinishedSession() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("Stop", status: "waiting_for_input"))
        await store.process(hook("PostToolUse", status: "processing") { $0.tool = "Bash"; $0.toolUseId = "toolu_bg" })
        await store.process(hook("SubagentStop", status: "processing"))
        await store.process(hook("PreToolUse", status: "running_tool") { $0.agentId = "agent-1"; $0.tool = "Read"; $0.toolUseId = "toolu_sub" })
        #expect(await session(store)?.phase == .waitingForInput)
        #expect(await session(store)?.attention == .readyForReview)
    }

    @Test func stopWithBackgroundTasksIsReadyForReview() async {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("Stop", status: "waiting_for_input") { $0.backgroundTaskCount = 1 })
        #expect(await session(store)?.attention == .readyForReview)
        #expect(await session(store)?.backgroundTaskCount == 1)
    }

    @Test func contextResumeStopIsNotACompletion() async {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("Stop", status: "waiting_for_input") {
            $0.lastAssistantMessage = "This session is being continued from a previous conversation that ran out of context."
        })
        #expect(await session(store)?.completedAt == nil)
        #expect(await session(store)?.attention == .idle)
    }

    @Test func sessionStartDoesNotCompleteButAllowsIdleToWaiting() async {
        let store = makeStore()
        await store.process(hook("SessionStart", status: "waiting_for_input") { $0.source = "startup"; $0.sessionTitle = "Refactor" })
        let started = await session(store)
        #expect(started?.phase == .waitingForInput)
        #expect(started?.completedAt == nil)
        #expect(started?.displayTitle == "Refactor")
    }

    @Test func stopFailureNeedsInputUntilNextPrompt() async {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("StopFailure", status: "waiting_for_input") { $0.stopError = "rate_limit" })
        #expect(await session(store)?.attention == .needsInput(.error("Rate limited")))
        await store.process(hook("UserPromptSubmit", status: "processing"))
        #expect(await session(store)?.attention == .working)
    }

    @Test func notificationsSetAndClearNeedsInput() async {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("Notification", status: "notification") { $0.notificationType = "elicitation_dialog"; $0.message = "Pick a repo" })
        #expect(await session(store)?.attention == .needsInput(.elicitation("Pick a repo")))
        await store.process(hook("Notification", status: "notification") { $0.notificationType = "elicitation_complete" })
        #expect(await session(store)?.attention == .working)

        await store.process(hook("Notification", status: "notification") { $0.notificationType = "permission_prompt"; $0.message = "Claude needs your permission to use Bash" })
        #expect(await session(store)?.attention == .needsInput(.permission(tool: "Bash")))
        await store.process(hook("PostToolUse", status: "processing") { $0.tool = "Bash"; $0.toolUseId = "toolu_1" })
        #expect(await session(store)?.attention == .working)
    }

    @Test func parallelApprovalsQueueAndOtherToolsDontHideThem() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") { $0.tool = "Bash"; $0.toolUseId = "toolu_a" })
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") { $0.tool = "Edit"; $0.toolUseId = "toolu_b" })
        #expect(await session(store)?.attention == .needsInput(.permission(tool: "Bash")))

        // An unrelated auto-allowed tool finishing doesn't answer anything.
        await store.process(hook("PostToolUse", status: "processing") { $0.tool = "Read"; $0.toolUseId = "toolu_c" })
        #expect(await session(store)?.activePermission?.toolUseId == "toolu_a")

        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_a"))
        #expect(await session(store)?.activePermission?.toolUseId == "toolu_b")

        // Answered in the terminal: its PostToolUse resolves it.
        await store.process(hook("PostToolUse", status: "processing") { $0.tool = "Edit"; $0.toolUseId = "toolu_b" })
        #expect(await session(store)?.phase == .processing)
    }

    @Test func deadPermissionSocketLeavesApproval() async {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") { $0.tool = "Bash"; $0.toolUseId = "toolu_a" })
        await store.process(.permissionSocketFailed(sessionId: "s1", toolUseId: "toolu_a"))
        let current = await session(store)
        #expect(current?.phase == .processing)
        #expect(current?.activePermission == nil)
    }

    @Test func reviewStateSurvivesRestart() async throws {
        let reviews = ReviewStateStore(fileURL: reviewFile, writeDelay: 0, createsFolder: false)
        let first = SessionStore.forTests(reviewStore: reviews)
        await first.process(hook("UserPromptSubmit", status: "processing"))
        await first.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Shipped" })
        #expect(await first.session(for: "s1")?.attention == .readyForReview)
        reviews.flush()
        #expect(FileManager.default.fileExists(atPath: reviewFile.path))

        // A new app run discovers the session through the registry.
        let second = makeStore()
        let entry = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: "idle", statusUpdatedAt: Date())
        await second.process(.registrySnapshot(configDir: directory.appendingPathComponent(".claude").path, entries: [entry]))
        let restored = try #require(await second.session(for: "s1"))
        #expect(restored.attention == .readyForReview)
        #expect(restored.lastAssistantMessage == "Shipped")
    }

    @Test func registryReconcilesInterruptsAndDialogs() async throws {
        let store = makeStore()
        let configDir = directory.appendingPathComponent(".claude").path
        await store.process(hook("UserPromptSubmit", status: "processing"))

        // Waiting in the registry while we think it's working: a dialog is open.
        let waiting = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", status: "waiting", waitingFor: "input needed", statusUpdatedAt: Date().addingTimeInterval(1))
        await store.process(.registrySnapshot(configDir: configDir, entries: [waiting]))
        #expect(await session(store)?.attention == .needsInput(.dialog("input needed")))

        // Idle without a Stop: interrupted.
        let idle = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", status: "idle", statusUpdatedAt: Date().addingTimeInterval(2))
        await store.process(.registrySnapshot(configDir: configDir, entries: [idle]))
        let interrupted = try #require(await session(store))
        #expect(interrupted.phase == .idle)
        #expect(interrupted.completedAt == nil)

        // Older registry data never overrides newer hook state.
        await store.process(hook("UserPromptSubmit", status: "processing"))
        let stale = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", status: "idle", statusUpdatedAt: Date().addingTimeInterval(-60))
        await store.process(.registrySnapshot(configDir: configDir, entries: [stale]))
        #expect(await session(store)?.phase == .processing)
    }

    @Test func registryCreatesUnknownSessions() async throws {
        let store = makeStore()
        let entry = SessionRegistryEntry(
            pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", entrypoint: "claude-vscode",
            name: "Registry name", status: "busy", statusUpdatedAt: Date()
        )
        await store.process(.registrySnapshot(configDir: "~/.claude-work", entries: [entry]))
        let created = try #require(await session(store))
        #expect(created.phase == .processing)
        #expect(created.entrypoint == "claude-vscode")
        #expect(created.accountId == AccountPaths.normalize("~/.claude-work"))
        #expect(created.displayTitle == "Registry name")

        // A hook title outranks the registry name.
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.sessionTitle = "Hook title" })
        #expect(await session(store)?.displayTitle == "Hook title")
    }

    @Test func statusLineCreatesSessionAndSetsContext() async throws {
        let store = makeStore()
        let json: [String: Any] = [
            "event": "StatusLine", "session_id": "s1", "cwd": "/tmp/proj", "transcript_path": transcript,
            "status_line": ["context_window": ["used_percentage": 64, "context_window_size": 200_000], "model": ["display_name": "Opus"], "cost": ["total_cost_usd": 0.5]],
        ]
        let message = try #require(StatusLineMessage(json: json))
        await store.process(.statusLineReceived(message))
        let created = try #require(await session(store))
        #expect(created.phase == .idle)
        #expect(created.contextUsedPercent == 64)
        #expect(created.contextWindowSize == 200_000)
        #expect(created.model == "Opus")
        #expect(created.costUSD == 0.5)
    }

    @Test func sessionEndRemovesAndBlocksLateStatusLines() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("SessionEnd", status: "ended"))
        #expect(await session(store) == nil)
        let message = try #require(StatusLineMessage(json: ["event": "StatusLine", "session_id": "s1", "status_line": [:]]))
        await store.process(.statusLineReceived(message))
        #expect(await session(store) == nil)
    }
}

/// Builds HookEvents tersely for store tests.
struct HookEventBuilder {
    var event: String
    var status: String
    var transcriptPath: String?
    var sessionId = "s1"
    var cwd = "/tmp/proj"
    var agentId: String?
    var tool: String?
    var toolUseId: String?
    var notificationType: String?
    var message: String?
    var lastAssistantMessage: String?
    var backgroundTaskCount: Int?
    var stopError: String?
    var source: String?
    var sessionTitle: String?
    var trigger: String?

    init(event: String, status: String, transcriptPath: String?) {
        self.event = event
        self.status = status
        self.transcriptPath = transcriptPath
    }

    func build() -> HookEvent {
        HookEvent(
            sessionId: sessionId,
            event: event,
            status: status,
            cwd: cwd,
            transcriptPath: transcriptPath,
            attended: true,
            entrypoint: "cli",
            agentId: agentId,
            tool: tool,
            toolInput: tool == nil ? nil : [:],
            toolUseId: toolUseId,
            notificationType: notificationType,
            message: message,
            lastAssistantMessage: lastAssistantMessage,
            backgroundTaskCount: backgroundTaskCount,
            stopError: stopError,
            source: source,
            sessionTitle: sessionTitle,
            trigger: trigger
        )
    }
}
