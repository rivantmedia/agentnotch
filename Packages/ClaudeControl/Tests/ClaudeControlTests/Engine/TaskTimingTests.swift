import Foundation
import Testing
@testable import ClaudeControl

/// When tasks start and finish, and what the list makes of it: progress with
/// credit for the task in progress, its pace and the time left.
struct TaskTimingTests {
    private let t0 = Date(timeIntervalSince1970: 1_790_000_000)

    private func at(_ seconds: TimeInterval) -> Date { t0.addingTimeInterval(seconds) }

    private func pre(_ tool: String, id: String, input: [String: Any], at seconds: TimeInterval) -> HookEvent {
        HookEvent(
            sessionId: "s1",
            event: "PreToolUse",
            status: "running_tool",
            tool: tool,
            toolInput: input.mapValues { AnyCodable($0) },
            toolUseId: id,
            receivedAt: at(seconds)
        )
    }

    private func postCreate(id: String, taskId: String, at seconds: TimeInterval) -> HookEvent {
        HookEvent(sessionId: "s1", event: "PostToolUse", status: "processing", tool: "TaskCreate",
                  toolUseId: id, taskId: taskId, receivedAt: at(seconds))
    }

    private func update(_ taskId: String, _ status: String, at seconds: TimeInterval) -> HookEvent {
        pre("TaskUpdate", id: "toolu_u\(taskId)_\(status)_\(Int(seconds))", input: ["taskId": taskId, "status": status], at: seconds)
    }

    /// Tasks "1"..."count" created at t0 through hooks.
    private func makeList(count: Int) -> SessionTaskList {
        var list = SessionTaskList()
        for index in 1...count {
            list.apply(pre("TaskCreate", id: "toolu_\(index)", input: ["subject": "Task \(index)"], at: 0))
            list.apply(postCreate(id: "toolu_\(index)", taskId: "\(index)", at: 1))
        }
        return list
    }

    // MARK: Recording

    @Test func hooksDateCreationStartAndCompletion() {
        var list = makeList(count: 2)
        list.apply(update("1", "in_progress", at: 10))
        list.apply(update("1", "in_progress", at: 20))
        list.apply(update("1", "completed", at: 70))

        let first = list.items[0]
        // Created when the call was made, not when its result came back.
        #expect(first.createdAt == at(0))
        // A repeated "in_progress" isn't a new start.
        #expect(first.startedAt == at(10))
        #expect(first.completedAt == at(70))
        #expect(first.duration(now: at(500)) == 60)

        list.apply(update("2", "in_progress", at: 80))
        #expect(list.items[1].duration(now: at(95)) == 15)
        // Back to pending forgets the start.
        list.apply(update("2", "pending", at: 90))
        #expect(list.items[1].startedAt == nil)
        #expect(list.items[1].duration(now: at(95)) == nil)
    }

    @Test func taskHookEventsAreDated() {
        var list = SessionTaskList()
        list.apply(HookEvent(sessionId: "s1", event: "TaskCreated", status: "processing", taskId: "4", taskSubject: "Ship it", receivedAt: at(5)))
        list.apply(HookEvent(sessionId: "s1", event: "TaskCompleted", status: "processing", taskId: "4", receivedAt: at(65)))
        #expect(list.items[0].createdAt == at(5))
        #expect(list.items[0].completedAt == at(65))
        // Never seen in progress: no start, so no duration of its own.
        #expect(list.items[0].startedAt == nil)
        #expect(list.items[0].duration(now: at(100)) == nil)
    }

    @Test func todoWriteKeepsTimesAcrossRewrites() {
        var list = SessionTaskList()
        let todos: (String, String) -> [(content: String, status: String, activeForm: String?)] = { first, second in
            [("Read code", first, nil), ("Fix bug", second, nil), ("Test", "pending", nil)]
        }
        list.todosReplaced(todos("in_progress", "pending"), at: at(0))
        list.todosReplaced(todos("completed", "in_progress"), at: at(120))
        list.todosReplaced(todos("completed", "in_progress"), at: at(150))

        #expect(list.items[0].startedAt == at(0))
        #expect(list.items[0].completedAt == at(120))
        #expect(list.items[1].createdAt == at(0))
        #expect(list.items[1].startedAt == at(120))
        #expect(list.timing.secondsPerTask == 120)
        #expect(list.timing.activeSince == at(120))
    }

    @Test func transcriptTimestampsDateTheRebuiltList() throws {
        func line(_ object: [String: Any]) throws -> Substring {
            Substring(String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self))
        }
        let lines: [Substring] = [
            try line(["type": "assistant", "timestamp": "2026-09-24T10:00:00.000Z", "message": ["content": [
                ["type": "tool_use", "id": "toolu_1", "name": "TaskCreate", "input": ["subject": "Plan"]],
                ["type": "tool_use", "id": "toolu_2", "name": "TaskCreate", "input": ["subject": "Build"]],
            ]]]),
            try line(["type": "user", "timestamp": "2026-09-24T10:00:01.000Z", "message": ["content": [
                ["type": "tool_result", "tool_use_id": "toolu_1", "content": "Task #1 created successfully: Plan"],
                ["type": "tool_result", "tool_use_id": "toolu_2", "content": "Task #2 created successfully: Build"],
            ]]]),
            try line(["type": "assistant", "timestamp": "2026-09-24T10:00:05Z", "message": ["content": [
                ["type": "tool_use", "id": "toolu_3", "name": "TaskUpdate", "input": ["taskId": "1", "status": "in_progress"]],
            ]]]),
            try line(["type": "assistant", "timestamp": "2026-09-24T10:03:05.000Z", "message": ["content": [
                ["type": "tool_use", "id": "toolu_4", "name": "TaskUpdate", "input": ["taskId": "1", "status": "completed"]],
                ["type": "tool_use", "id": "toolu_5", "name": "TaskUpdate", "input": ["taskId": "2", "status": "in_progress"]],
            ]]]),
        ]
        let list = SessionTaskList.reconstruct(fromTranscriptLines: lines)
        let created = try #require(ConversationParser.parseDate("2026-09-24T10:00:00.000Z"))
        #expect(list.items[0].createdAt == created)
        #expect(list.items[0].startedAt == created.addingTimeInterval(5))
        #expect(list.items[0].completedAt == created.addingTimeInterval(185))
        #expect(list.timing.secondsPerTask == 180)
        #expect(list.timing.activeSince == created.addingTimeInterval(185))
    }

    @Test func mergeKeepsTheTranscriptsEarlierTimes() {
        var reconstructed = SessionTaskList()
        reconstructed.taskCreateStarted(toolUseId: "toolu_1", subject: "Design", description: nil, activeForm: nil, at: at(0))
        reconstructed.taskCreateFinished(toolUseId: "toolu_1", taskId: "1", subject: nil, at: at(1))
        reconstructed.taskUpdated(taskId: "1", status: "in_progress", subject: nil, activeForm: nil, at: at(10))

        // The app saw the session only from its completion on.
        var hooks = SessionTaskList()
        hooks.taskUpdated(taskId: "1", status: "completed", subject: nil, activeForm: nil, at: at(130))

        let merged = hooks.merged(intoReconstructed: reconstructed)
        #expect(merged.items[0].subject == "Design")
        #expect(merged.items[0].createdAt == at(0))
        #expect(merged.items[0].startedAt == at(10))
        #expect(merged.items[0].completedAt == at(130))
        #expect(merged.timing.secondsPerTask == 120)
    }

    // MARK: Pace and estimate

    @Test func progressCreditsTheTaskInProgressAgainstThePace() {
        var list = makeList(count: 4)
        list.apply(update("1", "in_progress", at: 0))
        list.apply(update("1", "completed", at: 60))
        list.apply(update("2", "in_progress", at: 60))
        list.apply(update("2", "completed", at: 180))
        list.apply(update("3", "in_progress", at: 180))

        let timing = list.timing
        #expect(timing.completed == 2)
        #expect(timing.total == 4)
        #expect(timing.secondsPerTask == 90)
        #expect(timing.activeSince == at(180))

        let now = at(210)
        // 2 done + 30 s of a 90 s task, of 4.
        #expect(abs(timing.fraction(now: now) - (2 + 1.0 / 3) / 4) < 0.0001)
        #expect(timing.percent(now: now) == 58)
        // The rest of task 3, then task 4.
        #expect(timing.remaining(now: now) == 60.0 + 90)
        #expect(TaskTiming.remainingLabel(timing.remaining(now: now)) == "~3m left")
    }

    @Test func noEstimateUntilATaskIsTimed() {
        var list = makeList(count: 3)
        list.apply(update("1", "in_progress", at: 0))
        let timing = list.timing
        #expect(timing.secondsPerTask == nil)
        #expect(timing.remaining(now: at(600)) == nil)
        // No pace, no credit: only completed tasks count.
        #expect(timing.fraction(now: at(600)) == 0)
        #expect(TaskTiming.remainingLabel(timing.remaining(now: at(600))) == nil)
    }

    @Test func aTaskRunningLongIsAlmostDoneNeverDone() {
        var list = makeList(count: 2)
        list.apply(update("1", "in_progress", at: 0))
        list.apply(update("1", "completed", at: 60))
        list.apply(update("2", "in_progress", at: 60))

        let timing = list.timing
        let late = at(60 + 600)
        #expect(timing.remaining(now: late) == 0)
        #expect(TaskTiming.remainingLabel(timing.remaining(now: late)) == "almost done")
        #expect(abs(timing.fraction(now: late) - 1.9 / 2) < 0.0001)
        #expect(timing.percent(now: late) == 95)

        list.apply(update("2", "completed", at: 700))
        #expect(list.timing.fraction(now: late) == 1)
        #expect(list.timing.percent(now: late) == 100)
        #expect(list.timing.remaining(now: late) == nil)
    }

    @Test func tasksCompletedTogetherCountAsQuickOnes() {
        var list = makeList(count: 4)
        list.apply(update("1", "in_progress", at: 0))
        list.apply(update("1", "completed", at: 120))
        list.apply(update("2", "completed", at: 120))
        list.apply(update("3", "completed", at: 120))
        // 120 s for three tasks.
        #expect(list.timing.secondsPerTask == 40)
        #expect(list.timing.remaining(now: at(120)) == 40)
    }

    @Test func gapsBetweenTasksAreNotCounted() {
        var list = makeList(count: 3)
        list.apply(update("1", "in_progress", at: 0))
        list.apply(update("1", "completed", at: 60))
        // Waited an hour before starting the next one (a turn ended, say).
        list.apply(update("2", "in_progress", at: 3_660))
        list.apply(update("2", "completed", at: 3_720))
        #expect(list.timing.secondsPerTask == 60)
    }

    @Test func oneTaskLeftWaitingCountsAsAtMostFourMedians() {
        var list = makeList(count: 5)
        var clock: TimeInterval = 0
        for (id, seconds) in [("1", 60.0), ("2", 60), ("3", 60), ("4", 3_600)] {
            list.apply(update(id, "in_progress", at: clock))
            clock += seconds
            list.apply(update(id, "completed", at: clock))
        }
        // 60, 60, 60 and 3600 capped at 4 × 60.
        #expect(list.timing.secondsPerTask == (60.0 * 3 + 240) / 4)
    }

    @Test func aFirstTaskNeverStartedIsTimedFromItsCreation() {
        var list = makeList(count: 2)
        list.apply(update("1", "completed", at: 90))
        #expect(list.timing.secondsPerTask == 90)

        // Without any time at all there is nothing to measure.
        var undated = SessionTaskList()
        undated.todosReplaced([("a", "completed", nil), ("b", "in_progress", nil), ("c", "pending", nil)])
        #expect(undated.timing.secondsPerTask == nil)
        #expect(undated.timing.activeSince == nil)
        #expect(abs(undated.timing.fraction(now: at(0)) - 1.0 / 3) < 0.0001)
    }

    // MARK: Labels

    @Test func remainingLabels() {
        #expect(TaskTiming.remainingLabel(nil) == nil)
        #expect(TaskTiming.remainingLabel(.nan) == nil)
        #expect(TaskTiming.remainingLabel(-5) == nil)
        #expect(TaskTiming.remainingLabel(0) == "almost done")
        #expect(TaskTiming.remainingLabel(44) == "almost done")
        #expect(TaskTiming.remainingLabel(45) == "~1m left")
        #expect(TaskTiming.remainingLabel(4 * 60 + 20) == "~4m left")
        #expect(TaskTiming.remainingLabel(59 * 60) == "~59m left")
        #expect(TaskTiming.remainingLabel(60 * 60) == "~1h left")
        // Past an hour, to the nearest 5 minutes; past 10, to the hour.
        #expect(TaskTiming.remainingLabel(83 * 60) == "~1h 25m left")
        #expect(TaskTiming.remainingLabel(11 * 3_600 + 20 * 60) == "~11h left")
        #expect(TaskTiming.remainingShort(4 * 60) == "~4m")
        #expect(TaskTiming.remainingShort(20) == "almost done")
        #expect(TaskTiming.spokenRemaining(60) == "about 1 minute left")
        #expect(TaskTiming.spokenRemaining(4 * 60) == "about 4 minutes left")
        #expect(TaskTiming.spokenRemaining(85 * 60) == "about 1 hour 25 minutes left")
        #expect(TaskTiming.spokenRemaining(2 * 3_600) == "about 2 hours left")
        #expect(TaskTiming.spokenRemaining(10) == "almost done")
    }
}
