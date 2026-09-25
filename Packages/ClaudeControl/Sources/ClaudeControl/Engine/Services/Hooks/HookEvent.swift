//
//  HookEvent.swift
//  ClaudeIsland
//
//  Wire format of the socket protocol (v2): the JSON the hook script
//  (agentnotch-hook.py) and the status line wrapper
//  (agentnotch-statusline.py) write to the app, and the permission
//  response the app writes back. Decoding is lenient: unknown events, extra
//  keys and loosely typed values never make a message undecodable, and fields
//  a newer script adds (`stop_hook_active`, `background_task_types`,
//  `session_cron_count`) are simply absent from an older one.
//

import Foundation

// MARK: - Hook Event

/// Event received from Claude Code hooks.
nonisolated struct HookEvent: Decodable, Sendable {
    // Base fields, present on every event
    let sessionId: String
    let cwd: String
    let event: String
    /// Coarse status from the script: running_tool, processing,
    /// waiting_for_approval, waiting_for_input, notification, compacting, ended, unknown.
    let status: String
    let pid: Int?
    let tty: String?
    let transcriptPath: String?
    /// Raw CLAUDE_CONFIG_DIR of the Claude process; nil when unset (default account).
    let configDirEnv: String?
    /// CLAUDE_CODE_SESSION_ATTENDED: false for background/daemon sessions.
    let attended: Bool?
    /// CLAUDE_CODE_ENTRYPOINT, e.g. `cli`, `claude-vscode`, `sdk-ts`.
    let entrypoint: String?
    /// Present only for events fired inside a subagent.
    let agentId: String?
    let agentType: String?
    let permissionMode: String?

    // Tool events
    let tool: String?
    let toolInput: [String: AnyCodable]?
    /// Claude Code's tool_use_id. PermissionRequest carries none; the socket
    /// server fills it in from the matching PreToolUse.
    var toolUseId: String?
    /// True when the server could not match a PermissionRequest to a
    /// PreToolUse and invented `toolUseId` to keep the request answerable.
    var hasSyntheticToolUseId: Bool
    let toolError: String?
    let isInterrupt: Bool?
    let permissionSuggestions: [AnyCodable]?
    let denialReason: String?

    // Task tools (TaskCreate PostToolUse, TaskCreated, TaskCompleted)
    let taskId: String?
    let taskSubject: String?

    // Notification
    let notificationType: String?
    let message: String?
    let title: String?

    // Stop / StopFailure / SubagentStop
    let lastAssistantMessage: String?
    let backgroundTaskCount: Int?
    /// `type` of each in-flight background task at Stop ("shell", "subagent",
    /// "monitor", "workflow", …), when the script forwards them.
    let backgroundTaskTypes: [String]?
    /// Session-scoped crons and wake-ups (/loop, ScheduleWakeup) at Stop.
    let sessionCronCount: Int?
    /// Stop only: this Stop ends a continuation that a blocking Stop hook
    /// (e.g. /goal) forced; the turn had already stopped once.
    let stopHookActive: Bool?
    let stopError: String?
    let stopErrorDetails: String?
    let agentTranscriptPath: String?

    // SessionStart / UserPromptSubmit / SessionEnd / compaction
    let source: String?
    let model: String?
    let sessionTitle: String?
    let prompt: String?
    let reason: String?
    let trigger: String?

    /// When the socket server finished reading the message. Every time the
    /// store records (turn start, completion, review) is this one, not the
    /// moment the event is processed, so a backlog can't reorder them
    /// against the user's own actions.
    var receivedAt: Date

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case cwd, event, status, pid, tty
        case transcriptPath = "transcript_path"
        case configDirEnv = "config_dir_env"
        case attended, entrypoint
        case agentId = "agent_id"
        case agentType = "agent_type"
        case permissionMode = "permission_mode"
        case tool
        case toolInput = "tool_input"
        case toolUseId = "tool_use_id"
        case toolError = "tool_error"
        case isInterrupt = "is_interrupt"
        case permissionSuggestions = "permission_suggestions"
        case denialReason = "denial_reason"
        case taskId = "task_id"
        case taskSubject = "task_subject"
        case notificationType = "notification_type"
        case message, title
        case lastAssistantMessage = "last_assistant_message"
        case backgroundTaskCount = "background_task_count"
        case backgroundTaskTypes = "background_task_types"
        case sessionCronCount = "session_cron_count"
        case stopHookActive = "stop_hook_active"
        case stopError = "stop_error"
        case stopErrorDetails = "stop_error_details"
        case agentTranscriptPath = "agent_transcript_path"
        case source, model
        case sessionTitle = "session_title"
        case prompt, reason, trigger
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sessionId = try c.decode(String.self, forKey: .sessionId)
        event = try c.decode(String.self, forKey: .event)
        cwd = c.lossyString(.cwd) ?? ""
        status = c.lossyString(.status) ?? "unknown"
        pid = c.lossyInt(.pid).flatMap(ProcessID.valid)
        tty = c.lossyString(.tty)
        transcriptPath = c.lossyString(.transcriptPath)
        configDirEnv = c.lossyString(.configDirEnv)
        attended = c.lossyBool(.attended)
        entrypoint = c.lossyString(.entrypoint)
        agentId = c.lossyString(.agentId)
        agentType = c.lossyString(.agentType)
        permissionMode = c.lossyString(.permissionMode)
        tool = c.lossyString(.tool)
        toolInput = try? c.decodeIfPresent([String: AnyCodable].self, forKey: .toolInput)
        toolUseId = c.lossyString(.toolUseId)
        hasSyntheticToolUseId = false
        toolError = c.lossyString(.toolError)
        isInterrupt = c.lossyBool(.isInterrupt)
        permissionSuggestions = try? c.decodeIfPresent([AnyCodable].self, forKey: .permissionSuggestions)
        denialReason = c.lossyString(.denialReason)
        taskId = c.lossyString(.taskId)
        taskSubject = c.lossyString(.taskSubject)
        notificationType = c.lossyString(.notificationType)
        message = c.lossyString(.message)
        title = c.lossyString(.title)
        lastAssistantMessage = c.lossyString(.lastAssistantMessage)
        backgroundTaskCount = c.lossyInt(.backgroundTaskCount)
        backgroundTaskTypes = (try? c.decodeIfPresent([AnyCodable].self, forKey: .backgroundTaskTypes))?
            .compactMap { $0.value as? String }
        sessionCronCount = c.lossyInt(.sessionCronCount)
        stopHookActive = c.lossyBool(.stopHookActive)
        stopError = c.lossyString(.stopError)
        stopErrorDetails = c.lossyString(.stopErrorDetails)
        agentTranscriptPath = c.lossyString(.agentTranscriptPath)
        source = c.lossyString(.source)
        model = c.lossyString(.model)
        sessionTitle = c.lossyString(.sessionTitle)
        prompt = c.lossyString(.prompt)
        reason = c.lossyString(.reason)
        trigger = c.lossyString(.trigger)
        receivedAt = Date()
    }

    /// Memberwise initializer, mainly for tests and internally synthesized events.
    init(
        sessionId: String,
        event: String,
        status: String = "unknown",
        cwd: String = "",
        pid: Int? = nil,
        tty: String? = nil,
        transcriptPath: String? = nil,
        configDirEnv: String? = nil,
        attended: Bool? = nil,
        entrypoint: String? = nil,
        agentId: String? = nil,
        agentType: String? = nil,
        permissionMode: String? = nil,
        tool: String? = nil,
        toolInput: [String: AnyCodable]? = nil,
        toolUseId: String? = nil,
        hasSyntheticToolUseId: Bool = false,
        toolError: String? = nil,
        isInterrupt: Bool? = nil,
        permissionSuggestions: [AnyCodable]? = nil,
        denialReason: String? = nil,
        taskId: String? = nil,
        taskSubject: String? = nil,
        notificationType: String? = nil,
        message: String? = nil,
        title: String? = nil,
        lastAssistantMessage: String? = nil,
        backgroundTaskCount: Int? = nil,
        backgroundTaskTypes: [String]? = nil,
        sessionCronCount: Int? = nil,
        stopHookActive: Bool? = nil,
        stopError: String? = nil,
        stopErrorDetails: String? = nil,
        agentTranscriptPath: String? = nil,
        source: String? = nil,
        model: String? = nil,
        sessionTitle: String? = nil,
        prompt: String? = nil,
        reason: String? = nil,
        trigger: String? = nil,
        receivedAt: Date = Date()
    ) {
        self.sessionId = sessionId
        self.event = event
        self.status = status
        self.cwd = cwd
        self.pid = pid
        self.tty = tty
        self.transcriptPath = transcriptPath
        self.configDirEnv = configDirEnv
        self.attended = attended
        self.entrypoint = entrypoint
        self.agentId = agentId
        self.agentType = agentType
        self.permissionMode = permissionMode
        self.tool = tool
        self.toolInput = toolInput
        self.toolUseId = toolUseId
        self.hasSyntheticToolUseId = hasSyntheticToolUseId
        self.toolError = toolError
        self.isInterrupt = isInterrupt
        self.permissionSuggestions = permissionSuggestions
        self.denialReason = denialReason
        self.taskId = taskId
        self.taskSubject = taskSubject
        self.notificationType = notificationType
        self.message = message
        self.title = title
        self.lastAssistantMessage = lastAssistantMessage
        self.backgroundTaskCount = backgroundTaskCount
        self.backgroundTaskTypes = backgroundTaskTypes
        self.sessionCronCount = sessionCronCount
        self.stopHookActive = stopHookActive
        self.stopError = stopError
        self.stopErrorDetails = stopErrorDetails
        self.agentTranscriptPath = agentTranscriptPath
        self.source = source
        self.model = model
        self.sessionTitle = sessionTitle
        self.prompt = prompt
        self.reason = reason
        self.trigger = trigger
        self.receivedAt = receivedAt
    }

    // MARK: - Derived

    /// Whether this event expects a response (permission request)
    var expectsResponse: Bool {
        event == "PermissionRequest"
    }

    /// Fired inside a subagent (Agent/Task tool), not by the main session.
    var isSubagentEvent: Bool {
        !(agentId ?? "").isEmpty
    }

    /// Background, daemon and SDK sessions the app ignores entirely.
    var isFromIgnoredSession: Bool {
        SessionFilter.isIgnored(attended: attended, entrypoint: entrypoint)
    }

    /// The account's config dir: from transcript_path, else CLAUDE_CONFIG_DIR, else ~/.claude.
    var resolvedConfigDir: String {
        SessionFilter.configDir(transcriptPath: transcriptPath, configDirEnv: configDirEnv)
    }

    /// Top-level scalar tool input values as strings, for chat rows and previews.
    var flatToolInput: [String: String] {
        ToolInput.flatten((toolInput ?? [:]).mapValues(\.value))
    }

    // MARK: - Prompts

    /// Whether this UserPromptSubmit is the user typing (or sending from an
    /// editor), which means they have seen the previous result.
    ///
    /// - `user` (terminal) and a missing source (older Claude Code) count.
    /// - `sdk` counts only from an attended editor or desktop host
    ///   (`claude-vscode`, `claude-desktop`, …): that is how VS Code submits
    ///   what the user typed. Background-task wake-ups arrive as `sdk` there
    ///   too, so a prompt that is an injected notification or command echo
    ///   never counts.
    /// - `system`, `loop_wakeup`, `schedule_wakeup`, `poll_event` never count.
    var isUserAuthoredPrompt: Bool {
        guard event == "UserPromptSubmit" else { return false }
        switch source {
        case nil, "user"?:
            return !ToolInput.isInjectedPrompt(prompt)
        case "sdk"?:
            guard let entrypoint = entrypoint?.lowercased(), entrypoint.hasPrefix("claude-"),
                  let prompt, !prompt.isEmpty else { return false }
            return !ToolInput.isInjectedPrompt(prompt)
        default:
            return false
        }
    }

    // MARK: - Notifications

    /// Agent view's own announcements ("<label> needs your input: …",
    /// "<label> finished") fire on the session hosting agent view, but are
    /// about another (background) session; they must not flag this one.
    var isAgentViewAnnouncement: Bool {
        guard event == "Notification" else { return false }
        if notificationType == "agent_completed" { return true }
        guard notificationType == "agent_needs_input", let message else { return false }
        return message.range(of: #"^.+ needs your input(:|$)"#, options: .regularExpression) != nil
    }
}

// MARK: - Tool Input

/// One way to turn tool inputs into display strings, shared by the hook
/// path (`AnyCodable`) and the transcript path (`JSONSerialization`).
nonisolated enum ToolInput {
    /// Top-level scalars as strings. JSONSerialization hands booleans over as
    /// NSNumber, which also casts to Int, so the CFBoolean check comes first:
    /// `true` stays "true" on both paths, and doubles are kept on both.
    static func flatten(_ input: [String: Any]) -> [String: String] {
        var flat: [String: String] = [:]
        for (key, value) in input {
            if let string = scalarString(value) {
                flat[key] = string
            }
        }
        return flat
    }

    /// A scalar JSON value as text; nil for arrays, objects and null.
    static func scalarString(_ value: Any) -> String? {
        switch value {
        case let string as String:
            return string
        case let number as NSNumber:
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return number.boolValue ? "true" : "false"
            }
            if CFNumberIsFloatType(number) {
                return String(number.doubleValue)
            }
            return String(number.int64Value)
        default:
            return nil
        }
    }

    /// The one-line preview of a tool call: the file name for file tools,
    /// the command, pattern, URL, query or description where a tool has one,
    /// else the first telling string. Capped at `maxLength` characters with
    /// "..." when given.
    static func preview(toolName: String, input: [String: String], maxLength: Int? = nil) -> String? {
        let text: String?
        switch toolName {
        case "Read", "Write", "Edit", "MultiEdit", "NotebookEdit":
            text = (input["file_path"] ?? input["notebook_path"]).map { ($0 as NSString).lastPathComponent }
        case "Bash":
            text = input["command"]
        case "Grep", "Glob":
            text = input["pattern"]
        case "Task", "Agent":
            text = input["description"]
        case "WebFetch":
            text = input["url"]
        case "WebSearch":
            text = input["query"]
        default:
            let priority = ["command", "file_path", "path", "query", "pattern", "url"]
            text = priority.lazy.compactMap { input[$0] }.first
                ?? input.keys.sorted().lazy
                    .filter { $0 != "description" }
                    .compactMap { input[$0] }
                    .first { !$0.isEmpty }
        }
        guard let text, !text.isEmpty else { return nil }
        guard let maxLength, text.count > maxLength else { return text }
        return String(text.prefix(maxLength)) + "..."
    }

    /// The preview that may leave the panel (hover rows, the phone link,
    /// banners in Notification Center): only named fields — the command, a
    /// file's name, a search pattern or query, a URL's host — never a free
    /// text field an MCP or unknown tool took (a message body, a channel, a
    /// description Claude wrote). Nil means "the tool's name alone" (S4).
    static func offPanelPreview(toolName: String, input: [String: String], maxLength: Int? = nil) -> String? {
        let text: String?
        switch toolName {
        case "Read", "Write", "Edit", "MultiEdit", "NotebookEdit":
            text = (input["file_path"] ?? input["notebook_path"]).map { ($0 as NSString).lastPathComponent }
        case "Bash":
            text = input["command"]
        case "Grep", "Glob":
            text = input["pattern"]
        case "WebFetch":
            text = input["url"].flatMap { URL(string: $0)?.host }
        case "WebSearch":
            text = input["query"]
        default:
            text = nil
        }
        guard let text, !text.isEmpty else { return nil }
        guard let maxLength, text.count > maxLength else { return text }
        return String(text.prefix(maxLength)) + "..."
    }

    /// Text Claude Code puts in the user turn itself: background-task
    /// notifications, slash-command echoes and caveats.
    static func isInjectedPrompt(_ prompt: String?) -> Bool {
        guard let prompt = prompt?.drop(while: \.isWhitespace) else { return false }
        return injectedPrefixes.contains { prompt.hasPrefix($0) }
    }

    static let injectedPrefixes = [
        "<task-notification>",
        "<command-name>",
        "<command-message>",
        "<local-command",
        "<bash-input>",
        "<bash-stdout>",
        "Caveat:",
    ]
}

// MARK: - Process IDs

/// PIDs arrive as JSON from other processes; anything outside `pid_t`'s
/// positive range would trap when converted, so it is treated as unknown.
nonisolated enum ProcessID {
    static func valid(_ pid: Int) -> Int? {
        pid > 0 && pid <= Int(Int32.max) ? pid : nil
    }

    /// kill(pid, 0): the process exists (EPERM means it exists but isn't ours).
    static func isRunning(_ pid: Int) -> Bool {
        guard let pid = Int32(exactly: pid), pid > 0 else { return false }
        return kill(pid, 0) == 0 || errno == EPERM
    }
}

// MARK: - Session Filter

/// Which sessions the app tracks, and which account a session belongs to.
nonisolated enum SessionFilter {
    /// Unattended sessions (`CLAUDE_CODE_SESSION_ATTENDED=0`: background,
    /// daemon, scheduled) and non-interactive SDK entrypoints (`sdk-ts`,
    /// `sdk-py`, `sdk-cli` for `claude -p`) are ignored. The terminal CLI and
    /// the VS Code extension (`claude-vscode`) are kept.
    static func isIgnored(attended: Bool?, entrypoint: String?) -> Bool {
        if attended == false { return true }
        guard let entrypoint = entrypoint?.lowercased(), !entrypoint.isEmpty else { return false }
        return entrypoint.hasPrefix("sdk")
    }

    /// Registry entries: only interactive sessions (or entries predating `kind`).
    static func isIgnored(registryKind kind: String?, entrypoint: String?) -> Bool {
        if let kind, !kind.isEmpty, kind != "interactive" { return true }
        return isIgnored(attended: nil, entrypoint: entrypoint)
    }

    /// Normalized config dir for a session: the transcript path is authoritative,
    /// then the raw CLAUDE_CONFIG_DIR, then the default ~/.claude.
    static func configDir(transcriptPath: String?, configDirEnv: String?) -> String {
        // A transcript path through the shared history's own folder (a
        // resolved link) names no account; the environment then decides.
        if let transcriptPath, let dir = AccountPaths.configDir(fromTranscriptPath: transcriptPath),
           !AccountPaths.isInfrastructureDir(dir) {
            return dir
        }
        if let env = configDirEnv, !env.isEmpty {
            return AccountPaths.normalize(env)
        }
        return AccountPaths.defaultConfigDir
    }

    static func accountId(transcriptPath: String?, configDirEnv: String?) -> String {
        AccountPaths.accountId(forConfigDir: configDir(transcriptPath: transcriptPath, configDirEnv: configDirEnv))
    }
}

// MARK: - Status Line

/// A `StatusLine` message from the status line wrapper of a live terminal session.
nonisolated struct StatusLineMessage: Sendable {
    let sessionId: String
    let cwd: String?
    let transcriptPath: String?
    let configDirEnv: String?
    /// The parsed payload, as published on `AppEventBus.statusLineUpdates`.
    let update: StatusLineUpdate

    /// Parses `{"event":"StatusLine","session_id",...,"status_line":{...}}`.
    init?(json: [String: Any], receivedAt: Date = Date()) {
        guard let sessionId = json["session_id"] as? String, !sessionId.isEmpty else { return nil }
        let transcriptPath = JSONValue.string(json["transcript_path"])
        let configDirEnv = JSONValue.string(json["config_dir_env"])
        let statusLine = json["status_line"] as? [String: Any] ?? [:]

        // One parser for rate limits (UsageParser's): booleans, NaN and
        // infinities are not percentages, and resets_at may be epoch
        // seconds, milliseconds or ISO 8601.
        let windows = UsageParser.parseStatusLineRateLimits(statusLine["rate_limits"])
        let context = statusLine["context_window"] as? [String: Any]
        let model = statusLine["model"] as? [String: Any]
        let cost = statusLine["cost"] as? [String: Any]

        self.sessionId = sessionId
        self.cwd = JSONValue.string(json["cwd"])
        self.transcriptPath = transcriptPath
        self.configDirEnv = configDirEnv
        self.update = StatusLineUpdate(
            sessionId: sessionId,
            transcriptPath: transcriptPath,
            configDirEnv: configDirEnv,
            accountId: SessionFilter.accountId(transcriptPath: transcriptPath, configDirEnv: configDirEnv),
            receivedAt: receivedAt,
            fiveHour: windows.fiveHour,
            sevenDay: windows.sevenDay,
            contextUsedPercent: JSONValue.double(context?["used_percentage"]),
            contextWindowSize: JSONValue.int(context?["context_window_size"]),
            modelId: JSONValue.string(model?["id"]),
            modelDisplayName: JSONValue.string(model?["display_name"]),
            costUSD: JSONValue.double(cost?["total_cost_usd"]),
            sessionName: JSONValue.string(statusLine["session_name"]),
            claudeCodeVersion: JSONValue.string(statusLine["version"])
        )
    }
}

// MARK: - Socket Message

/// Anything that can arrive on the hook socket.
nonisolated enum HookSocketMessage: Sendable {
    case hook(HookEvent)
    case statusLine(StatusLineMessage)

    /// Decodes one message. Returns nil for malformed JSON or a hook event
    /// without `session_id`/`event`.
    static func decode(_ data: Data, receivedAt: Date = Date()) -> HookSocketMessage? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        if object["event"] as? String == "StatusLine" {
            return StatusLineMessage(json: object, receivedAt: receivedAt).map { .statusLine($0) }
        }
        guard var event = try? JSONDecoder().decode(HookEvent.self, from: data) else { return nil }
        event.receivedAt = receivedAt
        return .hook(event)
    }
}

// MARK: - Permission Response

/// Decision sent back to a blocked PermissionRequest hook.
nonisolated enum PermissionDecision: String, Encodable, Sendable {
    case allow
    /// Deny with a reason shown to Claude.
    case deny
    /// No decision: the hook exits silently and Claude Code's own prompt handles it.
    case ask
}

/// Response written to the hook script's socket. The script turns it into
/// Claude Code's `hookSpecificOutput.decision` JSON.
nonisolated struct PermissionResponse: Encodable, Sendable {
    let decision: PermissionDecision
    var reason: String?
    /// Fields to set on top of the tool's ORIGINAL input (the script merges
    /// them). An empty object means "echo the original input", which is how
    /// tools needing user interaction (ExitPlanMode) are approved;
    /// AskUserQuestion is answered with `["answers": [question: label]]`.
    var updatedInput: [String: AnyCodable]?
    /// PermissionUpdate objects, e.g. the request's first permission suggestion
    /// for "Yes, and don't ask again".
    var updatedPermissions: [AnyCodable]?
    /// With `deny`: also interrupt Claude's turn.
    var interrupt: Bool?

    enum CodingKeys: String, CodingKey {
        case decision, reason
        case updatedInput = "updated_input"
        case updatedPermissions = "updated_permissions"
        case interrupt
    }
}

// MARK: - AnyCodable for tool_input

/// Type-erasing codable wrapper for heterogeneous values
/// Used to decode JSON objects with mixed value types
nonisolated struct AnyCodable: Codable, @unchecked Sendable {
    /// The underlying value (nonisolated(unsafe) because Any is not Sendable)
    nonisolated(unsafe) let value: Any

    /// Initialize with any value
    init(_ value: Any) {
        self.value = value
    }

    /// Decode from JSON
    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        if container.decodeNil() {
            value = NSNull()
        } else if let bool = try? container.decode(Bool.self) {
            value = bool
        } else if let int = try? container.decode(Int.self) {
            value = int
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map { $0.value }
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues { $0.value }
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Cannot decode value")
        }
    }

    /// Encode to JSON
    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()

        switch value {
        case is NSNull:
            try container.encodeNil()
        case let bool as Bool:
            try container.encode(bool)
        case let int as Int:
            try container.encode(int)
        case let double as Double:
            try container.encode(double)
        case let string as String:
            try container.encode(string)
        case let wrapped as AnyCodable:
            try container.encode(wrapped)
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        default:
            throw EncodingError.invalidValue(value, EncodingError.Context(codingPath: [], debugDescription: "Cannot encode value"))
        }
    }
}

// MARK: - Lenient JSON helpers

/// Coercions for loosely typed JSON values (numbers sent as strings and vice
/// versa). Numbers follow `UsageParser.number`: a JSON boolean is not a
/// number, and neither are NaN or infinities.
nonisolated enum JSONValue {
    static func string(_ raw: Any?) -> String? {
        switch raw {
        case let string as String:
            return string.isEmpty ? nil : string
        case let number as NSNumber:
            return number.stringValue
        default:
            return nil
        }
    }

    static func double(_ raw: Any?) -> Double? {
        UsageParser.number(raw)
    }

    static func int(_ raw: Any?) -> Int? {
        guard let value = UsageParser.number(raw),
              value >= Double(Int.min), value <= Double(Int.max) else { return nil }
        return Int(value)
    }
}

private extension KeyedDecodingContainer {
    nonisolated func lossyString(_ key: Key) -> String? {
        if let string = try? decodeIfPresent(String.self, forKey: key) {
            return string
        }
        if let int = try? decodeIfPresent(Int.self, forKey: key) {
            return String(int)
        }
        if let double = try? decodeIfPresent(Double.self, forKey: key) {
            return String(double)
        }
        return nil
    }

    nonisolated func lossyInt(_ key: Key) -> Int? {
        if let int = try? decodeIfPresent(Int.self, forKey: key) {
            return int
        }
        if let string = try? decodeIfPresent(String.self, forKey: key) {
            return Int(string)
        }
        return nil
    }

    nonisolated func lossyBool(_ key: Key) -> Bool? {
        if let bool = try? decodeIfPresent(Bool.self, forKey: key) {
            return bool
        }
        if let string = try? decodeIfPresent(String.self, forKey: key) {
            switch string.lowercased() {
            case "1", "true", "yes": return true
            case "0", "false", "no": return false
            default: return nil
            }
        }
        if let int = try? decodeIfPresent(Int.self, forKey: key) {
            return int != 0
        }
        return nil
    }
}
