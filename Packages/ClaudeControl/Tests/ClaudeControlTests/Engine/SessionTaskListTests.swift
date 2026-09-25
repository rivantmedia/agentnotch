import Foundation
import Testing
@testable import ClaudeControl

struct SessionTaskListTests {
    private func pre(_ tool: String, id: String, input: [String: Any], agentId: String? = nil) -> HookEvent {
        HookEvent(
            sessionId: "s1",
            event: "PreToolUse",
            status: "running_tool",
            agentId: agentId,
            tool: tool,
            toolInput: input.mapValues { AnyCodable($0) },
            toolUseId: id
        )
    }

    private func postCreate(id: String, taskId: String?, subject: String? = nil) -> HookEvent {
        HookEvent(sessionId: "s1", event: "PostToolUse", status: "processing", tool: "TaskCreate", toolUseId: id, taskId: taskId, taskSubject: subject)
    }

    @Test func createIsPendingUntilItsIdIsKnown() {
        var list = SessionTaskList()
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "Write tests", "activeForm": "Writing tests"]))
        #expect(list.totalCount == 0)
        #expect(list.pendingCreates.count == 1)

        list.apply(postCreate(id: "toolu_a", taskId: "1", subject: "Write tests"))
        #expect(list.totalCount == 1)
        #expect(list.pendingCreates.isEmpty)
        #expect(list.items[0].id == "1")
        #expect(list.items[0].activeForm == "Writing tests")
        #expect(list.items[0].status == .pending)
    }

    @Test func duplicateSubjectsResolveByToolUseId() {
        var list = SessionTaskList()
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "Same", "activeForm": "First"]))
        list.apply(pre("TaskCreate", id: "toolu_b", input: ["subject": "Same", "activeForm": "Second"]))
        list.apply(postCreate(id: "toolu_b", taskId: "2"))
        list.apply(postCreate(id: "toolu_a", taskId: "1"))
        #expect(list.items.map(\.id) == ["2", "1"])
        #expect(list.items.first { $0.id == "1" }?.activeForm == "First")
        #expect(list.items.first { $0.id == "2" }?.activeForm == "Second")
    }

    @Test func updatesProgressAndCountsCompletion() {
        var list = SessionTaskList()
        for (index, subject) in ["A", "B", "C"].enumerated() {
            list.apply(pre("TaskCreate", id: "toolu_\(index)", input: ["subject": subject]))
            list.apply(postCreate(id: "toolu_\(index)", taskId: "\(index + 1)"))
        }
        list.apply(pre("TaskUpdate", id: "toolu_u1", input: ["taskId": "1", "status": "completed"]))
        list.apply(pre("TaskUpdate", id: "toolu_u2", input: ["taskId": "2", "status": "in_progress", "activeForm": "Doing B"]))

        #expect(list.completedCount == 1)
        #expect(list.totalCount == 3)
        #expect(list.activeItem?.id == "2")
        #expect(list.activeItem?.activeLabel == "Doing B")
        #expect(abs(list.fraction - 1.0 / 3.0) < 0.0001)
    }

    @Test func deletedRemovesAndUnknownIdCreatesPlaceholder() {
        var list = SessionTaskList()
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "A"]))
        list.apply(postCreate(id: "toolu_a", taskId: "1"))
        list.apply(pre("TaskUpdate", id: "toolu_u", input: ["taskId": "1", "status": "deleted"]))
        #expect(list.isEmpty)

        list.apply(pre("TaskUpdate", id: "toolu_v", input: ["taskId": "7", "status": "in_progress"]))
        #expect(list.items.map(\.subject) == ["Task #7"])
        #expect(list.activeItem?.id == "7")
    }

    @Test func taskHookEventsUpsertAndComplete() {
        var list = SessionTaskList()
        list.apply(HookEvent(sessionId: "s1", event: "TaskCreated", status: "processing", taskId: "4", taskSubject: "Ship it"))
        #expect(list.items.map(\.subject) == ["Ship it"])
        list.apply(HookEvent(sessionId: "s1", event: "TaskCompleted", status: "processing", taskId: "4"))
        #expect(list.completedCount == 1)
    }

    @Test func todoWriteIsTheFallbackList() {
        var list = SessionTaskList()
        let todos: [[String: Any]] = [
            ["content": "Read code", "status": "completed", "activeForm": "Reading code"],
            ["content": "Fix bug", "status": "in_progress", "activeForm": "Fixing bug"],
            ["content": "Test", "status": "pending", "activeForm": "Testing"],
        ]
        list.apply(pre("TodoWrite", id: "toolu_t", input: ["todos": todos]))
        #expect(list.totalCount == 3)
        #expect(list.completedCount == 1)
        #expect(list.activeItem?.activeLabel == "Fixing bug")

        // Task* tasks take precedence once they exist.
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "Real task"]))
        list.apply(postCreate(id: "toolu_a", taskId: "1"))
        #expect(list.items.map(\.subject) == ["Real task"])
    }

    @Test func subagentEventsAreIgnored() {
        var list = SessionTaskList()
        let handled = list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "Sub"], agentId: "agent-1"))
        #expect(!handled)
        #expect(list.pendingCreates.isEmpty)
    }

    @Test func survivesPromptsAndResetsOnClear() {
        var list = SessionTaskList()
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "A"]))
        list.apply(postCreate(id: "toolu_a", taskId: "1"))
        list.apply(HookEvent(sessionId: "s1", event: "UserPromptSubmit", status: "processing", source: "user"))
        #expect(list.totalCount == 1)
        list.apply(HookEvent(sessionId: "s1", event: "SessionStart", status: "waiting_for_input", source: "clear"))
        #expect(list.isEmpty)
    }

    @Test func failedCreateIsDropped() {
        var list = SessionTaskList()
        list.apply(pre("TaskCreate", id: "toolu_a", input: ["subject": "A"]))
        list.apply(HookEvent(sessionId: "s1", event: "PostToolUseFailure", status: "processing", tool: "TaskCreate", toolUseId: "toolu_a"))
        #expect(list.pendingCreates.isEmpty)
        #expect(list.isEmpty)
    }

    @Test func reconstructsFromTranscript() throws {
        func line(_ object: [String: Any]) throws -> Substring {
            Substring(String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self))
        }
        let lines: [Substring] = [
            try line(["type": "assistant", "isSidechain": false, "message": ["content": [
                ["type": "tool_use", "id": "toolu_1", "name": "TaskCreate", "input": ["subject": "Plan", "activeForm": "Planning"]],
                ["type": "tool_use", "id": "toolu_2", "name": "TaskCreate", "input": ["subject": "Build"]],
            ]]]),
            try line(["type": "user", "message": ["content": [
                ["type": "tool_result", "tool_use_id": "toolu_1", "content": "Task #1 created successfully: Plan"],
            ]]]),
            try line(["type": "user", "toolUseResult": ["task": ["id": "2", "subject": "Build"]], "message": ["content": [
                ["type": "tool_result", "tool_use_id": "toolu_2", "content": [["type": "text", "text": "Task #2 created successfully: Build"]]],
            ]]]),
            // Sidechain (subagent) calls don't count.
            try line(["type": "assistant", "isSidechain": true, "message": ["content": [
                ["type": "tool_use", "id": "toolu_9", "name": "TaskUpdate", "input": ["taskId": "2", "status": "completed"]],
            ]]]),
            try line(["type": "assistant", "message": ["content": [
                ["type": "tool_use", "id": "toolu_3", "name": "TaskUpdate", "input": ["taskId": "1", "status": "completed"]],
                ["type": "tool_use", "id": "toolu_4", "name": "TaskUpdate", "input": ["taskId": "2", "status": "in_progress"]],
            ]]]),
        ]
        let list = SessionTaskList.reconstruct(fromTranscriptLines: lines)
        #expect(list.items.map(\.id) == ["1", "2"])
        #expect(list.completedCount == 1)
        #expect(list.activeItem?.subject == "Build")
        #expect(list.items[0].activeForm == "Planning")
    }

    @Test func mergesTranscriptHistoryWithNewerHookState() {
        // Transcript (read late): tasks 1 and 2 created, 1 in progress; create 3 pending.
        var reconstructed = SessionTaskList()
        reconstructed.taskCreateStarted(toolUseId: "toolu_1", subject: "Design", description: nil, activeForm: "Designing")
        reconstructed.taskCreateFinished(toolUseId: "toolu_1", taskId: "1", subject: nil)
        reconstructed.taskCreateStarted(toolUseId: "toolu_2", subject: "Build", description: nil, activeForm: "Building")
        reconstructed.taskCreateFinished(toolUseId: "toolu_2", taskId: "2", subject: nil)
        reconstructed.taskCreateStarted(toolUseId: "toolu_3", subject: "Test", description: nil, activeForm: nil)
        reconstructed.taskUpdated(taskId: "1", status: "in_progress", subject: nil, activeForm: nil)

        // Hooks since the app saw the session: 3 resolved, 1 completed, 2 started
        // (known only by id), a deleted task.
        var hooks = SessionTaskList()
        hooks.taskCreateFinished(toolUseId: "toolu_3", taskId: "3", subject: "Test")
        hooks.taskUpdated(taskId: "1", status: "completed", subject: nil, activeForm: nil)
        hooks.taskUpdated(taskId: "2", status: "in_progress", subject: nil, activeForm: nil)
        hooks.taskUpdated(taskId: "9", status: "deleted", subject: nil, activeForm: nil)

        let merged = hooks.merged(intoReconstructed: reconstructed)
        #expect(merged.items.map(\.id) == ["1", "2", "3"])
        #expect(merged.items.map(\.subject) == ["Design", "Build", "Test"])
        #expect(merged.completedCount == 1)
        #expect(merged.activeItem?.activeLabel == "Building")
        #expect(merged.pendingCreates.isEmpty)
    }

    @Test func createdResultTextIsParsed() {
        #expect(SessionTaskList.createdTaskId(in: "Task #12 created successfully: Do it") == "12")
        #expect(SessionTaskList.createdTaskId(in: "Task 12 created successfully") == nil)
        #expect(SessionTaskList.createdTaskId(in: "nope") == nil)
    }
}
