import Foundation
import Testing
@testable import ClaudeControl

/// A turn that ends while agents or workflows it started still run isn't
/// done: they wake Claude when they finish. The session shows as working
/// ("Waiting on 1 workflow") until Claude's next Stop says nothing is left,
/// or the registry shows no agent left, and only then is it announced.
///
/// The registry follows Claude Code 2.1.x: "busy" from the start of the
/// turn for as long as the turn or an awaited agent runs (a status that
/// doesn't change isn't rewritten), then "shell" or "idle".
@Suite(.serialized)
struct BackgroundWaitTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-bg-wait")
        transcript = account.transcript("s1")
    }

    private func makeStore(registryGrace: TimeInterval = 10, reviews: ReviewStateStore? = nil) -> SessionStore {
        SessionStore.forTests(
            reviewStore: reviews ?? ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none,
            completionTiming: .immediate,
            backgroundWaitTiming: BackgroundWork.WaitTiming(registryGrace: registryGrace, quietTimeout: 30 * 60)
        )
    }

    private func hook(_ name: String, status: String, prompt: String? = nil, background: [String]? = nil,
                      notificationType: String? = nil, agentId: String? = nil, tool: String? = nil,
                      stopError: String? = nil, at date: Date = Date()) -> SessionEvent {
        .hookReceived(HookEvent(
            sessionId: "s1", event: name, status: status, cwd: "/tmp/proj", pid: Int(getpid()),
            transcriptPath: transcript, attended: true, entrypoint: "cli", agentId: agentId, tool: tool,
            notificationType: notificationType,
            backgroundTaskCount: background?.count, backgroundTaskTypes: background,
            stopError: stopError, prompt: prompt, receivedAt: date))
    }

    private func registry(_ status: String, at date: Date) -> SessionEvent {
        .registrySnapshot(configDir: account.configDir.path, entries: [
            SessionRegistryEntry(pid: Int(getpid()), sessionId: "s1", cwd: "/tmp/proj", status: status, statusUpdatedAt: date),
        ])
    }

    private func state(_ store: SessionStore) async throws -> SessionState {
        try #require(await store.session(for: "s1"))
    }

    /// A typed prompt, the registry going busy with it, then a Stop.
    private func turn(_ store: SessionStore, prompt: String = "go", background: [String]) async {
        let start = Date()
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: prompt, at: start))
        await store.process(registry("busy", at: start))
        await store.process(hook("Stop", status: "waiting_for_input", background: background))
    }

    private static let wake = "<task-notification>\n<task-id>w1</task-id>\n<status>completed</status>\n<summary>Dynamic workflow \"sweep\" completed</summary>\n</task-notification>"

    // MARK: - The wait

    @Test func aTurnWaitingOnAWorkflowIsWorkingUntilItsResultWakesClaude() async throws {
        let store = makeStore()
        await turn(store, prompt: "sweep the repo", background: ["workflow", "shell"])
        var session = try await state(store)
        // Busy registry: the Stop isn't confirmed yet, and the wait is on.
        #expect(session.completionPendingSince != nil)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription == "1 workflow")

        // A minute later Claude Code says it waits for input: that confirms
        // the Stop, but the workflow still runs.
        await store.process(hook("Notification", status: "waiting_for_input", notificationType: "idle_prompt"))
        session = try await state(store)
        #expect(session.completionPendingSince == nil)
        #expect(session.completedAt != nil)
        #expect(session.phase == .waitingForInput)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription == "1 workflow")

        // The workflow's own agents keep working after the Stop.
        await store.process(hook("PreToolUse", status: "running_tool", agentId: "a1", tool: "Grep"))
        #expect(try await state(store).isAwaitingBackgroundWork)

        // Its result wakes Claude: a turn of its own (the wait stands until
        // that turn's Stop says what is left).
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: Self.wake))
        session = try await state(store)
        #expect(session.attention == .working)
        #expect(session.isAwaitingBackgroundWork == false)
        #expect(session.backgroundWaitDescription == nil)

        await store.process(hook("Stop", status: "waiting_for_input", background: ["shell"]))
        await store.process(registry("shell", at: Date()))
        session = try await state(store)
        #expect(session.attention == .readyForReview)
        #expect(session.backgroundWaitSince == nil)
        #expect(session.completionIsQuiet == false)
        #expect(session.backgroundTaskCount == 1)
    }

    /// The SDK (VS Code) may wake Claude without a UserPromptSubmit: the next
    /// Stop still ends the wait, and the completion is that Stop's.
    @Test func aWakeWithoutAPromptStillEndsTheWait() async throws {
        let store = makeStore()
        let start = Date()
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: "go", at: start))
        await store.process(hook("Stop", status: "waiting_for_input", background: ["subagent"], at: start.addingTimeInterval(1)))
        #expect(try await state(store).attention == .working)

        await store.process(hook("Stop", status: "waiting_for_input", background: [], at: start.addingTimeInterval(30)))
        let done = try await state(store)
        #expect(done.attention == .readyForReview)
        #expect(done.completedAt.map { $0 >= start.addingTimeInterval(30) } == true)
    }

    /// A follow-up typed while a workflow runs is answered, but the session's
    /// work isn't done until the workflow is.
    @Test func aTypedTurnDuringTheWaitKeepsWaiting() async throws {
        let store = makeStore()
        await turn(store, prompt: "sweep the repo", background: ["workflow"])
        await turn(store, prompt: "what does foo() do?", background: ["workflow"])
        let session = try await state(store)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription == "1 workflow")
        // Announced once the workflow's result has been dealt with.
        #expect(session.completionIsQuiet == false)
    }

    /// Esc during a turn doesn't stop background agents: the wait stands.
    @Test func anInterruptedTurnKeepsWaitingOnAgentsStillRunning() async throws {
        let store = makeStore(registryGrace: 0)
        await turn(store, background: ["workflow", "workflow"])
        // The first workflow's result wakes Claude; the user presses Esc.
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: Self.wake))
        await store.process(.interruptDetected(sessionId: "s1", at: Date()))
        var session = try await state(store)
        #expect(session.phase == .idle)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription != nil)

        // The second one is stopped too, without a report: the registry
        // shows nothing left.
        await store.process(registry("idle", at: Date()))
        session = try await state(store)
        #expect(session.attention == .readyForReview)
        #expect(session.backgroundWaitSince == nil)
    }

    /// A failed turn (the API refused) doesn't stop the agents either.
    @Test func aFailedTurnKeepsWaitingBehindItsError() async throws {
        let store = makeStore()
        await turn(store, background: ["workflow"])
        await store.process(hook("StopFailure", status: "waiting_for_input", stopError: "rate_limit"))
        var session = try await state(store)
        #expect(session.attention == .needsInput(.error(NeedsInputReason.humanizedStopError("rate_limit"))))
        #expect(session.backgroundWaitSince != nil)

        await store.process(.dismissFailure(sessionId: "s1"))
        session = try await state(store)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription == "1 workflow")
    }

    // MARK: - Ending without a wake-up

    /// Agents stopped (or a report that never came): the registry shows no
    /// agent left, and after a short grace the work is done and announced.
    @Test func theRegistryEndsAWaitNoWakeUpEnds() async throws {
        let store = makeStore(registryGrace: 0)
        await turn(store, background: ["subagent", "subagent", "shell"])
        #expect(try await state(store).attention == .working)
        #expect(try await state(store).backgroundWaitDescription == "2 background agents")

        // Only the shell is left.
        await store.process(registry("shell", at: Date()))
        let session = try await state(store)
        #expect(session.phase == .waitingForInput)
        #expect(session.attention == .readyForReview)
        #expect(session.backgroundAgentCount == 0)
        #expect(session.backgroundTaskCount == 1)
        #expect(session.completionIsQuiet == false)
    }

    @Test func theRegistryWaitsOutItsGrace() async throws {
        let store = makeStore(registryGrace: 1)
        await turn(store, background: ["workflow"])
        await store.process(registry("idle", at: Date()))
        #expect(try await state(store).attention == .working)
        var ended = false
        for _ in 0..<50 where !ended {
            try await Task.sleep(for: .milliseconds(100))
            ended = try await state(store).attention == .readyForReview
        }
        #expect(ended)
    }

    /// Idle teammates don't keep the registry busy: a lead's typed turn is
    /// announced after the grace, not held until the teammates report.
    @Test func idleTeammatesEndTheWaitThroughTheRegistry() async throws {
        let store = makeStore(registryGrace: 0)
        await turn(store, prompt: "spawn a team", background: ["teammate", "teammate"])
        await store.process(registry("busy", at: Date().addingTimeInterval(-0.5)))
        #expect(try await state(store).attention == .working)
        await store.process(registry("idle", at: Date()))
        let session = try await state(store)
        #expect(session.attention == .readyForReview)
        #expect(session.completionIsQuiet == false)
    }

    @Test func shellsMonitorsAndHousekeepingDoNotHoldTheSession() async throws {
        let store = makeStore()
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: "start the dev server"))
        await store.process(hook("Stop", status: "waiting_for_input", background: ["shell", "monitor", "dream", "MCP task", "auto-mode scan"]))
        let session = try await state(store)
        #expect(session.attention == .readyForReview)
        #expect(session.backgroundWaitSince == nil)
        #expect(session.backgroundTaskCount == 5)
    }

    /// The app quit and came back while a workflow ran.
    @Test func theWaitSurvivesARelaunch() async throws {
        let reviews = ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false)
        let first = makeStore(reviews: reviews)
        await turn(first, background: ["workflow"])
        await first.process(hook("Notification", status: "waiting_for_input", notificationType: "idle_prompt"))
        reviews.flush()

        let second = makeStore(registryGrace: 0, reviews: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false))
        await second.process(registry("busy", at: Date().addingTimeInterval(-60)))
        var session = try await state(second)
        #expect(session.attention == .working)
        #expect(session.backgroundWaitDescription == "1 workflow")

        await second.process(registry("idle", at: Date()))
        session = try await state(second)
        #expect(session.attention == .readyForReview)
    }

    // MARK: - What the rows say

    @Test func theRowsSayWhatTheSessionWaitsOn() async throws {
        let store = makeStore()
        await turn(store, background: ["workflow", "subagent", "teammate"])
        let session = try await state(store)
        let summary = ClaudeHostProjections.session(session, home: "/Users/me")
        #expect(summary.attention == .working)
        #expect(summary.backgroundWait == "1 workflow, 1 background agent and 1 teammate")
        let row = ClaudeHostProjections.activityRow(summary)
        #expect(row.state == .busy)
        #expect(row.detail.hasPrefix("Waiting on 1 workflow, 1 background agent and 1 teammate"))
    }

    /// Woken for a turn of its own, the row shows that turn's progress.
    @Test func aWokenTurnShowsItsOwnProgress() async throws {
        let store = makeStore()
        await turn(store, background: ["workflow"])
        await store.process(hook("UserPromptSubmit", status: "processing", prompt: Self.wake))
        let session = try await state(store)
        let summary = ClaudeHostProjections.session(session, home: "/Users/me")
        #expect(summary.backgroundWait == nil)
        #expect(ClaudeHostProjections.activityRow(summary).detail.hasPrefix("Thinking…"))
    }

    // MARK: - Pure parts

    @Test func awaitedTypes() {
        for type in ["subagent", "workflow", "teammate", "cloud session", "local_agent", "local_workflow", "in_process_teammate", "remote_agent"] {
            #expect(BackgroundWork.awaitedTypes.contains(type), "\(type)")
        }
        for type in ["shell", "monitor", "MCP task", "dream", "auto-mode scan", "local_bash", "monitor_mcp", "mcp_task"] {
            #expect(!BackgroundWork.awaitedTypes.contains(type), "\(type)")
        }
    }

    @Test func phrases() {
        #expect(BackgroundWork.phrase(types: []) == nil)
        #expect(BackgroundWork.phrase(types: ["workflow"]) == "1 workflow")
        #expect(BackgroundWork.phrase(types: ["local_workflow", "workflow"]) == "2 workflows")
        #expect(BackgroundWork.phrase(types: ["subagent", "cloud session", "local_agent"]) == "3 background agents")
        #expect(BackgroundWork.phrase(types: ["workflow", "subagent"]) == "1 workflow and 1 background agent")
    }

    @Test func decisions() {
        let stop = Date(timeIntervalSince1970: 1000)
        let timing = BackgroundWork.WaitTiming(registryGrace: 10, quietTimeout: 600)
        func decide(_ status: String?, changed: Date?, hook: Date? = nil, now: Date) -> BackgroundWork.WaitDecision {
            BackgroundWork.decide(waitSince: stop, registryStatus: status, registryChangedAt: changed,
                                  lastHookEventAt: hook, now: now, timing: timing)
        }
        // Idle or shell: after the grace, counted from the Stop at the earliest.
        #expect(decide("idle", changed: stop.addingTimeInterval(2), now: stop.addingTimeInterval(5)) == .keep(recheckIn: 7))
        #expect(decide("shell", changed: stop.addingTimeInterval(2), now: stop.addingTimeInterval(12)) == .end(at: stop.addingTimeInterval(2)))
        #expect(decide("idle", changed: stop.addingTimeInterval(-30), now: stop.addingTimeInterval(4)) == .keep(recheckIn: 6))
        // Busy (or a dialog): until nothing, agents included, has been heard
        // from for a long time (a paused workflow keeps the registry busy).
        #expect(decide("busy", changed: stop.addingTimeInterval(-60), hook: stop.addingTimeInterval(100),
                       now: stop.addingTimeInterval(650)) == .keep(recheckIn: 50))
        #expect(decide("waiting", changed: stop.addingTimeInterval(5), hook: stop.addingTimeInterval(100),
                       now: stop.addingTimeInterval(700)) == .end(at: stop.addingTimeInterval(100)))
        // No registry: the same silence.
        #expect(decide(nil, changed: nil, hook: stop.addingTimeInterval(-5), now: stop.addingTimeInterval(599)) == .keep(recheckIn: 1))
        #expect(decide(nil, changed: nil, now: stop.addingTimeInterval(600)) == .end(at: stop))
    }

    @Test func attentionDerivation() {
        let done = Date()
        #expect(SessionAttention.derive(phase: .waitingForInput, needsInputReason: nil, backgroundTaskCount: 1,
                                        completedAt: done, reviewedAt: nil, awaitingBackgroundAgents: true) == .working)
        #expect(SessionAttention.derive(phase: .idle, needsInputReason: nil, backgroundTaskCount: 1,
                                        completedAt: done, reviewedAt: nil, awaitingBackgroundAgents: true) == .working)
        #expect(SessionAttention.derive(phase: .waitingForInput, needsInputReason: nil, backgroundTaskCount: 1,
                                        completedAt: done, reviewedAt: nil) == .readyForReview)
        // A background agent's question still comes first.
        #expect(SessionAttention.derive(phase: .waitingForInput, needsInputReason: .dialog("permission prompt"),
                                        backgroundTaskCount: 1, completedAt: done, reviewedAt: nil,
                                        awaitingBackgroundAgents: true).bucket == .needsInput)
    }
}
