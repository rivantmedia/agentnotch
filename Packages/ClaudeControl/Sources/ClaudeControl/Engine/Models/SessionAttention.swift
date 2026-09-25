//
//  SessionAttention.swift
//  ClaudeIsland
//
//  What a session needs from the user right now: input (a permission, a
//  question, an error), a review of finished work, nothing because Claude is
//  working, or nothing at all. Derived purely from SessionState so the UI and
//  tests share one definition.
//

import Foundation

/// Why a session is blocked on the user.
nonisolated enum NeedsInputReason: Equatable, Sendable {
    /// A tool is waiting for permission (PermissionRequest or a permission_prompt notification).
    case permission(tool: String)
    /// AskUserQuestion is waiting for an answer.
    case question
    /// ExitPlanMode is waiting for plan approval.
    case planApproval
    /// An MCP server asked for input (elicitation dialog); the message if known.
    case elicitation(String?)
    /// Some other dialog is open in the terminal (agent input, worker permission,
    /// registry `waitingFor`); the detail if known.
    case dialog(String?)
    /// The turn failed (StopFailure); humanized error, e.g. "Rate limited".
    case error(String)

    /// A failed turn (StopFailure), as opposed to something to answer.
    var isError: Bool {
        if case .error = self { return true }
        return false
    }

    /// Something the user can answer from here or in the terminal (a
    /// permission, question, plan, elicitation or dialog). A failed turn is
    /// still blocked on the user (retry, switch account, /login) but has
    /// nothing to answer: lists sort it after answerable reasons, and it
    /// warrants a different colour and banner.
    var isActionable: Bool {
        !isError
    }

    /// Sort key inside "Needs you": answerable reasons first, errors last.
    var sortRank: Int {
        isError ? 1 : 0
    }

    /// Short user-facing description, e.g. "Approve Bash", "Rate limited".
    var displayText: String {
        switch self {
        case .permission(let tool):
            return tool.isEmpty ? "Needs permission" : "Approve \(tool)"
        case .question:
            return "Question for you"
        case .planApproval:
            return "Review plan"
        case .elicitation(let message):
            return message.flatMap { $0.isEmpty ? nil : $0 } ?? "Input requested"
        case .dialog(let detail):
            guard let detail, !detail.isEmpty else { return "Waiting for you" }
            return detail.prefix(1).uppercased() + detail.dropFirst()
        case .error(let message):
            return message
        }
    }

    /// Compact identifier used by the debug state dump.
    var debugDescription: String {
        switch self {
        case .permission(let tool): return "permission:\(tool)"
        case .question: return "question"
        case .planApproval: return "planApproval"
        case .elicitation: return "elicitation"
        case .dialog(let detail): return "dialog:\(detail ?? "-")"
        case .error(let message): return "error:\(message)"
        }
    }

    /// The reason for a pending tool approval: questions and plans get their own case.
    static func forApproval(toolName: String) -> NeedsInputReason {
        switch toolName {
        case "AskUserQuestion": return .question
        case "ExitPlanMode": return .planApproval
        default: return .permission(tool: toolName)
        }
    }

    /// Human text for a StopFailure `error` code.
    static func humanizedStopError(_ code: String?) -> String {
        if let kind = StopErrorKind(code: code), kind != .other {
            return kind.displayText
        }
        switch code?.lowercased() {
        case .some(let other) where !other.isEmpty && other != "unknown":
            let words = other.replacingOccurrences(of: "_", with: " ")
            return words.prefix(1).uppercased() + words.dropFirst()
        default:
            return "Turn failed"
        }
    }
}

/// Why a turn failed (StopFailure `error`), grouped by what the user can do.
nonisolated enum StopErrorKind: String, Sendable, CaseIterable {
    /// The account hit a usage limit; the session can go on after the reset.
    case rateLimit
    /// Anthropic's side was busy or failed; retrying may work.
    case overloaded
    case serverError
    /// The account needs /login, or can't be used (org not allowed, on hold).
    case authentication
    case billing
    /// The request itself was refused (invalid, unknown model, output limit).
    case invalidRequest
    case maxOutputTokens
    case other

    init?(code: String?) {
        guard let code = code?.lowercased(), !code.isEmpty else { return nil }
        switch code {
        case "rate_limit": self = .rateLimit
        case "overloaded": self = .overloaded
        case "server_error": self = .serverError
        case "authentication_failed", "oauth_org_not_allowed", "account_on_hold", "cloud_credential_error":
            self = .authentication
        case "billing_error": self = .billing
        case "invalid_request", "model_not_found": self = .invalidRequest
        case "max_output_tokens": self = .maxOutputTokens
        default: self = .other
        }
    }

    var displayText: String {
        switch self {
        case .rateLimit: return "Rate limited"
        case .overloaded: return "Overloaded"
        case .serverError: return "Server error"
        case .authentication: return "Sign-in failed"
        case .billing: return "Billing problem"
        case .invalidRequest: return "Invalid request"
        case .maxOutputTokens: return "Output limit reached"
        case .other: return "Turn failed"
        }
    }

    /// Goes away by itself (a limit resets, a busy API recovers): retrying
    /// later works without the user fixing anything.
    var isTransient: Bool {
        switch self {
        case .rateLimit, .overloaded, .serverError: return true
        default: return false
        }
    }
}

/// Grouping for the session list; lower sorts first.
nonisolated enum AttentionBucket: Int, Comparable, Sendable, CaseIterable {
    case needsInput = 0
    case readyForReview = 1
    case working = 2
    case idle = 3

    static func < (lhs: AttentionBucket, rhs: AttentionBucket) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

/// What a session needs from the user.
nonisolated enum SessionAttention: Equatable, Sendable {
    case needsInput(NeedsInputReason)
    case working
    /// Claude finished a turn the user hasn't looked at yet.
    case readyForReview
    case idle

    var bucket: AttentionBucket {
        switch self {
        case .needsInput: return .needsInput
        case .readyForReview: return .readyForReview
        case .working: return .working
        case .idle: return .idle
        }
    }

    var needsInputReason: NeedsInputReason? {
        if case .needsInput(let reason) = self { return reason }
        return nil
    }

    /// Blocked by a failed turn rather than by something to answer.
    var isError: Bool {
        needsInputReason?.isError == true
    }

    /// Compact identifier used by the debug state dump.
    var debugDescription: String {
        switch self {
        case .needsInput(let reason): return "needsInput(\(reason.debugDescription))"
        case .working: return "working"
        case .readyForReview: return "readyForReview"
        case .idle: return "idle"
        }
    }

    /// Pure attention derivation, in priority order:
    /// 1. a pending approval (question / plan / permission),
    /// 2. an explicit needs-input reason (notification, registry, StopFailure),
    /// 3. processing or compacting, or a Stop not yet confirmed as the end
    ///    of the turn (`completionPending`: Claude Code still runs its Stop
    ///    hooks, and a blocking one such as /goal continues the turn), or a
    ///    turn that ended waiting on background agents or workflows that
    ///    will wake Claude when they finish (`awaitingBackgroundAgents`),
    /// 4. a completed turn not reviewed since it completed,
    /// 5. idle.
    static func derive(
        phase: SessionPhase,
        needsInputReason: NeedsInputReason?,
        backgroundTaskCount: Int,
        completedAt: Date?,
        reviewedAt: Date?,
        completionPending: Bool = false,
        awaitingBackgroundAgents: Bool = false
    ) -> SessionAttention {
        if case .waitingForApproval(let context) = phase {
            return .needsInput(.forApproval(toolName: context.toolName))
        }
        if let needsInputReason {
            return .needsInput(needsInputReason)
        }
        switch phase {
        case .processing, .compacting:
            return .working
        default:
            break
        }
        if completionPending || awaitingBackgroundAgents {
            return .working
        }
        // A turn that ended with only shells or monitors still running (a dev
        // server, a watcher) is finished work to look at: treating it as
        // "working" would hide it from the review queue for as long as the
        // task lives. The UI shows the running count as a detail instead.
        let turnEnded = phase == .waitingForInput || phase == .idle
        if turnEnded, let completedAt, reviewedAt.map({ $0 < completedAt }) ?? true {
            return .readyForReview
        }
        return .idle
    }
}
