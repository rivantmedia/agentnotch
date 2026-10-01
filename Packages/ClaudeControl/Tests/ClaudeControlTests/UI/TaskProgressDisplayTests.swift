import Foundation
import Testing
@testable import ClaudeControl

/// How far a session's tasks are and how long the rest should take, as rows,
/// the chat's task board and VoiceOver put it.
struct TaskProgressDisplayTests {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    /// 2 tasks done at 5 minutes each, "Writing tests" running for 2 minutes, 2 to go.
    private var timedTasks: SessionTaskList {
        SampleSessions.taskList(done: 2, active: "Writing tests", pending: 2, minutesEach: 5, activeFor: 2, now: now)
    }

    private var undatedTasks: SessionTaskList {
        SampleSessions.taskList(done: 2, active: "Writing tests", pending: 2)
    }

    private func session(_ phase: SessionPhase, tasks: SessionTaskList) -> SessionState {
        var session = SessionState(sessionId: "s1", cwd: "/Users/me/code/acme", phase: phase,
                                   lastActivity: now.addingTimeInterval(-60))
        session.tasks = tasks
        session.turnStartedAt = now.addingTimeInterval(-900)
        return session
    }

    private func row(_ session: SessionState) -> SessionRowModel {
        SessionRowModel.make(session, account: nil, rateLimit: nil, canFocus: false, now: now, home: "/Users/me")
    }

    @Test func aWorkingSessionsRowSaysHowFarAndHowLong() throws {
        let row = row(session(.processing, tasks: timedTasks))
        let estimate = try #require(row.taskEstimate)
        // 2 done + 2 of the 5 minutes of the third, of 5.
        #expect(estimate.percent == 48)
        #expect(estimate.activePercent == 40)
        // 3 minutes of the task in progress, then 2 × 5.
        #expect(estimate.remaining == "~13m left")
        #expect(estimate.remainingShort == "~13m")
        #expect(estimate.spokenRemaining == "about 13 minutes left")
        #expect(row.accessibilityLabel.contains("2 of 5 tasks done, 48%, about 13 minutes left, now: Writing tests"))

        #expect(TaskProgressBar.remainingLabel(estimate, style: .full) == "~13m left")
        #expect(TaskProgressBar.remainingLabel(estimate, style: .short) == "~13m")
        #expect(TaskProgressBar.remainingLabel(estimate, style: .hidden) == nil)
        #expect(TaskProgressBar.activeCredit(estimate) == 0.4)
    }

    @Test func noClockRunsForASessionThatIsntWorking() {
        var waiting = session(.waitingForInput, tasks: timedTasks)
        waiting.completedAt = now.addingTimeInterval(-30)
        let row = row(waiting)
        #expect(row.taskEstimate == nil)
        #expect(!row.accessibilityLabel.contains("left"))
        #expect(TaskProgressBar.remainingLabel(row.taskEstimate, style: .full) == nil)
        #expect(TaskProgressBar.activeCredit(row.taskEstimate) == nil)
    }

    @Test func noPaceNoTimeLeft() throws {
        let row = row(session(.processing, tasks: undatedTasks))
        let estimate = try #require(row.taskEstimate)
        #expect(estimate.percent == 40)
        #expect(estimate.remaining == nil)
        #expect(estimate.spokenRemaining == nil)
        // The segment in progress stays solid: nothing to credit it by.
        #expect(TaskProgressBar.activeCredit(estimate) == nil)
        #expect(row.accessibilityLabel.contains("2 of 5 tasks done, now: Writing tests"))
    }

    @Test func taskBoardHeadlineAndTaskTimes() throws {
        let tasks = timedTasks
        let estimate = tasks.timing.estimate(now: now)
        #expect(ChatTaskBoard.headline(tasks, estimate: estimate) == "2 of 5 done · 48% · ~13m left")
        #expect(ChatTaskBoard.headline(tasks, estimate: nil) == "2 of 5 done · 40%")
        #expect(ChatTaskBoard.headline(undatedTasks, estimate: undatedTasks.timing.estimate(now: now)) == "2 of 5 done · 40%")

        let items = tasks.items
        #expect(ChatTaskBoard.time(of: items[0], isRunning: false, now: now) == "5m")
        #expect(ChatTaskBoard.time(of: items[2], isRunning: true, now: now) == "2m")
        // Not working: a running time would only count the wait.
        #expect(ChatTaskBoard.time(of: items[2], isRunning: false, now: now) == nil)
        #expect(ChatTaskBoard.time(of: items[3], isRunning: true, now: now) == nil)
        #expect(ChatTaskBoard.time(of: undatedTasks.items[0], isRunning: true, now: now) == nil)
    }

    @Test func summaryButtonHelp() {
        let tasks = timedTasks
        #expect(ChatTaskSummaryButton.help(tasks, estimate: tasks.timing.estimate(now: now))
            == "Now: Writing tests · 48% · ~13m left")
        #expect(ChatTaskSummaryButton.help(tasks, estimate: nil) == "Now: Writing tests")
        #expect(ChatTaskSummaryButton.help(SampleSessions.taskList(done: 3, active: nil, pending: 0), estimate: nil) == "Tasks")
    }
}
