import Foundation
import Testing
@testable import ClaudeControl

/// End to end: the real hook script as the app installs it (written from
/// EmbeddedScripts with a private socket path filled in) → a private socket
/// server → the ordered pipeline → a private store, answered through a
/// session monitor. Plus the pipeline's throughput while the main thread
/// is busy.
@Suite(.serialized)
@MainActor
final class A1_HookPipelineIntegrationTests {
    private let account: TemporaryAccount
    private let socketPath: String
    private let scriptPath: String
    private let server: HookSocketServer
    private let store: SessionStore
    private let monitor: ClaudeSessionMonitor
    private let transcript: String

    init() throws {
        account = try TemporaryAccount(prefix: "spcn-a1-hook")
        socketPath = "/tmp/spcn-a1h-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
        scriptPath = account.root.appendingPathComponent("hooks/\(ClaudeControlConfiguration.defaultHookScriptName)").path
        try FileManager.default.createDirectory(atPath: (scriptPath as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        // Exactly what the installer writes into <configDir>/hooks/.
        try EmbeddedScripts.hook(socketPath: socketPath).write(toFile: scriptPath, atomically: true, encoding: .utf8)
        transcript = account.transcript("int-1")
        server = HookSocketServer(socketPath: socketPath)
        store = SessionStore.forTests(reviewFile: account.reviewFile)
        monitor = ClaudeSessionMonitor(store: store, server: server)
    }

    deinit {
        server.stop()
    }

    private func start() async throws {
        monitor.startSessionPipeline()
        #expect(try await eventually { FileManager.default.fileExists(atPath: self.socketPath) })
    }

    private func eventually(timeout: TimeInterval = 10, _ condition: () async -> Bool) async throws -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while await !condition() {
            guard Date() < deadline else { return false }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
        return true
    }

    /// Runs the installed script with Claude Code's hook environment. The
    /// socket path baked into the script is used (no SPCN_SOCKET).
    private func runHook(_ payload: [String: Any]) throws -> (Process, Pipe) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", scriptPath]
        process.environment = [
            "PATH": "/usr/bin:/bin",
            "CLAUDE_PID": String(getpid()),
            "CLAUDE_CODE_SESSION_ATTENDED": "1",
            "CLAUDE_CODE_ENTRYPOINT": "cli",
            "CLAUDE_CONFIG_DIR": account.configDir.path,
        ]
        let stdin = Pipe()
        let stdout = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = FileHandle.nullDevice
        try process.run()
        stdin.fileHandleForWriting.write(try JSONSerialization.data(withJSONObject: payload))
        try stdin.fileHandleForWriting.close()
        return (process, stdout)
    }

    /// Runs a fire-and-forget hook to completion.
    private func send(_ event: String, _ fields: [String: Any] = [:]) async throws {
        let (process, _) = try runHook(payload(event, fields))
        #expect(try await exited(process))
    }

    /// Waits for a hook to exit without blocking the main actor (answers
    /// are sent from main-actor tasks).
    private func exited(_ process: Process, timeout: TimeInterval = 10) async throws -> Bool {
        try await eventually(timeout: timeout) { !process.isRunning }
    }

    private func payload(_ event: String, _ fields: [String: Any]) -> [String: Any] {
        var data: [String: Any] = [
            "hook_event_name": event,
            "session_id": "int-1",
            "transcript_path": transcript,
            "cwd": "/tmp/proj",
            "permission_mode": "default",
        ]
        data.merge(fields) { _, new in new }
        return data
    }

    private func session() async -> SessionState? {
        await store.session(for: "int-1")
    }

    private func decision(_ stdout: Pipe) throws -> [String: Any]? {
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        guard !data.isEmpty else { return nil }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        return (json?["hookSpecificOutput"] as? [String: Any])?["decision"] as? [String: Any]
    }

    // MARK: - Background agents

    @Test func aBackgroundAgentsRequestSurvivesTheMainStopAndIsAnswered() async throws {
        try await start()
        try await send("UserPromptSubmit", ["prompt": "run the agents", "source": "user"])
        let input: [String: Any] = ["command": "npm test"]
        try await send("PreToolUse", ["agent_id": "bg-1", "agent_type": "general-purpose", "tool_name": "Bash", "tool_input": input, "tool_use_id": "toolu_bg"])
        let (agentHook, agentOut) = try runHook(payload("PermissionRequest", ["agent_id": "bg-1", "tool_name": "Bash", "tool_input": input]))
        #expect(try await eventually { await self.session()?.activePermission?.toolUseId == "toolu_bg" })

        // The main turn ends while the agent waits for its answer.
        try await send("Stop", ["last_assistant_message": "Agents are running.", "background_tasks": [["type": "subagent"]]])
        #expect(try await eventually { await self.session()?.completedAt != nil })
        #expect(server.hasPendingPermission(toolUseId: "toolu_bg"))
        #expect(agentHook.isRunning)
        #expect(await session()?.attention == .needsInput(.permission(tool: "Bash")))

        monitor.approvePermission(sessionId: "int-1", toolUseId: "toolu_bg")
        #expect(try await exited(agentHook))
        #expect(try decision(agentOut)?["behavior"] as? String == "allow")
        #expect(try await eventually { await self.session()?.activePermission == nil })
    }

    @Test func aMainRequestFallsBackToTheTerminalAtStop() async throws {
        try await start()
        try await send("UserPromptSubmit", ["prompt": "clean up", "source": "user"])
        let input: [String: Any] = ["command": "rm -rf build"]
        try await send("PreToolUse", ["tool_name": "Bash", "tool_input": input, "tool_use_id": "toolu_main"])
        let (hook, out) = try runHook(payload("PermissionRequest", ["tool_name": "Bash", "tool_input": input]))
        #expect(try await eventually { await self.session()?.activePermission?.toolUseId == "toolu_main" })

        try await send("StopFailure", ["error": "overloaded"])
        #expect(try await exited(hook))
        // No decision: Claude Code's own dialog handles it.
        #expect(try decision(out) == nil)
        #expect(!server.hasPendingPermission(toolUseId: "toolu_main"))
        #expect(try await eventually { await self.session()?.attention == .needsInput(.error("Overloaded")) })
    }

    // MARK: - Click-time identity

    @Test func aDoubleClickAnswersOnlyTheRequestShown() async throws {
        try await start()
        try await send("UserPromptSubmit", ["prompt": "tidy", "source": "user"])
        let first: [String: Any] = ["command": "ls"]
        let second: [String: Any] = ["command": "git push --force"]
        try await send("PreToolUse", ["tool_name": "Bash", "tool_input": first, "tool_use_id": "toolu_a"])
        try await send("PreToolUse", ["tool_name": "Bash", "tool_input": second, "tool_use_id": "toolu_b"])
        let (hookA, outA) = try runHook(payload("PermissionRequest", ["tool_name": "Bash", "tool_input": first]))
        #expect(try await eventually { await self.session()?.activePermission?.toolUseId == "toolu_a" })
        let (hookB, _) = try runHook(payload("PermissionRequest", ["tool_name": "Bash", "tool_input": second]))
        #expect(try await eventually { await self.session()?.queuedApprovals.first?.toolUseId == "toolu_b" })

        // Two clicks on the Allow of request A.
        monitor.approvePermission(sessionId: "int-1", toolUseId: "toolu_a")
        monitor.approvePermission(sessionId: "int-1", toolUseId: "toolu_a")
        #expect(try await exited(hookA))
        #expect(try decision(outA)?["behavior"] as? String == "allow")
        #expect(try await eventually { await self.session()?.activePermission?.toolUseId == "toolu_b" })
        try await Task.sleep(nanoseconds: 300_000_000)
        #expect(hookB.isRunning)
        #expect(server.hasPendingPermission(toolUseId: "toolu_b"))

        monitor.denyPermission(sessionId: "int-1", toolUseId: "toolu_b", reason: "Not now")
        #expect(try await exited(hookB))
    }

    // MARK: - Requests whose input another hook rewrote

    @Test func aRewrittenRequestMatchesTheOnlyCallInFlight() async throws {
        try await start()
        try await send("UserPromptSubmit", ["prompt": "publish", "source": "user"])
        try await send("PreToolUse", ["tool_name": "Bash", "tool_input": ["command": "npm publish"], "tool_use_id": "toolu_pub"])
        let (hook, out) = try runHook(payload("PermissionRequest", ["tool_name": "Bash", "tool_input": ["command": "npm publish --dry-run"]]))
        #expect(try await eventually { await self.session()?.activePermission != nil })
        let context = try #require(await session()?.activePermission)
        #expect(context.toolUseId == "toolu_pub")
        #expect(!context.hasSyntheticToolUseId)

        // Answered in the terminal: its own PostToolUse settles the request.
        try await send("PostToolUse", ["tool_name": "Bash", "tool_input": ["command": "npm publish --dry-run"], "tool_use_id": "toolu_pub"])
        #expect(try await exited(hook))
        #expect(try decision(out) == nil)
        #expect(try await eventually { await self.session()?.activePermission == nil })
    }

    // MARK: - Throughput

    @Test func aBurstIsAppliedWhileTheMainThreadIsBusy() async throws {
        // Production publish coalescing, and the real main-queue side effects.
        let busyStore = SessionStore(
            reviewStore: ReviewStateStore(fileURL: account.reviewFile, writeDelay: 0, createsFolder: false),
            parser: ConversationParser(),
            publishInterval: 0.05,
            effects: .none,
            completionTiming: .immediate
        )
        let busyServer = HookSocketServer(socketPath: socketPath)
        let busyMonitor = ClaudeSessionMonitor(store: busyStore, server: busyServer)
        busyMonitor.startSessionPipeline()
        defer { busyMonitor.stop() }

        let sessions = 30
        let turns = 4
        var events: [HookEvent] = []
        for turn in 0..<turns {
            for index in 0..<sessions {
                let id = "burst-\(index)"
                let tool = "toolu_\(index)_\(turn)"
                events.append(HookEvent(sessionId: id, event: "UserPromptSubmit", status: "processing", cwd: "/tmp/p\(index)", attended: true, entrypoint: "cli", source: "user"))
                events.append(HookEvent(sessionId: id, event: "PreToolUse", status: "running_tool", cwd: "/tmp/p\(index)", attended: true, entrypoint: "cli", tool: "Bash", toolInput: [:], toolUseId: tool))
                events.append(HookEvent(sessionId: id, event: "PostToolUse", status: "processing", cwd: "/tmp/p\(index)", attended: true, entrypoint: "cli", tool: "Bash", toolInput: [:], toolUseId: tool))
                events.append(HookEvent(sessionId: id, event: "Stop", status: "waiting_for_input", cwd: "/tmp/p\(index)", attended: true, entrypoint: "cli", lastAssistantMessage: "turn \(turn)"))
            }
        }

        // The main thread is stuck (a panel rendering a burst) for 1.5 s.
        let blocked = 1.5
        DispatchQueue.main.async { Thread.sleep(forTimeInterval: blocked) }
        let begin = Date()
        for event in events {
            busyMonitor.enqueue(.socket(.hook(event)))
        }
        // Polled off the main thread, which is about to be stuck.
        let drained = Task.detached { () -> Date in
            while busyMonitor.pipelineBacklog > 0 {
                usleep(2_000)
            }
            return Date()
        }
        let finishedAt = await drained.value
        let elapsed = finishedAt.timeIntervalSince(begin)
        #expect(elapsed < blocked - 0.3, "the store waited for the main thread: \(elapsed) s")

        for index in 0..<sessions {
            let state = try #require(await busyStore.session(for: "burst-\(index)"))
            #expect(state.attention == .readyForReview)
            #expect(state.lastAssistantMessage == "turn \(turns - 1)")
        }
    }
}
