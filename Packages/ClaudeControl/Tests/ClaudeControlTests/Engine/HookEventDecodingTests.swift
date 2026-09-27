import Foundation
import Testing
@testable import ClaudeControl

struct HookEventDecodingTests {
    private func decode(_ object: [String: Any]) throws -> HookSocketMessage? {
        HookSocketMessage.decode(try JSONSerialization.data(withJSONObject: object))
    }

    private func hook(_ object: [String: Any]) throws -> HookEvent {
        guard case .hook(let event)? = try decode(object) else {
            Issue.record("not a hook event")
            throw CancellationError()
        }
        return event
    }

    @Test func decodesBaseAndNewFields() throws {
        let event = try hook([
            "event": "Stop",
            "session_id": "abc",
            "cwd": "/Users/me/proj",
            "transcript_path": "/Users/me/.claude-work/projects/-Users-me-proj/abc.jsonl",
            "pid": 4242,
            "tty": "/dev/ttys003",
            "status": "waiting_for_input",
            "config_dir_env": "/Users/me/.claude-work",
            "attended": true,
            "entrypoint": "cli",
            "agent_id": NSNull(),
            "permission_mode": "default",
            "last_assistant_message": "All done.",
            "background_task_count": 2,
        ])
        #expect(event.sessionId == "abc")
        #expect(event.pid == 4242)
        #expect(event.transcriptPath?.hasSuffix("abc.jsonl") == true)
        #expect(event.configDirEnv == "/Users/me/.claude-work")
        #expect(event.attended == true)
        #expect(event.entrypoint == "cli")
        #expect(event.agentId == nil)
        #expect(!event.isSubagentEvent)
        #expect(event.lastAssistantMessage == "All done.")
        #expect(event.backgroundTaskCount == 2)
        #expect(event.resolvedConfigDir == "/Users/me/.claude-work")
    }

    @Test func decodesPermissionRequestWithNestedInputAndSuggestions() throws {
        let event = try hook([
            "event": "PermissionRequest",
            "session_id": "abc",
            "cwd": "/tmp",
            "status": "waiting_for_approval",
            "tool": "AskUserQuestion",
            "tool_input": ["questions": [["question": "Which DB?", "options": [["label": "Postgres"], ["label": "SQLite"]]]]],
            "permission_suggestions": [["type": "addRules", "rules": [["toolName": "Bash", "ruleContent": "npm test"]], "behavior": "allow", "destination": "session"]],
        ])
        #expect(event.expectsResponse)
        #expect(event.toolUseId == nil)
        let questions = event.toolInput?["questions"]?.value as? [Any]
        #expect(questions?.count == 1)
        #expect(event.permissionSuggestions?.count == 1)
        guard case .waitingForApproval(let context)? = event.determinePhase() else {
            Issue.record("expected an approval phase")
            return
        }
        #expect(context.toolName == "AskUserQuestion")
        #expect(context.canAlwaysAllow)
    }

    @Test func decodesTaskAndFailureFields() throws {
        let created = try hook(["event": "TaskCreated", "session_id": "s", "task_id": 3, "task_subject": "Write docs"])
        #expect(created.taskId == "3")
        #expect(created.taskSubject == "Write docs")
        #expect(created.cwd == "")
        #expect(created.status == "unknown")

        let failure = try hook(["event": "StopFailure", "session_id": "s", "status": "waiting_for_input", "stop_error": "rate_limit", "stop_error_details": "429"])
        #expect(failure.stopError == "rate_limit")
        #expect(failure.determinePhase() == .waitingForInput)

        let post = try hook(["event": "PostToolUseFailure", "session_id": "s", "tool": "Bash", "tool_use_id": "toolu_1", "tool_error": "boom", "is_interrupt": true])
        #expect(post.toolError == "boom")
        #expect(post.isInterrupt == true)
    }

    @Test func lenientTypesAndUnknownEvents() throws {
        let event = try hook([
            "event": "SomeFutureEvent",
            "session_id": "s",
            "pid": "123",
            "attended": "0",
            "unexpected_key": ["nested": true],
        ])
        #expect(event.pid == 123)
        #expect(event.attended == false)
        #expect(event.isFromIgnoredSession)
        #expect(event.determinePhase() == nil)

        #expect(try decode(["session_id": "s"]) == nil)
        #expect(HookSocketMessage.decode(Data("not json".utf8)) == nil)
    }

    @Test func sessionFilterKeepsInteractiveEntrypoints() {
        #expect(!SessionFilter.isIgnored(attended: true, entrypoint: "cli"))
        #expect(!SessionFilter.isIgnored(attended: nil, entrypoint: "claude-vscode"))
        #expect(SessionFilter.isIgnored(attended: nil, entrypoint: "sdk-ts"))
        #expect(SessionFilter.isIgnored(attended: false, entrypoint: "cli"))
        #expect(SessionFilter.isIgnored(registryKind: "bg", entrypoint: "cli"))
        #expect(!SessionFilter.isIgnored(registryKind: nil, entrypoint: "cli"))
    }

    @Test func notificationPhases() throws {
        let idle = try hook(["event": "Notification", "session_id": "s", "status": "waiting_for_input", "notification_type": "idle_prompt"])
        #expect(idle.determinePhase() == .waitingForInput)
        let elicitation = try hook(["event": "Notification", "session_id": "s", "status": "notification", "notification_type": "elicitation_dialog"])
        #expect(elicitation.determinePhase() == nil)
        let manualCompact = try hook(["event": "PostCompact", "session_id": "s", "status": "processing", "trigger": "manual"])
        #expect(manualCompact.determinePhase() == .waitingForInput)
    }

    @Test func parsesStatusLine() throws {
        let message = try decode([
            "event": "StatusLine",
            "session_id": "abc",
            "cwd": "/Users/me/proj",
            "transcript_path": "/Users/me/.claude/projects/-Users-me-proj/abc.jsonl",
            "config_dir_env": NSNull(),
            "pid": 4242,
            "status_line": [
                "rate_limits": [
                    "five_hour": ["used_percentage": 42.5, "resets_at": 1_800_000_000],
                    "seven_day": ["used_percentage": "12", "resets_at": 1_800_300_000],
                ],
                "context_window": ["used_percentage": 37, "context_window_size": 200_000],
                "model": ["id": "claude-opus-4-5", "display_name": "Opus 4.5"],
                "cost": ["total_cost_usd": 1.25],
                "session_name": "Fix login",
                "version": "2.1.280",
            ],
        ])
        guard case .statusLine(let statusLine)? = message else {
            Issue.record("not a status line")
            return
        }
        let update = statusLine.update
        #expect(update.sessionId == "abc")
        #expect(update.accountId == AccountPaths.normalize("/Users/me/.claude"))
        #expect(update.fiveHour?.utilization == 42.5)
        #expect(update.fiveHour?.resetsAt == Date(timeIntervalSince1970: 1_800_000_000))
        #expect(update.fiveHour?.duration == UsageWindow.sessionDuration)
        #expect(update.sevenDay?.utilization == 12)
        #expect(update.sevenDay?.duration == UsageWindow.weeklyDuration)
        #expect(update.contextUsedPercent == 37)
        #expect(update.contextWindowSize == 200_000)
        #expect(update.modelDisplayName == "Opus 4.5")
        #expect(update.costUSD == 1.25)
        #expect(update.sessionName == "Fix login")
        #expect(update.claudeCodeVersion == "2.1.280")
        #expect(update.processId == 4242)
        #expect(statusLine.configDirEnv == nil)
    }

    @Test func statusLineWithoutRateLimits() throws {
        guard case .statusLine(let statusLine)? = try decode(["event": "StatusLine", "session_id": "abc", "status_line": [:]]) else {
            Issue.record("not a status line")
            return
        }
        #expect(statusLine.update.fiveHour == nil)
        #expect(statusLine.update.contextUsedPercent == nil)
        // No pid (an older wrapper, or no CLAUDE_PID): keyed by session instead.
        #expect(statusLine.update.processId == nil)
        // 2^63 once trapped converting to Int; above pid_t's range is no pid either.
        for bad: Any in [NSNull(), 0, -3, "x", true, 9_223_372_036_854_775_808.0, UInt64(1) << 63, Int(Int32.max) + 1] {
            guard case .statusLine(let other)? = try decode(["event": "StatusLine", "session_id": "abc", "pid": bad, "status_line": [:]]) else {
                Issue.record("not a status line")
                return
            }
            #expect(other.update.processId == nil, "pid \(bad)")
        }
    }

    @Test func permissionResponseEncodesNewShape() throws {
        let response = PermissionResponse(
            decision: .allow,
            reason: nil,
            updatedInput: ["answers": AnyCodable(["Which DB?": "Postgres"] as [String: Any])],
            updatedPermissions: [AnyCodable(["type": "addRules", "behavior": "allow"] as [String: Any])],
            interrupt: nil
        )
        let object = try JSONSerialization.jsonObject(with: JSONEncoder().encode(response)) as? [String: Any]
        #expect(object?["decision"] as? String == "allow")
        #expect((object?["updated_input"] as? [String: Any])?["answers"] as? [String: String] == ["Which DB?": "Postgres"])
        #expect((object?["updated_permissions"] as? [Any])?.count == 1)
        #expect(object?["reason"] == nil)

        let deny = try JSONSerialization.jsonObject(with: JSONEncoder().encode(PermissionResponse(decision: .deny, reason: "No", interrupt: true))) as? [String: Any]
        #expect(deny?["decision"] as? String == "deny")
        #expect(deny?["reason"] as? String == "No")
        #expect(deny?["interrupt"] as? Bool == true)
    }

    @Test func toolUseIdCacheHygiene() {
        var cache = ToolUseIdCache()
        let input: [String: AnyCodable] = ["command": AnyCodable("ls")]
        cache.record(sessionId: "s", toolName: "Bash", toolInput: input, toolUseId: "toolu_1")
        cache.record(sessionId: "s", toolName: "Bash", toolInput: input, toolUseId: "toolu_2")
        // toolu_1 was auto-allowed and ran: its id must not be handed out later.
        cache.remove(toolUseId: "toolu_1")
        #expect(cache.pop(sessionId: "s", toolName: "Bash", toolInput: input) == "toolu_2")
        #expect(cache.pop(sessionId: "s", toolName: "Bash", toolInput: input) == nil)

        cache.record(sessionId: "s", toolName: "Bash", toolInput: input, toolUseId: "toolu_3")
        cache.record(sessionId: "other", toolName: "Bash", toolInput: input, toolUseId: "toolu_4")
        cache.removeAll(sessionId: "s")
        #expect(cache.count == 1)
        #expect(cache.pop(sessionId: "other", toolName: "Bash", toolInput: input) == "toolu_4")

        let old = Date(timeIntervalSinceNow: -2 * ToolUseIdCache.maxAge)
        cache.record(sessionId: "s", toolName: "Bash", toolInput: input, toolUseId: "toolu_old", at: old)
        cache.record(sessionId: "s", toolName: "Read", toolInput: nil, toolUseId: "toolu_new")
        #expect(cache.pop(sessionId: "s", toolName: "Bash", toolInput: input) == nil)
    }

    @Test func sightingsAreThrottledPerSession() {
        var throttle = SightingThrottle()
        let now = Date()
        let path = "/Users/me/.claude-work/projects/-x/a.jsonl"
        #expect(throttle.sighting(sessionId: "a", transcriptPath: path, configDirEnv: nil, now: now)?.configDir == "/Users/me/.claude-work")
        #expect(throttle.sighting(sessionId: "a", transcriptPath: path, configDirEnv: nil, now: now.addingTimeInterval(10)) == nil)
        #expect(throttle.sighting(sessionId: "b", transcriptPath: path, configDirEnv: nil, now: now.addingTimeInterval(10)) != nil)
        #expect(throttle.sighting(sessionId: "a", transcriptPath: path, configDirEnv: nil, now: now.addingTimeInterval(61)) != nil)
        #expect(throttle.sighting(sessionId: "c", transcriptPath: nil, configDirEnv: nil, now: now) == nil)
    }
}
