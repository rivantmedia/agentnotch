import Foundation
import Testing
@testable import ClaudeControl

/// BHV-1: a cron or teammate that stays in the session doesn't silence the
/// turns the user types there; only a turn that adds one, or a tick, is quiet.
struct Fix_QuietCompletionTests {
    private let account: TemporaryAccount
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-fix-quiet")
        transcript = account.transcript("s1")
    }

    private func makeStore() -> SessionStore {
        SessionStore.forTests(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            effects: .none,
            completionTiming: .immediate
        )
    }

    private func event(_ name: String, status: String, source: String? = nil, prompt: String? = nil,
                       agents: [String]? = nil, crons: Int? = nil) -> SessionEvent {
        .hookReceived(HookEvent(
            sessionId: "s1", event: name, status: status, cwd: "/tmp/proj", transcriptPath: transcript,
            attended: true, entrypoint: "cli",
            backgroundTaskCount: agents?.count, backgroundTaskTypes: agents,
            sessionCronCount: crons, source: source, prompt: prompt, receivedAt: Date()))
    }

    private func turn(_ store: SessionStore, source: String, prompt: String = "go",
                      agents: [String]? = nil, crons: Int? = nil) async throws -> SessionState {
        await store.process(event("UserPromptSubmit", status: "processing", source: source, prompt: prompt))
        await store.process(event("Stop", status: "waiting_for_input", agents: agents, crons: crons))
        let state = try #require(await store.session(for: "s1"))
        #expect(state.attention == .readyForReview)
        return state
    }

    @Test func aTypedTurnInALoopSessionIsAnnounced() async throws {
        let store = makeStore()
        // The user starts a loop: the cron is new in this turn, so it's quiet.
        #expect(try await turn(store, source: "user", prompt: "/loop 5m check the build", crons: 1).completionIsQuiet)
        // A tick: quiet.
        #expect(try await turn(store, source: "schedule_wakeup", crons: 1).completionIsQuiet)
        // A turn the user types while the loop stays: announced.
        #expect(try await turn(store, source: "user", prompt: "fix the lint error", crons: 1).completionIsQuiet == false)
        // Claude schedules another wake-up in a typed turn: paused, quiet.
        #expect(try await turn(store, source: "user", prompt: "check back in 10 min", crons: 2).completionIsQuiet)
        // The loop is stopped: announced.
        #expect(try await turn(store, source: "user", prompt: "stop the loop", crons: 0).completionIsQuiet == false)
    }

    @Test func aTurnInterruptedBeforeItsStopKeepsWhatWasAlreadyThere() async throws {
        let store = makeStore()
        _ = try await turn(store, source: "user", prompt: "/loop 5m poll", crons: 1)
        // Interrupted: a prompt with no Stop, then another.
        await store.process(event("UserPromptSubmit", status: "processing", source: "user", prompt: "no wait"))
        #expect(try await turn(store, source: "user", prompt: "do this instead", crons: 1).completionIsQuiet == false)
    }

    @Test func teammatesThatStayDoNotSilenceTheLead() async throws {
        let store = makeStore()
        // Spawning teammates: they will wake the lead, quiet.
        #expect(try await turn(store, source: "user", agents: ["teammate", "teammate"]).completionIsQuiet)
        // A typed turn while they stay alive: announced.
        #expect(try await turn(store, source: "user", agents: ["teammate", "teammate"]).completionIsQuiet == false)
        // A teammate's report wakes the lead (not typed): quiet while one is out.
        #expect(try await turn(store, source: "system", agents: ["teammate"]).completionIsQuiet)
        // The last one reports: the final turn is announced.
        #expect(try await turn(store, source: "system", agents: []).completionIsQuiet == false)
    }
}
