import Foundation
import Testing
@testable import ClaudeControl

/// What the panel and its chat know about a session: task progress of
/// sessions no hook reports, the chat's status line, and an open chat that
/// disappears and comes back.
struct ChatTrackingTests {
    private let account: TemporaryAccount

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-chat-tracking")
    }

    // MARK: - Hookless task progress

    private func create(_ id: String, _ taskId: String, _ subject: String, activeForm: String? = nil) -> [[String: Any]] {
        var input: [String: Any] = ["subject": subject]
        if let activeForm { input["activeForm"] = activeForm }
        return [
            TranscriptLines.toolUse(id: id, name: "TaskCreate", input: input),
            TranscriptLines.toolResult(id: id, text: "Task #\(taskId) created successfully: \(subject)"),
        ]
    }

    private func update(_ id: String, _ taskId: String, _ status: String) -> [String: Any] {
        TranscriptLines.toolUse(id: id, name: "TaskUpdate", input: ["taskId": taskId, "status": status])
    }

    private func registryEntry(status: String, at date: Date = Date()) -> SessionRegistryEntry {
        SessionRegistryEntry(pid: Int(getpid()), sessionId: "h1", cwd: "/tmp/proj", status: status, statusUpdatedAt: date)
    }

    private func waitFor(_ store: SessionStore, _ condition: (SessionState) -> Bool) async throws -> SessionState? {
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if let session = await store.session(for: "h1"), condition(session) { return session }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        return await store.session(for: "h1")
    }

    @Test func aSessionNoHookReportsFollowsItsTranscriptsTasks() async throws {
        let path = account.transcript("h1")
        try TranscriptLines.append(create("c1", "1", "Plan", activeForm: "Planning"), to: path)
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        let configDir = account.configDir.path
        await store.process(.registrySnapshot(configDir: configDir, entries: [registryEntry(status: "busy")]))

        let first = try #require(try await waitFor(store, { $0.tasks.totalCount == 1 }))
        #expect(!first.isHookBacked)
        #expect(first.tasks.completedCount == 0)

        // Later: the first task is done and a second appears. Nothing but the
        // transcript says so.
        // (The second is created before the first is completed: a task added
        // once the whole list is done starts a new list.)
        try TranscriptLines.append(create("c2", "2", "Build", activeForm: "Building")
                                   + [update("u1", "1", "completed"), update("u2", "2", "in_progress")], to: path)
        await store.recheckAllSessions()
        let later = try #require(try await waitFor(store, { $0.tasks.totalCount == 2 }))
        #expect(later.tasks.completedCount == 1)
        #expect(later.tasks.activeItem?.subject == "Build")
    }

    @Test func aClearedSessionStartsItsTaskListOver() async throws {
        let path = account.transcript("h1")
        try TranscriptLines.append(create("c1", "1", "Old one") + create("c2", "2", "Old two"), to: path)
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        await store.process(.registrySnapshot(configDir: account.configDir.path, entries: [registryEntry(status: "busy")]))
        _ = try #require(try await waitFor(store, { $0.tasks.totalCount == 2 }))

        try TranscriptLines.append(
            [TranscriptLines.user("<command-name>/clear</command-name>")] + create("c3", "1", "Fresh"), to: path)
        await store.recheckAllSessions()
        let cleared = try #require(try await waitFor(store, { $0.tasks.items.map(\.subject) == ["Fresh"] }))
        #expect(cleared.tasks.items.map(\.subject) == ["Fresh"])
    }

    @Test func hooksKeepTheirTaskListWhenTheTranscriptIsRead() async throws {
        let path = account.transcript("h1")
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        func hook(_ event: String, status: String, tool: String? = nil, id: String? = nil,
                  input: [String: Any] = [:], taskId: String? = nil, subject: String? = nil) -> SessionEvent {
            .hookReceived(HookEvent(sessionId: "h1", event: event, status: status, cwd: "/tmp/proj", transcriptPath: path,
                                    tool: tool, toolInput: input.mapValues { AnyCodable($0) }, toolUseId: id,
                                    taskId: taskId, taskSubject: subject))
        }
        await store.process(hook("UserPromptSubmit", status: "processing"))
        await store.process(hook("PreToolUse", status: "running_tool", tool: "TaskCreate", id: "t1", input: ["subject": "Plan"]))
        await store.process(hook("PostToolUse", status: "processing", tool: "TaskCreate", id: "t1", taskId: "1", subject: "Plan"))
        await store.process(hook("PreToolUse", status: "running_tool", tool: "TaskUpdate", id: "t2",
                                 input: ["taskId": "1", "status": "completed"]))
        let before = try #require(await store.session(for: "h1"))
        #expect(before.isHookBacked)
        #expect(before.tasks.completedCount == 1)

        // A transcript read that still has the task pending (written late).
        var stale = SessionTaskList()
        stale.taskCreateStarted(toolUseId: "x", subject: "Plan", description: nil, activeForm: nil)
        stale.taskCreateFinished(toolUseId: "x", taskId: "1", subject: nil)
        let payload = FileUpdatePayload(
            sessionId: "h1", transcriptPath: path, conversationInfo: TranscriptSummary().info, transcriptTasks: stale
        )
        await store.process(.fileUpdated(payload))
        let after = try #require(await store.session(for: "h1"))
        #expect(after.tasks == before.tasks)
        #expect(after.tasks.completedCount == 1)
    }

    // MARK: - The chat's status line

    private let now = Date()

    private func finished(_ text: String? = "Done.") -> SessionState {
        var session = SessionState(sessionId: "s", cwd: "/tmp/proj")
        session.phase = .waitingForInput
        session.completedAt = now.addingTimeInterval(-300)
        session.lastAssistantMessage = text
        return session
    }

    @Test func statusLineSaysWhatTheRowSaysForAFailedTurn() throws {
        var session = finished()
        session.needsInputReason = .error(NeedsInputReason.humanizedStopError("rate_limit"))
        let reset = RateLimitReset(window: "5-hour limit", resetsAt: now.addingTimeInterval(47 * 60))

        let line = try #require(ChatStatusLine.make(for: session, rateLimit: reset, now: now))
        let row = SessionRowContent.detail(for: session, rateLimit: reset, now: now)
        #expect(line.text == row.plainText)
        #expect(line.text.contains("5-hour limit"))
        #expect(line.glyph == .error)
        #expect(line.canDismiss)

        let plain = try #require(ChatStatusLine.make(for: session, rateLimit: nil, now: now))
        #expect(plain.text == NeedsInputReason.humanizedStopError("rate_limit"))
    }

    @Test func statusLineForReviewAndIdle() throws {
        let review = try #require(ChatStatusLine.make(for: finished(), rateLimit: nil, now: now))
        #expect(review.glyph == .review)
        #expect(review.text == "Ready for review · finished 5m ago")
        #expect(!review.canDismiss)

        var reviewed = finished()
        reviewed.reviewedAt = now.addingTimeInterval(-60)
        reviewed.lastActivity = now.addingTimeInterval(-3_600)
        let idle = try #require(ChatStatusLine.make(for: reviewed, rateLimit: nil, now: now))
        #expect(idle.glyph == .idle)
        #expect(idle.text == "Idle · last active 1h ago")
    }

    @Test func statusLineIsNotShownWhileWorkingOrWaitingOnAnAnswer() {
        var working = SessionState(sessionId: "s", cwd: "/tmp/proj")
        working.phase = .processing
        #expect(ChatStatusLine.make(for: working, rateLimit: nil, now: now) == nil)

        var dialog = working
        dialog.needsInputReason = .dialog("worker permission")
        #expect(ChatStatusLine.make(for: dialog, rateLimit: nil, now: now) == nil)

        var permission = SessionState(sessionId: "s", cwd: "/tmp/proj")
        permission.phase = .waitingForApproval(PermissionContext(toolUseId: "t", toolName: "Bash",
                                                                 toolInput: ["command": AnyCodable("ls")], receivedAt: now))
        #expect(ChatStatusLine.make(for: permission, rateLimit: nil, now: now) == nil)
    }

    // MARK: - A chat that goes away and comes back

    @MainActor
    @Test func aChatOpenedAgainFollowsTheSessionAgain() async throws {
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        let monitor = ClaudeSessionMonitor(store: store, server: HookSocketServer(socketPath: "/tmp/agentnotch-chat-tracking-unused.sock"))
        let manager = ChatHistoryManager(monitor: monitor)
        func session(_ texts: [String]) -> SessionState {
            var state = SessionState(sessionId: "c0", cwd: "/tmp/proj")
            // One change per message, as the store makes them: the chat
            // rebuilds when the session's `chatRevision` moves.
            for (index, text) in texts.enumerated() {
                state.chatItems.append(ChatHistoryItem(id: "c0-\(index)", type: .assistant(text), timestamp: Date()))
            }
            return state
        }
        await store.replaceAllWithFixtures([session(["one"])])
        try await Task.sleep(nanoseconds: 100_000_000)

        await manager.loadFromFile(sessionId: "c0", cwd: "/tmp/proj")
        #expect(manager.history(for: "c0").count == 1)

        // The view disappears (the panel closes), then appears again and
        // registers itself once more, as the chat's task does.
        manager.chatClosed(sessionId: "c0")
        #expect(manager.history(for: "c0").isEmpty)
        // Closed and not registered again: nothing follows the session (the
        // frozen chat the view's task must not allow).
        await store.replaceAllWithFixtures([session(["one", "x"])])
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(manager.history(for: "c0").isEmpty)
        await manager.loadFromFile(sessionId: "c0", cwd: "/tmp/proj")
        #expect(manager.history(for: "c0").count == 2)
        // Registering again while open changes nothing.
        await manager.loadFromFile(sessionId: "c0", cwd: "/tmp/proj")
        #expect(manager.history(for: "c0").count == 2)

        await store.replaceAllWithFixtures([session(["one", "two", "three"])])
        for _ in 0..<80 where manager.history(for: "c0").count < 3 {
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        #expect(manager.history(for: "c0").count == 3)
    }
}
