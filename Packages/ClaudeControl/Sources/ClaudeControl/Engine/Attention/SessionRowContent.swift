//
//  SessionRowContent.swift
//  ClaudeControl
//
//  What a session row says and offers: its status mark, the elapsed-time
//  label, the detail line for its attention state, the inline actions, the
//  keyboard context and the VoiceOver sentence. Pure functions of the session
//  (and the clock) so the wording rules are unit-tested and shared by the
//  panel, the chat and the snapshots.
//

import Foundation

/// The row's leading status mark.
nonisolated enum SessionGlyphKind: Equatable, Sendable {
    /// Blocked on the user: half a ring in watch yellow.
    case needsInput
    /// The turn failed (rate limit, overload…): a critical dot.
    case error
    /// Finished, not yet reviewed: a full ring in ample green.
    case review
    /// Claude is running: the turning arc.
    case working
    /// Nothing going on: a grey ring.
    case idle

    init(_ attention: SessionAttention) {
        switch attention {
        case .needsInput(let reason): self = reason.isError ? .error : .needsInput
        case .readyForReview: self = .review
        case .working: self = .working
        case .idle: self = .idle
        }
    }

    /// What VoiceOver says for the mark.
    var spokenState: String {
        switch self {
        case .needsInput: return "Needs you"
        case .error: return "Failed"
        case .review: return "Ready for review"
        case .working: return "Working"
        case .idle: return "Idle"
        }
    }
}

/// Colour role of detail text; the view maps roles to the theme.
nonisolated enum SessionDetailTone: Equatable, Sendable {
    /// Readable body text (review previews, plan, dialogs).
    case primary
    /// Secondary text (idle previews, "Thinking…").
    case secondary
    /// A failed turn.
    case error
}

/// The row's second line.
nonisolated enum SessionDetail: Equatable, Sendable {
    /// A tool name and its input. `isAttention` marks a permission request:
    /// the name takes the needs-you colour and the input is shown whole, in
    /// monospace, wrapped over a few lines rather than cut.
    case tool(name: String, input: String?, isAttention: Bool)
    /// A short lead-in ("Asks") followed by text.
    case prompt(label: String, text: String)
    /// Plain text.
    case text(String, tone: SessionDetailTone, lineLimit: Int)

    /// The whole line as one string (compact rows, VoiceOver).
    var plainText: String {
        switch self {
        case let .tool(name, input, _): return [name, input].compactMap { $0 }.joined(separator: " ")
        case let .prompt(label, text): return "\(label) \(text)"
        case let .text(text, _, _): return text
        }
    }
}

/// "Always allow" as the row offers it.
nonisolated struct AlwaysAllowOffer: Equatable, Sendable {
    /// What it saves and where, e.g. "Don't ask again for Bash(npm run
    /// test:*) in this project (just you)". Nil when the suggestion has a
    /// shape we can't describe.
    let description: String?
    /// Whether the row itself shows the button. Only for a rule saved for
    /// this session or this project on this machine: a switch of permission
    /// mode, or a rule written to every project or the shared project
    /// settings, is offered in the chat only, next to its full description.
    let isInline: Bool
}

/// Inline actions that answer a session from the row itself. Each carries
/// the request it was built for, so a click can never answer a request that
/// replaced it after the row was drawn.
nonisolated enum SessionPrimaryActions: Equatable, Sendable {
    case none
    /// Deny / Allow, plus "Always" when Claude Code offered a rule to save.
    /// `needsReview`: the request is too long to judge from the row, so
    /// Allow becomes "Review…" (the chat shows it whole).
    case permission(toolUseId: String, always: AlwaysAllowOffer?, needsReview: Bool)
    /// One single-choice question with a few options: answer with a chip.
    case questionChips(toolUseId: String, question: ChatQuestion)
    /// A question too rich for chips: "Answer…" opens the chat.
    case answerInChat(toolUseId: String)
    /// ExitPlanMode: "Review plan" (chat) and "Approve".
    case plan(toolUseId: String)
    /// Waiting on a dialog only the terminal can answer.
    case answerInTerminal

    /// Whether the row shows its action bar under the text.
    var hasActionBar: Bool { self != .none }

    /// Allow is offered in the row itself, so the row must show the
    /// request whole: never a line cut off beside an Allow button.
    var allowsFromRow: Bool {
        if case .permission(_, _, needsReview: false) = self { return true }
        return false
    }

    /// The pending request these actions answer.
    var toolUseId: String? {
        switch self {
        case .permission(let id, _, _), .questionChips(let id, _), .answerInChat(let id), .plan(let id):
            return id
        case .none, .answerInTerminal:
            return nil
        }
    }
}

nonisolated enum SessionRowContent {
    static func glyph(for session: SessionState) -> SessionGlyphKind {
        SessionGlyphKind(session.attention)
    }

    // MARK: Elapsed

    /// The row's time label: how long it has waited (needs you), how long the
    /// turn has run (working), when it finished ("5m ago", review) or when it
    /// was last active (idle). Nil when there's nothing meaningful to show.
    static func elapsed(for session: SessionState, now: Date) -> String? {
        switch session.attention.bucket {
        case .needsInput:
            return UsageFormatter.duration(now.timeIntervalSince(SessionSections.waitingSince(session)))
        case .working:
            guard let started = session.turnStartedAt else { return nil }
            return UsageFormatter.duration(now.timeIntervalSince(started))
        case .readyForReview:
            guard let completed = session.completedAt else { return nil }
            return UsageFormatter.age(of: completed, now: now)
        case .idle:
            return UsageFormatter.age(of: session.lastActivity, now: now)
        }
    }

    // MARK: Detail line

    /// The second line for the session's attention state.
    ///
    /// - Parameter rateLimit: The limit that is exhausted on the session's
    ///   account, if any (`RateLimitReset.current`); named on a "Rate
    ///   limited" row with its reset time.
    static func detail(
        for session: SessionState,
        rateLimit: RateLimitReset? = nil,
        now: Date = Date(),
        home: String = AccountPaths.homeDirectory
    ) -> SessionDetail {
        switch session.attention {
        case .needsInput(let reason):
            return needsInputDetail(reason, session: session, rateLimit: rateLimit, now: now, home: home)
        case .working:
            return workingDetail(session)
        case .readyForReview:
            let message = nonEmpty(session.lastAssistantMessage) ?? nonEmpty(session.lastMessage) ?? "Finished"
            return .text(collapseWhitespace(message), tone: .primary, lineLimit: 2)
        case .idle:
            return idleDetail(session)
        }
    }

    private static func needsInputDetail(
        _ reason: NeedsInputReason,
        session: SessionState,
        rateLimit: RateLimitReset?,
        now: Date,
        home: String
    ) -> SessionDetail {
        switch reason {
        case .permission(let tool):
            if let permission = session.activePermission {
                return .tool(
                    name: MCPToolFormatter.formatToolName(permission.toolName),
                    input: PermissionPreview.text(toolName: permission.toolName, toolInput: permission.toolInput, home: home)
                        ?? permission.formattedInput,
                    isAttention: true
                )
            }
            // A prompt seen only through a notification or the registry:
            // it can be answered in the terminal, not from here.
            return .tool(
                name: tool.isEmpty ? "Permission" : MCPToolFormatter.formatToolName(tool),
                input: "waiting in the terminal",
                isAttention: true
            )
        case .question:
            let question = ChatQuestion.parse(toolInput: session.activePermission?.toolInput).first?.question
            return .prompt(label: "Asks", text: question.map(collapseWhitespace) ?? "A question for you")
        case .planApproval:
            return .text("Plan ready for approval", tone: .primary, lineLimit: 1)
        case .elicitation, .dialog:
            return .text(collapseWhitespace(reason.displayText), tone: .primary, lineLimit: 1)
        case .error(let message):
            var text = message
            if message == NeedsInputReason.humanizedStopError("rate_limit"), let rateLimit {
                text += " · \(rateLimit.phrase(now: now))"
            }
            return .text(text, tone: .error, lineLimit: 1)
        }
    }

    private static func workingDetail(_ session: SessionState) -> SessionDetail {
        if session.phase == .compacting {
            return .text("Compacting context…", tone: .secondary, lineLimit: 1)
        }
        if let task = session.tasks.activeItem {
            return .text(collapseWhitespace(task.activeLabel), tone: .secondary, lineLimit: 1)
        }
        if session.lastMessageRole == "tool", let tool = session.lastToolName, !tool.isEmpty {
            return .tool(
                name: MCPToolFormatter.formatToolName(tool),
                input: nonEmpty(session.lastMessage).map(collapseWhitespace),
                isAttention: false
            )
        }
        return .text("Thinking…", tone: .secondary, lineLimit: 1)
    }

    private static func idleDetail(_ session: SessionState) -> SessionDetail {
        switch session.lastMessageRole {
        case "tool":
            if let tool = session.lastToolName, !tool.isEmpty {
                return .tool(
                    name: MCPToolFormatter.formatToolName(tool),
                    input: nonEmpty(session.lastMessage).map(collapseWhitespace),
                    isAttention: false
                )
            }
        case "user":
            if let message = nonEmpty(session.lastMessage) {
                return .text("You: \(collapseWhitespace(message))", tone: .secondary, lineLimit: 1)
            }
        default:
            break
        }
        let message = nonEmpty(session.lastMessage) ?? nonEmpty(session.lastAssistantMessage)
        return .text(message.map(collapseWhitespace) ?? "No messages yet", tone: .secondary, lineLimit: 1)
    }

    // MARK: Actions

    /// Inline actions for a session blocked on something the panel can
    /// answer, or that only the terminal can.
    static func primaryActions(for session: SessionState, home: String = AccountPaths.homeDirectory) -> SessionPrimaryActions {
        guard let permission = session.activePermission else {
            if let reason = session.attention.needsInputReason, !reason.isError {
                return .answerInTerminal
            }
            return .none
        }
        switch permission.toolName {
        case "AskUserQuestion":
            if let question = ChatQuestion.inline(ChatQuestion.parse(toolInput: permission.toolInput)) {
                return .questionChips(toolUseId: permission.toolUseId, question: question)
            }
            return .answerInChat(toolUseId: permission.toolUseId)
        case "ExitPlanMode":
            return .plan(toolUseId: permission.toolUseId)
        default:
            let preview = PermissionPreview.text(toolName: permission.toolName, toolInput: permission.toolInput,
                                                 home: home)
            return .permission(
                toolUseId: permission.toolUseId,
                always: alwaysAllowOffer(permission.permissionSuggestions),
                needsReview: PermissionPreview.isTooLongToReviewInline(preview)
            )
        }
    }

    /// "Always allow" for Claude Code's first `permission_suggestions` entry
    /// (the terminal's "Yes, and don't ask again"), or nil when none came.
    static func alwaysAllowOffer(_ suggestions: [AnyCodable]?) -> AlwaysAllowOffer? {
        guard let first = suggestions?.first else { return nil }
        let description = PermissionSuggestionText.describe(suggestions)
        guard let dict = first.value as? [String: Any] else {
            return AlwaysAllowOffer(description: description, isInline: false)
        }
        let type = dict["type"] as? String
        let destination = dict["destination"] as? String
        let isNarrowRule = (type == "addRules" || type == "replaceRules")
            && (destination == nil || destination == "session" || destination == "localSettings")
        return AlwaysAllowOffer(description: description, isInline: isNarrowRule && description != nil)
    }

    // MARK: Meta line

    /// "2 background" wording without the glyph.
    static func backgroundLabel(count: Int) -> String? {
        count > 0 ? "\(count) background" : nil
    }

    // MARK: Compact rows

    /// The short text a one-line row shows after the title: what Claude is
    /// on, what it asks, or the project.
    static func compactDetail(for session: SessionState, rateLimit: RateLimitReset?, now: Date) -> String {
        switch session.attention {
        case .working:
            return detail(for: session, now: now).plainText
        case .readyForReview, .idle:
            return session.displayProjectName
        case .needsInput:
            return detail(for: session, rateLimit: rateLimit, now: now).plainText
        }
    }

    // MARK: Accessibility

    /// One sentence for VoiceOver: title, state, what it waits on, the time
    /// and the account. Never the assistant's message.
    static func accessibilityLabel(
        for session: SessionState,
        accountLabel: String?,
        rateLimit: RateLimitReset?,
        now: Date
    ) -> String {
        var parts = [session.displayTitle, glyph(for: session).spokenState]
        switch session.attention {
        case .needsInput, .working:
            parts.append(detail(for: session, rateLimit: rateLimit, now: now).plainText)
        case .readyForReview, .idle:
            break
        }
        if let elapsed = elapsed(for: session, now: now) {
            switch session.attention.bucket {
            case .needsInput: parts.append("waiting \(elapsed)")
            case .working: parts.append("running \(elapsed)")
            case .readyForReview: parts.append("finished \(elapsed)")
            case .idle: parts.append("last active \(elapsed)")
            }
        }
        if !session.tasks.isEmpty {
            parts.append(taskSummary(session.tasks))
        }
        if let accountLabel {
            parts.append("account \(accountLabel)")
        }
        return parts.joined(separator: ", ")
    }

    /// "3 of 7 tasks done, now: Writing tests".
    static func taskSummary(_ tasks: SessionTaskList) -> String {
        var text = "\(tasks.completedCount) of \(tasks.totalCount) tasks done"
        if let active = tasks.activeItem {
            text += ", now: \(active.activeLabel)"
        }
        return text
    }

    // MARK: Helpers

    private static func nonEmpty(_ value: String?) -> String? {
        guard let value, !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return value
    }

    /// Newlines and runs of spaces become single spaces, so previews stay on
    /// their line limit instead of breaking early.
    static func collapseWhitespace(_ text: String) -> String {
        text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }
}

// MARK: - Permission preview

/// A permission request as the row shows it: the whole command, not the
/// engine's 100-character cut, and a full path (under `~`) rather than the
/// file name alone, so nothing that matters hides past the edge.
nonisolated enum PermissionPreview {
    /// Lines the row gives the request before sending it to the chat.
    static let inlineLineLimit = 4
    /// About as many characters as `inlineLineLimit` lines hold at the
    /// narrowest panel width.
    static let inlineCharacterLimit = 200

    static func text(toolName: String, toolInput: [String: AnyCodable]?, home: String) -> String? {
        guard let input = toolInput else { return nil }
        func string(_ key: String) -> String? {
            (input[key]?.value as? String).flatMap { $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : $0 }
        }
        if toolName == "Bash", let command = string("command") {
            return command.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        for key in ["file_path", "notebook_path", "path"] {
            if let path = string(key) {
                return AccountPathDisplay.abbreviated(path, home: home)
            }
        }
        for key in ["command", "url", "query", "pattern"] {
            if let value = string(key) { return value }
        }
        let firstString = input.keys.sorted().lazy
            .filter { $0 != "description" }
            .compactMap { string($0) }
            .first
        return firstString
    }

    /// Whether the request is too long to approve from the row.
    static func isTooLongToReviewInline(_ text: String?) -> Bool {
        guard let text else { return false }
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).count
        return lines > inlineLineLimit || text.count > inlineCharacterLimit
    }
}

// MARK: - Rate limits

/// The limit that stopped a rate-limited session, and when it lifts.
nonisolated struct RateLimitReset: Equatable, Sendable {
    /// "5-hour limit", "weekly limit", "Opus weekly limit".
    let window: String
    let resetsAt: Date

    /// "weekly limit resets Thu 09:00", "5-hour limit resets in 47m".
    func phrase(now: Date) -> String {
        guard let reset = UsageFormatter.resetPhrase(resetsAt, now: now) else { return window }
        return "\(window) \(reset)"
    }

    /// The exhausted window of an account's reading with the latest reset,
    /// or nil when no window is at 100% (then the row says only "Rate
    /// limited"). A stale reading still counts for a window at 100% whose
    /// reset is ahead: usage cannot drop before its window resets.
    static func current(in reading: ClaudeRingReading?, now: Date) -> RateLimitReset? {
        guard let reading else { return nil }
        let exhausted = reading.windows.filter { window in
            guard window.money == nil, window.usedFraction >= 1, let resetsAt = window.resetsAt else { return false }
            return resetsAt > now
        }
        guard let latest = exhausted.max(by: { ($0.resetsAt ?? .distantPast) < ($1.resetsAt ?? .distantPast) }),
              let resetsAt = latest.resetsAt else { return nil }
        return RateLimitReset(window: name(of: latest), resetsAt: resetsAt)
    }

    private static func name(of window: ClaudeRingReading.Window) -> String {
        switch window.id {
        case "session": return "5-hour limit"
        case "weekly_all": return "weekly limit"
        default:
            let model = window.label ?? window.id
                .replacingOccurrences(of: "weekly_", with: "")
                .replacingOccurrences(of: "_", with: " ")
                .capitalized
            return "\(model) weekly limit"
        }
    }
}

// MARK: - Paths

/// Paths under the home folder as `~/…`: the engine's one abbreviator
/// (`AccountPathDisplayName`), under the name the panel has always used.
typealias AccountPathDisplay = AccountPathDisplayName
