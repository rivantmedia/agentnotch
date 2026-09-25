import Foundation
import Testing
@testable import ClaudeControl

/// Runs the real hook script against a private HookSocketServer: the
/// PermissionRequest round-trip (read-until-EOF, response shape, merge onto
/// the original tool_input) and dead-hook detection.
@Suite(.serialized)
struct HookSocketIntegrationTests {
    /// Packages/ClaudeControl/Scripts/agentnotch-hook.py.
    private static let hookScript: String =
        TestPaths.scripts.appendingPathComponent(ClaudeControlConfiguration.defaultHookScriptName).path

    /// Short path: sun_path holds only 104 bytes.
    private static func socketPath() -> String {
        "/tmp/agentnotch-test-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
    }

    /// Collects messages delivered by the server.
    private nonisolated final class Inbox: @unchecked Sendable {
        private let lock = NSLock()
        private var events: [HookEvent] = []
        private var failures: [(String, String)] = []

        func add(_ message: HookSocketMessage) {
            guard case .hook(let event) = message else { return }
            lock.lock(); events.append(event); lock.unlock()
        }

        func addFailure(_ sessionId: String, _ toolUseId: String) {
            lock.lock(); failures.append((sessionId, toolUseId)); lock.unlock()
        }

        func event(named name: String) -> HookEvent? {
            lock.lock(); defer { lock.unlock() }
            return events.first { $0.event == name }
        }

        var failureCount: Int {
            lock.lock(); defer { lock.unlock() }
            return failures.count
        }
    }

    private func startServer(path: String, inbox: Inbox) async throws -> HookSocketServer {
        let server = HookSocketServer(socketPath: path)
        server.start(
            onMessage: { inbox.add($0) },
            onPermissionFailure: { inbox.addFailure($0, $1) }
        )
        try await waitUntil { FileManager.default.fileExists(atPath: path) }
        return server
    }

    private func runHook(socket: String, payload: [String: Any]) throws -> (Process, Pipe) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", Self.hookScript]
        process.environment = [
            "AGENTNOTCH_SOCKET": socket,
            "AGENTNOTCH_DEV": "1",
            "PATH": "/usr/bin:/bin",
            "CLAUDE_PID": String(getpid()),
            "CLAUDE_CODE_SESSION_ATTENDED": "1",
            "CLAUDE_CODE_ENTRYPOINT": "cli",
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

    private func waitUntil(timeout: TimeInterval = 5, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            guard Date() < deadline else {
                Issue.record("timed out waiting")
                return
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
    }

    private func basePayload(_ event: String) -> [String: Any] {
        [
            "hook_event_name": event,
            "session_id": "sess-int",
            "transcript_path": "/tmp/agentnotch-int/.claude/projects/-tmp/sess-int.jsonl",
            "cwd": "/tmp",
            "permission_mode": "default",
        ]
    }

    @Test func answersAskUserQuestionThroughTheHook() async throws {
        let path = Self.socketPath()
        let inbox = Inbox()
        let server = try await startServer(path: path, inbox: inbox)
        defer { server.stop() }

        let longText = String(repeating: "x", count: 30_000)
        let toolInput: [String: Any] = [
            "questions": [["question": "Which DB?", "header": "DB", "multiSelect": false,
                           "options": [["label": "Postgres", "description": "Relational"], ["label": "SQLite", "description": longText]]]],
        ]
        var pre = basePayload("PreToolUse")
        pre["tool_name"] = "AskUserQuestion"
        pre["tool_input"] = toolInput
        pre["tool_use_id"] = "toolu_ask"
        let (preProcess, _) = try runHook(socket: path, payload: pre)
        preProcess.waitUntilExit()
        try await waitUntil { inbox.event(named: "PreToolUse") != nil }

        var request = basePayload("PermissionRequest")
        request["tool_name"] = "AskUserQuestion"
        request["tool_input"] = toolInput
        request["permission_suggestions"] = []
        let (process, stdout) = try runHook(socket: path, payload: request)
        try await waitUntil { inbox.event(named: "PermissionRequest") != nil }

        let received = try #require(inbox.event(named: "PermissionRequest"))
        #expect(received.toolUseId == "toolu_ask")
        #expect(!received.hasSyntheticToolUseId)
        #expect(server.hasPendingPermission(toolUseId: "toolu_ask"))

        let delivered = await server.respondToPermission(
            toolUseId: "toolu_ask",
            decision: .allow,
            updatedInput: ["answers": AnyCodable(["Which DB?": "Postgres"] as [String: Any])]
        )
        #expect(delivered)
        process.waitUntilExit()

        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        let json = try #require(try JSONSerialization.jsonObject(with: output) as? [String: Any])
        let specific = try #require(json["hookSpecificOutput"] as? [String: Any])
        #expect(specific["hookEventName"] as? String == "PermissionRequest")
        let decision = try #require(specific["decision"] as? [String: Any])
        #expect(decision["behavior"] as? String == "allow")
        let updatedInput = try #require(decision["updatedInput"] as? [String: Any])
        #expect(updatedInput["answers"] as? [String: String] == ["Which DB?": "Postgres"])
        // Merged onto the ORIGINAL input: the 30k description is intact, not the truncated copy.
        let questions = try #require(updatedInput["questions"] as? [[String: Any]])
        let options = try #require(questions.first?["options"] as? [[String: Any]])
        #expect((options.last?["description"] as? String)?.count == 30_000)
        #expect(!server.hasPendingPermission(toolUseId: "toolu_ask"))
    }

    @Test func denyAndAlwaysAllowShapes() async throws {
        let path = Self.socketPath()
        let inbox = Inbox()
        let server = try await startServer(path: path, inbox: inbox)
        defer { server.stop() }

        var request = basePayload("PermissionRequest")
        request["tool_name"] = "Bash"
        request["tool_input"] = ["command": "rm -rf build"]
        let suggestion: [String: Any] = ["type": "addRules", "rules": [["toolName": "Bash", "ruleContent": "rm -rf build"]], "behavior": "allow", "destination": "session"]
        request["permission_suggestions"] = [suggestion]

        // No PreToolUse was seen: the server makes up an id and still holds the request.
        let (process, stdout) = try runHook(socket: path, payload: request)
        try await waitUntil { inbox.event(named: "PermissionRequest") != nil }
        let received = try #require(inbox.event(named: "PermissionRequest"))
        #expect(received.hasSyntheticToolUseId)
        let toolUseId = try #require(received.toolUseId)

        let delivered = await server.respondToPermission(
            toolUseId: toolUseId,
            decision: .allow,
            updatedPermissions: received.permissionSuggestions.map { Array($0.prefix(1)) }
        )
        #expect(delivered)
        process.waitUntilExit()
        let json = try #require(try JSONSerialization.jsonObject(with: stdout.fileHandleForReading.readDataToEndOfFile()) as? [String: Any])
        let decision = try #require((json["hookSpecificOutput"] as? [String: Any])?["decision"] as? [String: Any])
        #expect(decision["behavior"] as? String == "allow")
        #expect(decision["updatedInput"] == nil)
        let permissions = try #require(decision["updatedPermissions"] as? [[String: Any]])
        #expect(permissions.first?["destination"] as? String == "session")

        // Deny.
        var second = request
        second["tool_input"] = ["command": "git push --force"]
        let (denyProcess, denyOut) = try runHook(socket: path, payload: second)
        try await waitUntil { server.pendingCount() == 1 }
        let pendingId = try #require(server.pendingPermissionIds().first)
        #expect(await server.respondToPermission(toolUseId: pendingId, decision: .deny, reason: "Not on main"))
        denyProcess.waitUntilExit()
        let denyJson = try #require(try JSONSerialization.jsonObject(with: denyOut.fileHandleForReading.readDataToEndOfFile()) as? [String: Any])
        let denyDecision = try #require((denyJson["hookSpecificOutput"] as? [String: Any])?["decision"] as? [String: Any])
        #expect(denyDecision["behavior"] as? String == "deny")
        #expect(denyDecision["message"] as? String == "Not on main")
    }

    @Test func detectsAHookThatWentAway() async throws {
        let path = Self.socketPath()
        let inbox = Inbox()
        let server = try await startServer(path: path, inbox: inbox)
        defer { server.stop() }

        var request = basePayload("PermissionRequest")
        request["tool_name"] = "Bash"
        request["tool_input"] = ["command": "make"]
        let (process, _) = try runHook(socket: path, payload: request)
        try await waitUntil { inbox.event(named: "PermissionRequest") != nil }
        let toolUseId = try #require(inbox.event(named: "PermissionRequest")?.toolUseId)

        // Claude Code kills the hook when the terminal dialog answers first.
        process.terminate()
        process.waitUntilExit()
        try await waitUntil(timeout: HookSocketServer.livenessCheckInterval * 3) { inbox.failureCount == 1 }
        #expect(!server.hasPendingPermission(toolUseId: toolUseId))
        #expect(await server.respondToPermission(toolUseId: toolUseId, decision: .allow) == false)
    }

    @Test func fireAndForgetEventsAndIgnoredSessions() async throws {
        let path = Self.socketPath()
        let inbox = Inbox()
        let server = try await startServer(path: path, inbox: inbox)
        defer { server.stop() }

        var stop = basePayload("Stop")
        stop["last_assistant_message"] = String(repeating: "y", count: 5000)
        stop["background_tasks"] = [["id": "b1"], ["id": "b2"]]
        let (process, _) = try runHook(socket: path, payload: stop)
        process.waitUntilExit()
        try await waitUntil { inbox.event(named: "Stop") != nil }
        let event = try #require(inbox.event(named: "Stop"))
        #expect(event.lastAssistantMessage?.count == 1500)
        #expect(event.backgroundTaskCount == 2)
        #expect(event.pid == Int(getpid()))
        #expect(event.attended == true)
        #expect(event.entrypoint == "cli")
    }
}

extension HookSocketServer {
    /// Test helper.
    func pendingCount() -> Int { pendingPermissionIds().count }
}
