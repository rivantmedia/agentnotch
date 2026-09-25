import Combine
import Foundation
import Testing
@testable import ClaudeControl

/// The ported session simulator (DevTools/simulate-sessions.py) drives a
/// private socket, pipeline and store with the fork's names: its fake
/// sessions end in the states it announces.
@Suite(.serialized)
@MainActor
final class A1_DevToolsSimulatorTests {
    private let account: TemporaryAccount
    private let socketPath: String

    init() throws {
        account = try TemporaryAccount(prefix: "agentnotch-a1-sim")
        socketPath = "/tmp/agentnotch-a1s-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
    }

    private static let simulator = TestPaths.packageRoot.appendingPathComponent("DevTools/simulate-sessions.py").path
    private static let hook = TestPaths.scripts.appendingPathComponent(ClaudeControlConfiguration.defaultHookScriptName).path

    @Test func scenariosEndInTheStatesTheyAnnounce() async throws {
        let server = HookSocketServer(socketPath: socketPath)
        let store = SessionStore.forTests(reviewFile: account.reviewFile)
        let monitor = ClaudeSessionMonitor(store: store, server: server)
        monitor.startSessionPipeline()
        defer { monitor.stop() }
        let deadline = Date().addingTimeInterval(5)
        while !FileManager.default.fileExists(atPath: socketPath), Date() < deadline {
            try await Task.sleep(nanoseconds: 25_000_000)
        }

        let root = account.root.appendingPathComponent("fake").path
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = [
            "python3", Self.simulator,
            "--socket", socketPath, "--hook", Self.hook, "--root", root,
            "--scenario", "tasks,review,ratelimit",
            "--step", "0.05", "--linger", "1.5", "--quiet", "--no-registry",
        ]
        process.environment = ["PATH": "/usr/bin:/bin"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = output
        try process.run()

        // While it lingers, its sessions are alive; check them then.
        var states: [SessionState] = []
        let checkDeadline = Date().addingTimeInterval(20)
        while Date() < checkDeadline {
            try await Task.sleep(nanoseconds: 200_000_000)
            states = await Self.sessions(store, prefixes: ["c333", "d444", "e555"])
            let done = states.count == 3
                && states.contains { $0.sessionId.hasPrefix("d444") && $0.attention == .readyForReview }
                && states.contains { $0.sessionId.hasPrefix("e555") && $0.attention.isError }
                && states.contains { $0.sessionId.hasPrefix("c333") && $0.tasks.completedCount == 1 }
            if done { break }
        }
        while process.isRunning {
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        let log = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        #expect(process.terminationStatus == 0, "\(log)")

        let tasks = try #require(states.first { $0.sessionId.hasPrefix("c333") })
        #expect(tasks.attention == .working)
        #expect(tasks.tasks.totalCount == 3)
        #expect(tasks.tasks.completedCount == 1)
        #expect(tasks.tasks.activeItem?.activeLabel == "Building the settings form")
        let review = try #require(states.first { $0.sessionId.hasPrefix("d444") })
        #expect(review.attention == .readyForReview)
        #expect(review.lastAssistantMessage?.hasPrefix("Fixed the flaky login test") == true)
        let failed = try #require(states.first { $0.sessionId.hasPrefix("e555") })
        #expect(failed.attention == .needsInput(.error("Rate limited")))
        #expect(failed.accountId == AccountPaths.normalize(root + "/.claude-work"))

        // The given root is kept (only one the simulator made itself goes)...
        #expect(FileManager.default.fileExists(atPath: root))
        // ...but no stand-in process or registry file outlives it.
        let sessionFiles = (try? FileManager.default.contentsOfDirectory(atPath: root + "/.claude-work/sessions")) ?? []
        #expect(sessionFiles.isEmpty)
        for state in states {
            if let pid = state.pid {
                #expect(!ProcessID.isRunning(pid))
            }
        }
    }

    /// The /goal, background-agent and background-workflow scenarios, with
    /// the registry the simulator mirrors read the way the app's scanner
    /// reads it.
    @Test func goalLoopsAreDoneOnceAndAgentRequestsSurviveTheStop() async throws {
        let server = HookSocketServer(socketPath: socketPath)
        let registryOnly = TurnCompletion.Timing(fallbackDelay: 30, registryTimeout: 60, clockTolerance: 1)
        let store = SessionStore.forTests(reviewFile: account.reviewFile, completionTiming: registryOnly)
        let monitor = ClaudeSessionMonitor(store: store, server: server)
        monitor.startSessionPipeline()
        defer { monitor.stop() }
        let deadline = Date().addingTimeInterval(5)
        while !FileManager.default.fileExists(atPath: socketPath), Date() < deadline {
            try await Task.sleep(nanoseconds: 25_000_000)
        }

        // Every published state of the goal and workflow sessions, to count
        // their "done"s.
        let recorder = AttentionRecorder()
        let workflowRecorder = AttentionRecorder()
        let waited = AttentionRecorder()
        let subscription = store.sessionsPublisher.sink { sessions in
            for session in sessions where session.sessionId.hasPrefix("6888") {
                recorder.record(session.attention)
            }
            for session in sessions where session.sessionId.hasPrefix("3bbb") {
                workflowRecorder.record(session.attention)
                if session.isAwaitingBackgroundWork { waited.record(session.attention) }
            }
        }
        defer { subscription.cancel() }

        let root = account.root.appendingPathComponent("fake-goal").path
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = [
            "python3", Self.simulator,
            "--socket", socketPath, "--hook", Self.hook, "--root", root,
            "--scenario", "goal,bgagent,bgworkflow", "--step", "0.2", "--linger", "8", "--quiet",
        ]
        process.environment = ["PATH": "/usr/bin:/bin"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = output
        try process.run()

        var answered = false
        let testDeadline = Date().addingTimeInterval(30)
        while process.isRunning, Date() < testDeadline {
            try await Task.sleep(nanoseconds: 100_000_000)
            for dir in [".claude", ".claude-work"] {
                let configDir = root + "/" + dir
                monitor.enqueue(.registry(configDir: configDir, entries: SessionRegistryScanner.liveEntries(configDir: configDir)))
            }
            let agentSession = await Self.sessions(store, prefixes: ["5999"]).first
            if !answered, let request = agentSession?.activePermission, request.isFromSubagent,
               agentSession?.backgroundWaitSince != nil {
                // The main turn is over (waiting on its agent), and the
                // agent's request is still there.
                #expect(agentSession?.attention == .needsInput(.permission(tool: "Bash")))
                monitor.approvePermission(sessionId: agentSession!.sessionId, toolUseId: request.toolUseId)
                answered = true
            }
            let goal = await Self.sessions(store, prefixes: ["6888"]).first
            let workflow = await Self.sessions(store, prefixes: ["3bbb"]).first
            // The answer reached the hook once the store recorded it.
            if answered, agentSession?.activePermission == nil, goal?.attention == .readyForReview,
               workflow?.attention == .readyForReview {
                try await Task.sleep(nanoseconds: 300_000_000)
                process.terminate()
            }
        }
        while process.isRunning {
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        let log = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        #expect(answered, "\(log)")
        #expect(log.contains("agent permission hook returned: {\"hookSpecificOutput\""), "\(log)")
        // Working the whole time, then done exactly once.
        #expect(recorder.readyForReviewCrossings == 1, "\(recorder.history)")
        #expect(recorder.history.last == .readyForReview)
        // The workflow session waited (working) after its first Stop and was
        // done once, after the workflow's result woke Claude.
        #expect(waited.history == [.working], "\(waited.history)")
        #expect(workflowRecorder.readyForReviewCrossings == 1, "\(workflowRecorder.history)")
        #expect(workflowRecorder.history.last == .readyForReview)
        // The agent session is still waiting on its agent (it never reported).
        #expect(await Self.sessions(store, prefixes: ["5999"]).first?.attention == .working)
    }

    private static func sessions(_ store: SessionStore, prefixes: [String]) async -> [SessionState] {
        var found: [SessionState] = []
        for id in await store.sessionIds() where prefixes.contains(where: { id.hasPrefix($0) }) {
            if let state = await store.session(for: id) {
                found.append(state)
            }
        }
        return found
    }
}

/// Attention values a session went through, in order (duplicates dropped).
private nonisolated final class AttentionRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [SessionAttention] = []

    func record(_ attention: SessionAttention) {
        lock.lock(); defer { lock.unlock() }
        if values.last != attention { values.append(attention) }
    }

    var history: [SessionAttention] {
        lock.lock(); defer { lock.unlock() }
        return values
    }

    var readyForReviewCrossings: Int {
        history.filter { $0 == .readyForReview }.count
    }
}
