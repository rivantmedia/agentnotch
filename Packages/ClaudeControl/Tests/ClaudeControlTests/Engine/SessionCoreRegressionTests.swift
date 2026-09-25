import Foundation
import Testing
@testable import ClaudeControl

/// Regression tests for defects found reviewing the session core: compaction
/// mid-turn, stale transcript data at Stop, derived registry names, pids from
/// JSON, background-agent approvals after Stop, interrupt watching of a
/// transcript that doesn't exist yet, and the subagent transcript cache.
struct SessionCoreRegressionTests {
    /// Removed with the suite instance.
    private let account: TemporaryAccount
    private let directory: URL
    private let transcript: String
    private let reviewFile: URL

    init() throws {
        account = try TemporaryAccount(prefix: "spcn-regress")
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

    // MARK: - Compaction

    @Test func automaticCompactionMidTurnKeepsWorkingAndStillCompletes() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "user" })
        await store.process(hook("PreCompact", status: "compacting") { $0.trigger = "auto" })
        await store.process(hook("PostCompact", status: "processing") { $0.trigger = "auto" })
        // Claude Code runs SessionStart(compact) after every compaction.
        await store.process(hook("SessionStart", status: "waiting_for_input") { $0.source = "compact" })
        #expect(await session(store)?.phase == .processing)
        #expect(await session(store)?.attention == .working)

        await store.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Migrated all 40 files." })
        let finished = try #require(await session(store))
        #expect(finished.completedAt != nil)
        #expect(finished.attention == .readyForReview)
    }

    @Test func staleTranscriptPreambleDoesNotHideACompletion() async throws {
        let store = makeStore()
        let preamble = SessionStore.contextResumePrefix + " that ran out of context. The conversation is summarized below."
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "user" })
        try Self.appendUserLine(preamble, to: transcript)
        await store.process(.fileUpdated(Self.payload(transcriptPath: transcript, lastMessage: preamble)))
        #expect(await session(store)?.lastMessage?.hasPrefix(SessionStore.contextResumePrefix) == true)

        // The hook's own final message is what counts.
        await store.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Done: all green." })
        #expect(await session(store)?.attention == .readyForReview)
    }

    @Test func preambleStillSuppressesWithoutAHookMessage() async throws {
        let store = makeStore()
        let preamble = SessionStore.contextResumePrefix + "."
        await store.process(hook("UserPromptSubmit", status: "processing"))
        try Self.appendUserLine(preamble, to: transcript)
        await store.process(.fileUpdated(Self.payload(transcriptPath: transcript, lastMessage: preamble)))
        await store.process(hook("Stop", status: "waiting_for_input"))
        #expect(await session(store)?.completedAt == nil)
    }

    // MARK: - Restored review state

    @Test func promptAfterARestoredCompletionCountsAsReviewed() async throws {
        let reviews = ReviewStateStore(fileURL: reviewFile, writeDelay: 0, createsFolder: false)
        let first = SessionStore.forTests(reviewStore: reviews)
        await first.process(hook("UserPromptSubmit", status: "processing"))
        await first.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Shipped" })
        let completedAt = try #require(await first.session(for: "s1")?.completedAt)
        reviews.flush()

        // Next app run: rediscovered through the registry...
        let second = SessionStore.forTests(reviewFile: reviewFile)
        let configDir = directory.appendingPathComponent(".claude").path
        let entry = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: "idle", statusUpdatedAt: Date())
        await second.process(.registrySnapshot(configDir: configDir, entries: [entry]))
        #expect(await second.session(for: "s1")?.attention == .readyForReview)

        // ...but its transcript shows the user prompted again after that result.
        let promptedAt = completedAt.addingTimeInterval(60)
        await second.process(.fileUpdated(Self.payload(
            transcriptPath: transcript, lastMessage: "and now the docs", lastPromptAt: promptedAt, discovery: true
        )))
        let restored = try #require(await second.session(for: "s1"))
        #expect(restored.attention == .idle)
        #expect(restored.reviewedAt == promptedAt)
    }

    @Test func stopOfASessionFirstSeenMidTurnIsACompletion() async throws {
        let reviews = ReviewStateStore(fileURL: reviewFile, writeDelay: 0, createsFolder: false)
        let first = SessionStore.forTests(reviewStore: reviews)
        await first.process(hook("UserPromptSubmit", status: "processing"))
        await first.process(hook("Stop", status: "waiting_for_input"))
        let oldCompletion = try #require(await first.session(for: "s1")?.completedAt)
        reviews.flush()

        // The app restarts while the user runs another turn; its Stop is the
        // first event the new run sees.
        let second = SessionStore.forTests(reviewFile: reviewFile)
        await second.process(hook("Stop", status: "waiting_for_input") { $0.lastAssistantMessage = "Second result" })
        let finished = try #require(await second.session(for: "s1"))
        let newCompletion = try #require(finished.completedAt)
        #expect(newCompletion > oldCompletion)

        // The prompt of that turn predates its Stop: still waiting for review.
        let promptedAt = oldCompletion.addingTimeInterval(newCompletion.timeIntervalSince(oldCompletion) / 2)
        await second.process(.fileUpdated(Self.payload(
            transcriptPath: transcript, lastMessage: "Second result", lastPromptAt: promptedAt, discovery: true
        )))
        #expect(await second.session(for: "s1")?.attention == .readyForReview)
    }

    @Test func olderPromptLeavesARestoredCompletionForReview() async throws {
        let reviews = ReviewStateStore(fileURL: reviewFile, writeDelay: 0, createsFolder: false)
        let first = SessionStore.forTests(reviewStore: reviews)
        await first.process(hook("UserPromptSubmit", status: "processing"))
        await first.process(hook("Stop", status: "waiting_for_input"))
        let completedAt = try #require(await first.session(for: "s1")?.completedAt)
        reviews.flush()

        let second = SessionStore.forTests(reviewFile: reviewFile)
        let entry = SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: "idle", statusUpdatedAt: Date())
        await second.process(.registrySnapshot(configDir: directory.appendingPathComponent(".claude").path, entries: [entry]))
        await second.process(.fileUpdated(Self.payload(
            transcriptPath: transcript, lastMessage: "fix it", lastPromptAt: completedAt.addingTimeInterval(-30), discovery: true
        )))
        #expect(await second.session(for: "s1")?.attention == .readyForReview)
    }

    // MARK: - Background agents

    @Test func backgroundAgentApprovalAfterStopDoesNotRestartTheTurn() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing") { $0.source = "user" })
        await store.process(hook("Stop", status: "waiting_for_input") { $0.backgroundTaskCount = 1 })
        #expect(await session(store)?.attention == .readyForReview)

        await store.process(hook("PermissionRequest", status: "waiting_for_approval") {
            $0.agentId = "agent-bg"; $0.tool = "Bash"; $0.toolUseId = "toolu_bg1"
        })
        #expect(await session(store)?.attention == .needsInput(.permission(tool: "Bash")))
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_bg1"))
        #expect(await session(store)?.phase == .waitingForInput)

        // Answered in the terminal instead: the agent's PostToolUse resolves it.
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") {
            $0.agentId = "agent-bg"; $0.tool = "Bash"; $0.toolUseId = "toolu_bg2"
        })
        await store.process(hook("PostToolUse", status: "processing") {
            $0.agentId = "agent-bg"; $0.tool = "Bash"; $0.toolUseId = "toolu_bg2"
        })
        let session = try #require(await session(store))
        #expect(session.phase == .waitingForInput)
        #expect(session.attention == .readyForReview)  // still unreviewed; the running agent shows as a detail
    }

    @Test func idleNotificationDoesNotDropAPendingApproval() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") { $0.tool = "Bash"; $0.toolUseId = "toolu_1" })
        await store.process(hook("Notification", status: "waiting_for_input") { $0.notificationType = "idle_prompt" })
        #expect(await session(store)?.activePermission?.toolUseId == "toolu_1")
    }

    @Test func backgroundAgentActivityKeepsAFailedTurnsError() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("StopFailure", status: "waiting_for_input") { $0.stopError = "overloaded" })
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") {
            $0.agentId = "agent-bg"; $0.tool = "Bash"; $0.toolUseId = "toolu_bg"
        })
        #expect(await session(store)?.attention == .needsInput(.permission(tool: "Bash")))
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_bg"))
        await store.process(hook("PostToolUse", status: "processing") {
            $0.agentId = "agent-bg"; $0.tool = "Bash"; $0.toolUseId = "toolu_bg"
        })
        #expect(await session(store)?.attention == .needsInput(.error("Overloaded")))

        // A terminal permission prompt answered for the agent is settled by its activity.
        await store.process(hook("Notification", status: "notification") {
            $0.notificationType = "permission_prompt"; $0.message = "Claude needs your permission to use Edit"
        })
        #expect(await session(store)?.attention == .needsInput(.permission(tool: "Edit")))
        await store.process(hook("PostToolUse", status: "processing") {
            $0.agentId = "agent-bg"; $0.tool = "Edit"; $0.toolUseId = "toolu_bg2"
        })
        #expect(await session(store)?.needsInputReason == nil)
    }

    @Test func mainSessionApprovalStillResumesProcessing() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("PreToolUse", status: "running_tool") { $0.tool = "Bash"; $0.toolUseId = "toolu_1" })
        await store.process(hook("PermissionRequest", status: "waiting_for_approval") { $0.tool = "Bash"; $0.toolUseId = "toolu_1" })
        await store.process(.permissionApproved(sessionId: "s1", toolUseId: "toolu_1"))
        #expect(await session(store)?.phase == .processing)
    }

    // MARK: - Titles

    @Test func derivedRegistryNamesRankBelowTranscriptTitles() {
        var state = SessionState(sessionId: "s1", cwd: "/tmp/proj")
        state.applyName("proj-3", isDerived: true)
        #expect(state.displayTitle == "proj-3")

        // A first prompt says more than a made-up name...
        state.conversationInfo = Self.info(firstUserMessage: "Fix the login flow")
        #expect(state.displayTitle == "Fix the login flow")
        // ...and an AI title replaces it.
        state.applyTitle("Login flow fix", source: .transcript)
        #expect(state.displayTitle == "Login flow fix")
        // The status line repeating the derived name changes nothing.
        state.applyName("proj-3")
        #expect(state.displayTitle == "Login flow fix")
        // A name the user chose wins.
        state.applyName("Auth rewrite")
        #expect(state.displayTitle == "Auth rewrite")
    }

    @Test func derivedNameSeenFirstOnTheStatusLineIsDowngraded() {
        var state = SessionState(sessionId: "s1", cwd: "/tmp/proj")
        state.applyName("proj-3")  // status line, source unknown yet
        #expect(state.titleSource == .registry)
        state.applyName("proj-3", isDerived: true)  // registry: it was derived
        #expect(state.titleSource == .derivedName)
        state.applyTitle("Login flow fix", source: .transcript)
        #expect(state.displayTitle == "Login flow fix")
    }

    @Test func registryEntryParsesNameSource() throws {
        let entry = try #require(SessionRegistryEntry(json: [
            "pid": 123, "sessionId": "s1", "name": "proj-3", "nameSource": "derived", "status": "idle",
        ]))
        #expect(entry.isNameDerived)
        let chosen = try #require(SessionRegistryEntry(json: ["pid": 123, "sessionId": "s1", "name": "Mine"]))
        #expect(!chosen.isNameDerived)
    }

    // MARK: - PIDs from JSON

    @Test func outOfRangePidsAreDroppedInsteadOfTrapping() throws {
        let data = try JSONSerialization.data(withJSONObject: [
            "event": "Stop", "session_id": "s1", "pid": 99_999_999_999,
        ])
        guard case .hook(let event)? = HookSocketMessage.decode(data) else {
            Issue.record("not decoded")
            return
        }
        #expect(event.pid == nil)
        #expect(SessionRegistryEntry(json: ["pid": 99_999_999_999, "sessionId": "s1"]) == nil)
        #expect(SessionRegistryEntry(json: ["pid": -5, "sessionId": "s1"]) == nil)
        #expect(ProcessID.isRunning(Int.max) == false)
        #expect(ProcessID.isRunning(Int(getpid())))
        #expect(SessionRegistryScanner.processStartDate(pid: Int.max) == nil)
    }

    // MARK: - Interrupt watcher

    @Test func interruptWatcherWaitsForTheTranscriptToAppear() async throws {
        let path = directory.appendingPathComponent("late.jsonl").path
        let delegate = InterruptRecorder()
        let watcher = JSONLInterruptWatcher(sessionId: "late", filePath: path)
        watcher.delegate = delegate
        watcher.start()
        defer { watcher.stop() }

        // The transcript shows up after the watcher started (first prompt).
        try await Task.sleep(nanoseconds: 300_000_000)
        FileManager.default.createFile(atPath: path, contents: Data("{\"type\":\"user\"}\n".utf8))
        try await Task.sleep(nanoseconds: 1_500_000_000)

        let handle = try #require(FileHandle(forWritingAtPath: path))
        try handle.seekToEnd()
        handle.write(Data("{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"[Request interrupted by user]\"}]}}\n".utf8))
        try handle.close()

        let deadline = Date().addingTimeInterval(3)
        while delegate.sessionIds.isEmpty && Date() < deadline {
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        #expect(delegate.sessionIds == ["late"])
    }

    // MARK: - Subagent transcripts

    @Test func subagentToolCacheFollowsFileGrowth() async throws {
        let project = directory.appendingPathComponent(".claude/projects/-tmp-proj")
        let main = project.appendingPathComponent("s1.jsonl").path
        let agentFile = project.appendingPathComponent("s1/subagents/agent-a1.jsonl")
        try FileManager.default.createDirectory(at: agentFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        try (try Self.toolUseLine(id: "t1")).write(to: agentFile)

        var transcript = SubagentTranscript(
            agentFile: TranscriptLocator.subagentTranscriptPath(transcriptPath: main, agentId: "a1"))
        #expect(transcript.hasGrown)
        transcript.readNewLines()
        #expect(transcript.tools.map(\.id) == ["t1"])
        #expect(!transcript.hasGrown)

        let handle = try FileHandle(forWritingTo: agentFile)
        try handle.seekToEnd()
        handle.write(try Self.toolUseLine(id: "t2"))
        try handle.close()
        #expect(transcript.hasGrown)
        transcript.readNewLines()
        #expect(transcript.tools.map(\.id) == ["t1", "t2"])
    }

    // MARK: - Helpers

    private static func info(lastMessage: String? = nil, firstUserMessage: String? = nil, lastPromptAt: Date? = nil) -> ConversationInfo {
        var info = ConversationInfo(
            summary: nil, lastMessage: lastMessage, lastMessageRole: lastMessage == nil ? nil : "user",
            lastToolName: nil, firstUserMessage: firstUserMessage, lastUserMessageDate: lastPromptAt
        )
        info.lastTurn.humanPromptAt = lastPromptAt
        return info
    }

    /// `discovery`: the first sync of a session found mid-flight (carries a rebuilt task list).
    private static func payload(transcriptPath: String, lastMessage: String, lastPromptAt: Date? = nil, discovery: Bool = false) -> FileUpdatePayload {
        FileUpdatePayload(
            sessionId: "s1",
            transcriptPath: transcriptPath,
            conversationInfo: info(lastMessage: lastMessage, lastPromptAt: lastPromptAt),
            reconstructedTasks: discovery ? SessionTaskList() : nil
        )
    }

    /// Keeps the transcript consistent with the payload, so a background
    /// file sync of the store reads the same last message.
    private static func appendUserLine(_ text: String, to path: String) throws {
        let line: [String: Any] = ["type": "user", "uuid": UUID().uuidString, "message": ["role": "user", "content": text]]
        let handle = try #require(FileHandle(forWritingAtPath: path))
        try handle.seekToEnd()
        handle.write(try JSONSerialization.data(withJSONObject: line) + Data("\n".utf8))
        try handle.close()
    }

    private static func toolUseLine(id: String) throws -> Data {
        let line: [String: Any] = [
            "type": "assistant", "timestamp": "2026-09-24T10:00:00.000Z",
            "message": ["content": [["type": "tool_use", "id": id, "name": "Read", "input": ["file_path": "/tmp/x"]]]],
        ]
        return try JSONSerialization.data(withJSONObject: line) + Data("\n".utf8)
    }
}

/// Records interrupt callbacks from JSONLInterruptWatcher.
private nonisolated final class InterruptRecorder: JSONLInterruptWatcherDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var ids: [String] = []

    var sessionIds: [String] {
        lock.lock(); defer { lock.unlock() }
        return ids
    }

    func didDetectInterrupt(sessionId: String, at: Date) {
        lock.lock(); ids.append(sessionId); lock.unlock()
    }
}
