import Foundation
import Testing
@testable import ClaudeControl

/// The transcript parser after the single-pass rewrite: what one sync
/// returns, what it keeps, how agents are followed and settled (the
/// resync CPU burn), and the summary's view of how a turn ended.
struct A1_TranscriptParserTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "spcn-a1-parser")
        transcript = account.transcript("p1")
    }

    private func request(wanted: Set<String> = [], full: Bool = false) -> ConversationParser.SyncRequest {
        ConversationParser.SyncRequest(sessionId: "p1", transcriptPath: transcript, keepsFullHistory: full, wantedToolIds: wanted)
    }

    // MARK: - One pass

    @Test func oneSyncFeedsSummaryTasksAndMessages() async throws {
        let now = Date()
        try TranscriptLines.append([
            ["type": "ai-title", "aiTitle": "Build the thing"],
            TranscriptLines.user("please build it", at: now.addingTimeInterval(-10)),
            TranscriptLines.toolUse(id: "t1", name: "TaskCreate", input: ["subject": "Design", "activeForm": "Designing"], at: now.addingTimeInterval(-9)),
            TranscriptLines.toolResult(id: "t1", text: "Task #1 created successfully: Design", at: now.addingTimeInterval(-8), toolUseResult: ["task": ["id": "1", "subject": "Design"]]),
            TranscriptLines.assistantText("Done designing.", at: now.addingTimeInterval(-7)),
        ], to: transcript)

        let parser = ConversationParser()
        let result = try #require(await parser.sync(request()))
        #expect(result.advanced)
        #expect(result.conversationInfo.title == "Build the thing")
        #expect(result.transcriptTasks.items.map(\.subject) == ["Design"])
        #expect(result.newMessages.count == 3)
        #expect(result.completedToolIds == ["t1"])
        #expect(await parser.statistics.linesDecoded == 5)

        // Nothing appended: nothing read, nothing to apply.
        let again = try #require(await parser.sync(request()))
        #expect(again.isEmpty)
        #expect(await parser.statistics.linesDecoded == 5)
    }

    @Test func aSessionWhoseChatIsClosedGetsOnlyTheNewestMessages() async throws {
        var lines: [[String: Any]] = []
        for index in 0..<200 {
            lines.append(TranscriptLines.toolUse(id: "tool-\(index)", name: "Read", input: ["file_path": "/tmp/f\(index)"]))
            lines.append(TranscriptLines.toolResult(id: "tool-\(index)", text: String(repeating: "x", count: 2_000), toolUseResult: ["file": ["filePath": "/tmp/f\(index)", "content": "c"]]))
        }
        try TranscriptLines.append(lines, to: transcript)

        let parser = ConversationParser()
        let result = try #require(await parser.sync(request(wanted: ["tool-3"])))
        #expect(result.newMessages.count == ConversationParser.retainedMessageCount)
        // Results only for the retained calls and the ones the store waits for.
        #expect(result.toolResults.count <= ConversationParser.retainedMessageCount + 1)
        #expect(result.toolResults["tool-3"] != nil)
        #expect(result.toolResults["tool-199"] != nil)
        #expect(result.toolResults["tool-50"] == nil)

        let full = try #require(await parser.fullHistory(sessionId: "p1", transcriptPath: transcript))
        #expect(full.messages.count == 200)  // result lines carry no message blocks
        #expect(full.toolResults.count == 200)
    }

    @Test func partialLinesWaitForTheirNewline() async throws {
        let first = try JSONSerialization.data(withJSONObject: TranscriptLines.assistantText("one"))
        let second = try JSONSerialization.data(withJSONObject: TranscriptLines.assistantText("two"))
        try (first + Data("\n".utf8) + second.prefix(20)).write(to: URL(fileURLWithPath: transcript))

        let parser = ConversationParser()
        #expect(try #require(await parser.sync(request())).newMessages.count == 1)
        try (first + Data("\n".utf8) + second + Data("\n".utf8)).write(to: URL(fileURLWithPath: transcript))
        let rest = try #require(await parser.sync(request()))
        #expect(rest.newMessages.count == 1)
        #expect(rest.conversationInfo.lastTurn.replyText == "two")
    }

    @Test func aRewrittenTranscriptIsReadAgainAndReplacesHistory() async throws {
        try TranscriptLines.append((0..<5).map { TranscriptLines.assistantText("old \($0)") }, to: transcript)
        let parser = ConversationParser()
        _ = await parser.sync(request())
        try (try JSONSerialization.data(withJSONObject: TranscriptLines.assistantText("new")) + Data("\n".utf8))
            .write(to: URL(fileURLWithPath: transcript))
        let result = try #require(await parser.sync(request()))
        #expect(result.didReset)
        #expect(result.newMessages.count == 1)
        #expect(result.conversationInfo.lastTurn.replyText == "new")
    }

    @Test func clearAfterTheFirstReadIsReported() async throws {
        try TranscriptLines.append([TranscriptLines.assistantText("before")], to: transcript)
        let parser = ConversationParser()
        _ = await parser.sync(request())
        try TranscriptLines.append([
            TranscriptLines.user("<command-name>/clear</command-name>"),
            TranscriptLines.assistantText("after"),
        ], to: transcript)
        let result = try #require(await parser.sync(request()))
        #expect(result.clearDetected)
        #expect(result.newMessages.map(\.textContent) == ["after"])
    }

    @Test func timestampsParseWithAndWithoutFractionalSeconds() {
        #expect(ConversationParser.parseDate("2026-09-24T10:00:00.123Z") != nil)
        #expect(ConversationParser.parseDate("2026-09-24T10:00:00Z") != nil)
        #expect(ConversationParser.parseDate("yesterday") == nil)
    }

    // MARK: - Agents (the resync CPU burn)

    private func agentFile(_ agentId: String) -> String {
        account.project.appendingPathComponent("p1/subagents/agent-\(agentId).jsonl").path
    }

    private func writeAgent(_ agentId: String, tools: Int, completed: Bool = true) throws {
        let path = agentFile(agentId)
        try FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: path, contents: Data())
        var lines: [[String: Any]] = []
        for index in 0..<tools {
            lines.append(TranscriptLines.toolUse(id: "\(agentId)-\(index)", name: "Grep", input: ["pattern": "p\(index)"]))
            if completed || index < tools - 1 {
                lines.append(TranscriptLines.toolResult(id: "\(agentId)-\(index)"))
            }
        }
        try TranscriptLines.append(lines, to: path)
    }

    private func agentCall(_ toolUseId: String, agentId: String, status: String = "completed") -> [[String: Any]] {
        [
            TranscriptLines.toolUse(id: toolUseId, name: "Agent", input: ["description": "look around"]),
            TranscriptLines.toolResult(id: toolUseId, text: "found it", toolUseResult: ["agentId": agentId, "status": status, "content": "found it"]),
        ]
    }

    @Test func finishedAgentsAreReadOnceNotOnEverySync() async throws {
        var lines: [[String: Any]] = []
        for index in 0..<40 {
            try writeAgent("a\(index)", tools: 20)
            lines += agentCall("call-\(index)", agentId: "a\(index)")
        }
        try TranscriptLines.append(lines, to: transcript)

        let parser = ConversationParser()
        let first = try #require(await parser.sync(request(full: true)))
        #expect(first.subagentTools.count == 40)
        #expect(first.subagentTools["call-7"]?.count == 20)
        #expect(first.subagentTools["call-7"]?.allSatisfy(\.isCompleted) == true)
        #expect(first.subagentTools["call-7"]?.first?.timestamp != nil)
        let reads = await parser.statistics.subagentFileReads
        #expect(reads == 40)
        #expect(await parser.followedAgentToolIds(sessionId: "p1").isEmpty)

        // The periodic resync of a working session: nothing new anywhere.
        for _ in 0..<10 {
            let idle = try #require(await parser.sync(request(full: true)))
            #expect(idle.isEmpty)
        }
        #expect(await parser.statistics.subagentFileReads == reads)

        // One more agent: only it is read.
        try writeAgent("late", tools: 3)
        try TranscriptLines.append(agentCall("call-late", agentId: "late"), to: transcript)
        let next = try #require(await parser.sync(request(full: true)))
        #expect(Set(next.subagentTools.keys) == ["call-late"])
        #expect(await parser.statistics.subagentFileReads == reads + 1)
    }

    @Test func aRunningBackgroundAgentIsFollowedUntilItSettles() async throws {
        try writeAgent("bg", tools: 2, completed: false)
        try TranscriptLines.append(agentCall("call-bg", agentId: "bg", status: "async_launched"), to: transcript)

        let parser = ConversationParser()
        let first = try #require(await parser.sync(request(full: true)))
        #expect(first.subagentTools["call-bg"]?.map(\.isCompleted) == [true, false])
        #expect(await parser.followedAgentToolIds(sessionId: "p1") == ["call-bg"])

        // Unchanged file: a stat, no read, nothing to apply.
        let reads = await parser.statistics.subagentFileReads
        #expect(try #require(await parser.sync(request(full: true))).isEmpty)
        #expect(await parser.statistics.subagentFileReads == reads)

        // It finishes its last tool: only the change is read and reported.
        try TranscriptLines.append([TranscriptLines.toolResult(id: "bg-1")], to: agentFile("bg"))
        let update = try #require(await parser.sync(request(full: true)))
        #expect(update.subagentTools["call-bg"]?.map(\.isCompleted) == [true, true])
        #expect(await parser.statistics.subagentFileReads == reads + 1)
    }

    @Test func subagentTranscriptsAreIncremental() async throws {
        try writeAgent("solo", tools: 2)
        var agent = SubagentTranscript(agentFile: TranscriptLocator.subagentTranscriptPath(transcriptPath: transcript, agentId: "solo"))
        let first = agent.readNewLines()
        #expect(first)
        #expect(agent.tools.count == 2)
        // Nothing appended: nothing to read.
        #expect(!agent.hasGrown)
        let second = agent.readNewLines()
        #expect(!second)
        #expect(agent.tools.count == 2)
    }

    // MARK: - How a turn ended

    private func summary(_ lines: [[String: Any]]) -> TranscriptSummary {
        var summary = TranscriptSummary()
        for line in lines {
            summary.consume(line, dateParser: ConversationParser.parseDate)
        }
        return summary
    }

    @Test func aTurnEndsWithClaudesReply() throws {
        let t0 = Date(timeIntervalSince1970: 1_800_000_000)
        let turn = summary([
            TranscriptLines.user("do it", at: t0),
            TranscriptLines.toolUse(id: "x", name: "Bash", at: t0.addingTimeInterval(1)),
            TranscriptLines.toolResult(id: "x", at: t0.addingTimeInterval(2)),
            TranscriptLines.assistantText("All done.", at: t0.addingTimeInterval(3)),
        ]).turn
        #expect(turn.endsWithReply)
        #expect(turn.replyText == "All done.")
        #expect(turn.humanPromptAt == t0)
        #expect(turn.finishedTurn(after: t0.addingTimeInterval(1))?.at == t0.addingTimeInterval(3))
        #expect(turn.finishedTurn(after: t0.addingTimeInterval(5)) == nil)
    }

    @Test func interruptsToolCallsAndWakeUpsAreNotAFinishedTurn() {
        let t0 = Date(timeIntervalSince1970: 1_800_000_000)
        let reply = TranscriptLines.assistantText("Working on it", at: t0)
        #expect(!summary([reply, TranscriptLines.user("[Request interrupted by user]", at: t0.addingTimeInterval(1))]).turn.endsWithReply)
        #expect(!summary([reply, TranscriptLines.toolUse(id: "y", name: "Read", at: t0.addingTimeInterval(1))]).turn.endsWithReply)
        let wakeUp = TranscriptLines.user("<task-notification>agent done</task-notification>", at: t0.addingTimeInterval(1),
                                          extra: ["origin": ["kind": "task-notification"]])
        #expect(!summary([reply, wakeUp]).turn.endsWithReply)
    }

    @Test func onlyWordsAPersonTypedCountAsPrompts() {
        let t0 = Date(timeIntervalSince1970: 1_800_000_000)
        let notification = TranscriptLines.user("<task-notification>x</task-notification>", at: t0,
                                                extra: ["origin": ["kind": "task-notification"]])
        let oldNotification = TranscriptLines.user("<task-notification>y</task-notification>", at: t0)
        let compact = TranscriptLines.user(SessionStore.contextResumePrefix + " ...", at: t0, extra: ["isCompactSummary": true])
        let continuation = TranscriptLines.user("continue", at: t0, extra: ["origin": ["kind": "auto-continuation"]])
        let human = TranscriptLines.user("fix the tests", at: t0.addingTimeInterval(5), extra: ["origin": ["kind": "human"]])

        for entry in [notification, oldNotification, compact, continuation] {
            #expect(!TranscriptSummary.isHumanPrompt(entry))
        }
        #expect(TranscriptSummary.isHumanPrompt(human))
        #expect(TranscriptSummary.isHumanPrompt(TranscriptLines.user("plain old prompt")))

        let result = summary([notification, compact, continuation])
        #expect(result.lastUserMessageDate == nil)
        #expect(result.firstUserMessage == nil)
        #expect(result.lastMessage == nil)
        let withHuman = summary([notification, human])
        #expect(withHuman.lastUserMessageDate == t0.addingTimeInterval(5))
        #expect(withHuman.lastMessage == "fix the tests")
    }

    // MARK: - Flattening

    @Test func toolInputFlattensTheSameOnBothPaths() throws {
        let raw: [String: Any] = ["run_in_background": true, "timeout": 30, "ratio": 0.5, "command": "ls", "list": [1]]
        let viaJSON = try #require(try JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: raw)) as? [String: Any])
        let fromTranscript = ToolInput.flatten(viaJSON)
        #expect(fromTranscript == ["run_in_background": "true", "timeout": "30", "ratio": "0.5", "command": "ls"])

        let decoded = try JSONDecoder().decode([String: AnyCodable].self, from: JSONSerialization.data(withJSONObject: raw))
        #expect(ToolInput.flatten(decoded.mapValues(\.value)) == fromTranscript)
    }
}
