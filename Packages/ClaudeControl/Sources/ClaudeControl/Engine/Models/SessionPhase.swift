//
//  SessionPhase.swift
//  ClaudeIsland
//
//  Explicit state machine for Claude session lifecycle.
//  All state transitions are validated before being applied.
//

import Foundation

/// Permission context for tools waiting for approval
nonisolated struct PermissionContext: Sendable {
    let toolUseId: String
    let toolName: String
    /// The tool's full, nested input as sent by the hook (not flattened), so it
    /// can be shown and, for AskUserQuestion, answered.
    let toolInput: [String: AnyCodable]?
    let receivedAt: Date
    /// Claude Code's `permission_suggestions` (PermissionUpdate objects); the
    /// first one is what the terminal's "Yes, and don't ask again" applies.
    var permissionSuggestions: [AnyCodable]?
    /// True when no PreToolUse matched the request and the socket server made
    /// up `toolUseId`; the request is still answerable through its socket.
    var hasSyntheticToolUseId: Bool = false
    /// The subagent asking (`agent_id`), nil for the main session. A
    /// background agent's request outlives the main turn's Stop.
    var agentId: String?
    /// When this request became the one shown (it may have waited in the
    /// queue behind others). The UI ignores clicks for a moment after that,
    /// so a double-click can't land on a request the user never saw.
    var activatedAt: Date?

    init(
        toolUseId: String,
        toolName: String,
        toolInput: [String: AnyCodable]?,
        receivedAt: Date,
        permissionSuggestions: [AnyCodable]? = nil,
        hasSyntheticToolUseId: Bool = false,
        agentId: String? = nil
    ) {
        self.toolUseId = toolUseId
        self.toolName = toolName
        self.toolInput = toolInput
        self.receivedAt = receivedAt
        self.permissionSuggestions = permissionSuggestions
        self.hasSyntheticToolUseId = hasSyntheticToolUseId
        self.agentId = agentId.flatMap { $0.isEmpty ? nil : $0 }
    }

    /// Asked by a subagent rather than the main session.
    var isFromSubagent: Bool {
        agentId != nil
    }

    /// Whether "always allow" can be offered (Claude Code sent a suggestion to apply).
    var canAlwaysAllow: Bool {
        !(permissionSuggestions ?? []).isEmpty
    }

    /// One-line preview of the input, at most 100 characters (plus "...").
    /// For Write/Edit/Read only the file name: show `fullInput` before
    /// offering Allow on anything longer than the preview.
    var formattedInput: String? {
        ToolInput.preview(toolName: toolName, input: flatInput, maxLength: Self.previewLength)
    }

    /// What may be shown of the input outside the panel (hover rows, the
    /// phone, banners): `ToolInput.offPanelPreview` (S4).
    var offPanelInput: String? {
        ToolInput.offPanelPreview(toolName: toolName, input: flatInput, maxLength: Self.previewLength)
    }

    /// The whole thing being approved, untruncated: the full command, or the
    /// full path (with `~` for the home folder) for file tools.
    var fullInput: String? {
        let input = flatInput
        switch toolName {
        case "Write", "Edit", "MultiEdit", "Read", "NotebookEdit":
            return (input["file_path"] ?? input["notebook_path"]).map(Self.abbreviatingHome)
        default:
            return ToolInput.preview(toolName: toolName, input: input)
        }
    }

    /// The preview hides part of the input (a long or multi-line command, a
    /// path shortened to its file name).
    var isPreviewTruncated: Bool {
        guard let full = fullInput else { return false }
        return full != formattedInput || full.contains("\n")
    }

    /// Longest one-line preview.
    static let previewLength = 100

    /// What the first permission suggestion ("Always allow") would do.
    var alwaysAllowSuggestion: PermissionSuggestion? {
        (permissionSuggestions?.first?.value as? [String: Any]).flatMap(PermissionSuggestion.init(json:))
    }

    private var flatInput: [String: String] {
        ToolInput.flatten((toolInput ?? [:]).mapValues(\.value))
    }

    private static func abbreviatingHome(_ path: String) -> String {
        AccountPathDisplayName.abbreviated(path, home: AppIdentity.homeDirectory)
    }
}

/// Claude Code's PermissionUpdate: what "Yes, and don't ask again" saves.
nonisolated struct PermissionSuggestion: Equatable, Sendable {
    /// addRules, replaceRules, removeRules, setMode, addDirectories, removeDirectories.
    let type: String
    /// session, localSettings, projectSettings, userSettings (or nil).
    let destination: String?
    /// For setMode: acceptEdits, bypassPermissions, plan, default.
    let mode: String?

    init?(json: [String: Any]) {
        guard let type = json["type"] as? String, !type.isEmpty else { return nil }
        self.type = type
        destination = json["destination"] as? String
        mode = json["mode"] as? String
    }

    /// A rule for this session or for this project and user only. Anything
    /// else (a permission mode, a rule shared with the team or for every
    /// project) should be offered only where the user can read it in full.
    var isNarrow: Bool {
        guard type == "addRules" || type == "replaceRules" else { return false }
        return destination == "session" || destination == "localSettings"
    }

    /// Changes how Claude Code asks from now on rather than allowing a rule.
    var changesPermissionMode: Bool {
        type == "setMode"
    }
}

extension PermissionContext: Equatable {
    nonisolated static func == (lhs: PermissionContext, rhs: PermissionContext) -> Bool {
        // Compare by identity fields only (AnyCodable doesn't conform to Equatable)
        lhs.toolUseId == rhs.toolUseId &&
        lhs.toolName == rhs.toolName &&
        lhs.receivedAt == rhs.receivedAt
    }
}

/// Explicit session phases - the state machine
nonisolated enum SessionPhase: Sendable {
    /// Session is idle, waiting for user input or new activity
    case idle

    /// Claude is actively processing (running tools, generating response)
    case processing

    /// Claude has finished and is waiting for user input
    case waitingForInput

    /// A tool is waiting for user permission approval
    case waitingForApproval(PermissionContext)

    /// Context is being compacted (auto or manual)
    case compacting

    /// Session has ended
    case ended

    // MARK: - State Machine Transitions

    /// Check if a transition to the target phase is valid
    nonisolated func canTransition(to next: SessionPhase) -> Bool {
        switch (self, next) {
        // Terminal state - no transitions out
        case (.ended, _):
            return false

        // Any state can transition to ended
        case (_, .ended):
            return true

        // Idle transitions
        case (.idle, .processing):
            return true
        case (.idle, .waitingForApproval):
            return true  // Direct permission request on idle session
        case (.idle, .compacting):
            return true
        case (.idle, .waitingForInput):
            return true  // First seen via Stop/SessionStart, or a turn that ended after an interrupt

        // Processing transitions
        case (.processing, .waitingForInput):
            return true
        case (.processing, .waitingForApproval):
            return true
        case (.processing, .compacting):
            return true
        case (.processing, .idle):
            return true  // Interrupt or quick completion

        // WaitingForInput transitions
        case (.waitingForInput, .processing):
            return true
        case (.waitingForInput, .idle):
            return true  // Can become idle
        case (.waitingForInput, .compacting):
            return true
        case (.waitingForInput, .waitingForApproval):
            return true  // A request can outlive a missed or late Stop

        // WaitingForApproval transitions
        case (.waitingForApproval, .processing):
            return true  // Approved - tool will run
        case (.waitingForApproval, .idle):
            return true  // Denied or cancelled
        case (.waitingForApproval, .waitingForInput):
            return true  // Denied and Claude stopped
        case (.waitingForApproval, .waitingForApproval):
            return true  // Another tool needs approval (multiple pending permissions)

        // Compacting transitions
        case (.compacting, .processing):
            return true
        case (.compacting, .idle):
            return true
        case (.compacting, .waitingForInput):
            return true
        case (.compacting, .waitingForApproval):
            return true

        // Allow staying in same state (no-op transitions)
        default:
            return self == next
        }
    }

    /// Attempt to transition to a new phase, returns the new phase if valid
    nonisolated func transition(to next: SessionPhase) -> SessionPhase? {
        canTransition(to: next) ? next : nil
    }

    /// Whether this phase indicates active processing
    var isActive: Bool {
        switch self {
        case .processing, .compacting:
            return true
        default:
            return false
        }
    }

    /// Whether this is a waitingForApproval phase
    var isWaitingForApproval: Bool {
        if case .waitingForApproval = self {
            return true
        }
        return false
    }
}

// MARK: - Equatable

extension SessionPhase: Equatable {
    nonisolated static func == (lhs: SessionPhase, rhs: SessionPhase) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle): return true
        case (.processing, .processing): return true
        case (.waitingForInput, .waitingForInput): return true
        case (.waitingForApproval(let ctx1), .waitingForApproval(let ctx2)):
            return ctx1 == ctx2
        case (.compacting, .compacting): return true
        case (.ended, .ended): return true
        default: return false
        }
    }
}

// MARK: - Debug Description

extension SessionPhase: CustomStringConvertible {
    nonisolated var description: String {
        switch self {
        case .idle:
            return "idle"
        case .processing:
            return "processing"
        case .waitingForInput:
            return "waitingForInput"
        case .waitingForApproval(let ctx):
            return "waitingForApproval(\(ctx.toolName))"
        case .compacting:
            return "compacting"
        case .ended:
            return "ended"
        }
    }
}
