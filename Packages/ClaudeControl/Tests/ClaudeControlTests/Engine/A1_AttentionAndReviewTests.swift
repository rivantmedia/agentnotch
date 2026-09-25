import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// Launch baseline and quiet completions (AttentionTracker), the review
/// file (heartbeat, legacy shape, failures), the open-chat histories, the
/// task list's batches and the lenient status line numbers.
@MainActor
struct A1_AttentionAndReviewTests {
    // MARK: - AttentionTracker

    private func session(_ id: String, attention: SessionAttention, completedAt: Date? = nil, agents: Int = 0) -> SessionState {
        var state = SessionState(sessionId: id, cwd: "/tmp/\(id)")
        switch attention {
        case .working:
            state.phase = .processing
        case .readyForReview:
            state.phase = .waitingForInput
            state.completedAt = completedAt ?? Date()
            state.backgroundAgentCount = agents
        case .needsInput(let reason):
            state.phase = .processing
            state.needsInputReason = reason
        case .idle:
            state.phase = .idle
        }
        return state
    }

    private func collect(_ tracker: AttentionTracker) -> (AnyCancellable, () -> [AttentionTransition]) {
        var received: [AttentionTransition] = []
        let cancellable = tracker.transitions.sink { received.append($0) }
        return (cancellable, { received })
    }

    @Test func accountsFoundAtLaunchDoNotAlert() {
        let launch = Date()
        let tracker = AttentionTracker(launchedAt: launch)
        let (subscription, received) = collect(tracker)
        defer { subscription.cancel() }

        // Account A's registry, then account B's, each its own snapshot.
        tracker.update([session("a1", attention: .idle)], now: launch)
        tracker.update([
            session("a1", attention: .idle),
            session("b1", attention: .needsInput(.dialog("permission prompt"))),
            session("b2", attention: .readyForReview, completedAt: launch.addingTimeInterval(-86_400)),
        ], now: launch.addingTimeInterval(0.5))
        tracker.initialScanCompleted(now: launch.addingTimeInterval(1))
        // Late first syncs within the settle window are still baseline.
        tracker.update([
            session("a1", attention: .readyForReview, completedAt: launch.addingTimeInterval(-60)),
            session("b1", attention: .needsInput(.dialog("permission prompt"))),
            session("b2", attention: .readyForReview, completedAt: launch.addingTimeInterval(-86_400)),
        ], now: launch.addingTimeInterval(1.5))
        #expect(received().isEmpty)

        // After the baseline, a real change is news.
        let later = launch.addingTimeInterval(10)
        tracker.update([
            session("a1", attention: .readyForReview, completedAt: launch.addingTimeInterval(-60)),
            session("b1", attention: .working),
            session("b2", attention: .readyForReview, completedAt: launch.addingTimeInterval(-86_400)),
            session("c1", attention: .needsInput(.question)),
        ], now: later)
        #expect(received().map(\.session.sessionId) == ["b1", "c1"] || received().map(\.session.sessionId) == ["c1", "b1"])
    }

    @Test func completionsFromBeforeLaunchAndQuietOnesAreSilent() {
        let launch = Date(timeIntervalSinceNow: -100)
        let tracker = AttentionTracker(launchedAt: launch)
        tracker.initialScanCompleted(now: launch)
        let (subscription, received) = collect(tracker)
        defer { subscription.cancel() }
        let now = Date()

        tracker.update([session("inferred", attention: .idle), session("quiet", attention: .working), session("done", attention: .working)], now: now)
        tracker.update([
            // Found finished on its first sync: it finished while the app was down.
            session("inferred", attention: .readyForReview, completedAt: launch.addingTimeInterval(-30)),
            // Ended waiting for its agents: they will wake it.
            session("quiet", attention: .readyForReview, completedAt: now, agents: 2),
            session("done", attention: .readyForReview, completedAt: now),
        ], now: now)
        #expect(received().filter(\.becameReadyForReview).map(\.session.sessionId) == ["done"])
    }

    @Test func failuresAreFlagged() {
        let failure = AttentionTransition(session: SessionState(sessionId: "f", cwd: "/tmp"), from: .working, to: .needsInput(.error("Rate limited")))
        let question = AttentionTransition(session: SessionState(sessionId: "q", cwd: "/tmp"), from: .working, to: .needsInput(.question))
        #expect(failure.isFailure)
        #expect(!question.isFailure)
        #expect(NeedsInputReason.error("x").sortRank > NeedsInputReason.question.sortRank)
        #expect(!NeedsInputReason.error("x").isActionable)
        #expect(StopErrorKind(code: "rate_limit")?.isTransient == true)
        #expect(StopErrorKind(code: "billing_error")?.isTransient == false)
        #expect(StopErrorKind(code: "oauth_org_not_allowed") == .authentication)
        #expect(NeedsInputReason.humanizedStopError("unknown") == "Turn failed")
        #expect(NeedsInputReason.humanizedStopError("new_kind_of_error") == "New kind of error")
    }

    // MARK: - Review file

    private func temporaryFile() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("spcn-a1-review-\(UUID().uuidString).json")
    }

    @Test func readsSuperpoweredVibeNotchsFileAndWritesTheNewShape() throws {
        let url = temporaryFile()
        defer { try? FileManager.default.removeItem(at: url) }
        let now = Date().timeIntervalSince1970
        let legacy = "{\"s1\":{\"completedAt\":\(now - 60),\"lastAssistantMessage\":\"Shipped\",\"updatedAt\":\(now - 60)}}"
        try Data(legacy.utf8).write(to: url)

        let store = ReviewStateStore(fileURL: url, writeDelay: 0, createsFolder: false)
        #expect(store.record(for: "s1")?.lastAssistantMessage == "Shipped")
        #expect(store.previousRunAliveAt == nil)
        store.stopHeartbeat()

        let json = try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        #expect(json["version"] as? Int == 2)
        #expect(json["lastAliveAt"] != nil)
        #expect((json["sessions"] as? [String: Any])?["s1"] != nil)
        #expect(ReviewStateStore(fileURL: url, createsFolder: false).previousRunAliveAt != nil)
    }

    @Test func failuresPersistWithTheirTime() throws {
        let url = temporaryFile()
        defer { try? FileManager.default.removeItem(at: url) }
        let failedAt = Date(timeIntervalSince1970: 1_800_000_000)
        let store = ReviewStateStore(fileURL: url, writeDelay: 60, createsFolder: false)
        store.update(sessionId: "s1", record: ReviewRecord(stopError: "Rate limited", stopErrorCode: "rate_limit", failedAt: failedAt, updatedAt: Date()), urgent: true)
        store.flush()
        let reloaded = ReviewStateStore(fileURL: url, createsFolder: false).record(for: "s1")
        #expect(reloaded?.stopError == "Rate limited")
        #expect(reloaded?.stopErrorCode == "rate_limit")
        #expect(reloaded?.failedAt == failedAt)
        // Previews of Claude's replies are the owner's alone.
        let mode = try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? NSNumber
        #expect(mode?.intValue == 0o600)

        // Nothing left to keep: the record goes.
        store.update(sessionId: "s1", record: ReviewRecord(updatedAt: Date()), urgent: true)
        store.flush()
        #expect(ReviewStateStore(fileURL: url, createsFolder: false).record(for: "s1") == nil)
    }

    // MARK: - Open chat histories

    @Test func onlyOpenChatsKeepHistoriesAndTheOldestIsReleased() async throws {
        let account = try TemporaryAccount(prefix: "spcn-a1-chat")
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        let monitor = ClaudeSessionMonitor(store: store, server: HookSocketServer(socketPath: "/tmp/spcn-a1-unused.sock"))
        let manager = ChatHistoryManager(monitor: monitor)
        var sessions: [SessionState] = []
        for index in 0..<3 {
            var state = SessionState(sessionId: "c\(index)", cwd: "/tmp/proj")
            state.chatItems = [ChatHistoryItem(id: "c\(index)-text-0", type: .assistant("hello \(index)"), timestamp: Date())]
            sessions.append(state)
        }
        await store.replaceAllWithFixtures(sessions)
        // Sealed-style fixtures have no transcript; opening still keeps them.
        try await Task.sleep(nanoseconds: 100_000_000)

        await manager.loadFromFile(sessionId: "c0", cwd: "/tmp/proj")
        await manager.loadFromFile(sessionId: "c1", cwd: "/tmp/proj")
        #expect(manager.histories.keys.sorted() == ["c0", "c1"])
        await manager.loadFromFile(sessionId: "c2", cwd: "/tmp/proj")
        #expect(manager.histories.keys.sorted() == ["c1", "c2"])
        #expect(!manager.isLoaded(sessionId: "c0"))
        #expect(manager.history(for: "c2").count == 1)

        manager.chatClosed(sessionId: "c1")
        #expect(manager.histories.keys.sorted() == ["c2"])
    }

    // MARK: - Task batches

    @Test func aNewBatchStartsAfterEveryTaskIsDone() {
        var list = SessionTaskList()
        for id in 1...3 {
            list.taskCreateStarted(toolUseId: "t\(id)", subject: "Task \(id)", description: nil, activeForm: nil)
            list.taskCreateFinished(toolUseId: "t\(id)", taskId: "\(id)", subject: nil)
        }
        for id in 1...3 {
            list.taskUpdated(taskId: "\(id)", status: "completed", subject: nil, activeForm: nil)
        }
        #expect(list.completedCount == 3)  // the review row still reads 3/3

        list.taskCreateStarted(toolUseId: "t4", subject: "Task 4", description: nil, activeForm: nil)
        list.taskCreateFinished(toolUseId: "t4", taskId: "4", subject: nil)
        list.taskCreated(taskId: "5", subject: "Task 5")
        #expect(list.totalCount == 2)
        #expect(list.completedCount == 0)

        // Unfinished lists keep growing.
        list.taskCreated(taskId: "6", subject: "Task 6")
        #expect(list.totalCount == 3)
    }

    @Test func reconstructionSeesTheSameBatches() throws {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("spcn-a1-tasks-\(UUID().uuidString).jsonl").path
        FileManager.default.createFile(atPath: path, contents: Data())
        defer { try? FileManager.default.removeItem(atPath: path) }
        var lines: [[String: Any]] = []
        for id in 1...2 {
            lines.append(TranscriptLines.toolUse(id: "c\(id)", name: "TaskCreate", input: ["subject": "Old \(id)"]))
            lines.append(TranscriptLines.toolResult(id: "c\(id)", text: "Task #\(id) created successfully: Old \(id)"))
            lines.append(TranscriptLines.toolUse(id: "u\(id)", name: "TaskUpdate", input: ["taskId": "\(id)", "status": "completed"]))
        }
        lines.append(TranscriptLines.toolUse(id: "c3", name: "TaskCreate", input: ["subject": "New"]))
        lines.append(TranscriptLines.toolResult(id: "c3", text: "Task #3 created successfully: New"))
        try TranscriptLines.append(lines, to: path)

        let list = try #require(SessionTaskList.reconstruct(fromTranscriptAt: path))
        #expect(list.items.map(\.subject) == ["New"])
    }

    // MARK: - Status line numbers

    @Test func statusLineRejectsBooleansAndNaN() throws {
        let json: [String: Any] = [
            "event": "StatusLine", "session_id": "sl",
            "status_line": [
                "rate_limits": [
                    "five_hour": ["used_percentage": true, "resets_at": 1_800_000_000],
                    "seven_day": ["used_percentage": "nan", "resets_at": 1_800_000_000],
                ],
                "context_window": ["used_percentage": false],
            ],
        ]
        let message = try #require(StatusLineMessage(json: json))
        #expect(message.update.fiveHour == nil)
        #expect(message.update.sevenDay == nil)
        #expect(message.update.contextUsedPercent == nil)

        let valid = try #require(StatusLineMessage(json: [
            "event": "StatusLine", "session_id": "sl",
            "status_line": ["rate_limits": ["five_hour": ["used_percentage": 42.5, "resets_at": 1_800_000_000]]],
        ]))
        #expect(valid.update.fiveHour?.utilization == 42.5)
        #expect(valid.update.fiveHour?.resetsAt == Date(timeIntervalSince1970: 1_800_000_000))
    }

    // MARK: - Permission previews

    @Test func previewsSayWhenTheyHideSomething() {
        let long = String(repeating: "a", count: 150) + " && git push --force"
        let bash = PermissionContext(toolUseId: "t", toolName: "Bash", toolInput: ["command": AnyCodable(long)], receivedAt: Date())
        #expect(bash.formattedInput?.count == PermissionContext.previewLength + 3)
        #expect(bash.fullInput == long)
        #expect(bash.isPreviewTruncated)

        let edit = PermissionContext(toolUseId: "e", toolName: "Edit", toolInput: ["file_path": AnyCodable("/etc/hosts")], receivedAt: Date())
        #expect(edit.formattedInput == "hosts")
        #expect(edit.fullInput == "/etc/hosts")
        #expect(edit.isPreviewTruncated)

        let short = PermissionContext(toolUseId: "s", toolName: "Bash", toolInput: ["command": AnyCodable("ls")], receivedAt: Date())
        #expect(!short.isPreviewTruncated)

        let rule: [String: Any] = ["type": "addRules", "destination": "userSettings", "rules": []]
        let mode: [String: Any] = ["type": "setMode", "mode": "acceptEdits", "destination": "session"]
        let session: [String: Any] = ["type": "addRules", "destination": "session", "rules": []]
        func suggestion(_ raw: [String: Any]) -> PermissionSuggestion? {
            PermissionContext(toolUseId: "x", toolName: "Bash", toolInput: [:], receivedAt: Date(), permissionSuggestions: [AnyCodable(raw)]).alwaysAllowSuggestion
        }
        #expect(suggestion(rule)?.isNarrow == false)
        #expect(suggestion(mode)?.changesPermissionMode == true)
        #expect(suggestion(session)?.isNarrow == true)
    }
}
