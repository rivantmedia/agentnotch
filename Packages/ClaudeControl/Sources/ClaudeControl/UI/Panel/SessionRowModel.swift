//
//  SessionRowModel.swift
//  ClaudeControl
//
//  Everything one row of the sessions list draws, as a small Equatable
//  value built once per update. The row compares these instead of whole
//  `SessionState`s (which carry the chat history), so a publish that changes
//  one session redraws one row, and nothing in a row's body asks the system
//  anything (no process or LaunchServices lookups while drawing).
//

import Foundation

nonisolated struct SessionRowModel: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let glyph: SessionGlyphKind
    let bucket: AttentionBucket
    /// "2m", "5m ago"; nil when there is nothing meaningful.
    let elapsed: String?
    let detail: SessionDetail
    /// The short line a one-line row shows after the title.
    let compactDetail: String
    let actions: SessionPrimaryActions
    let tasks: SessionTaskList
    let contextPercent: Double?
    let projectName: String
    let backgroundTasks: Int
    /// The account's name and colour, when more than one account is in use.
    let account: AccountTagModel?
    let canFocus: Bool
    /// "Show in editor" for sessions in VS Code, else "Show terminal".
    let focusLabel: String
    let accessibilityLabel: String
    /// The turn stopped on an error: "Dismiss" clears it (GUX-2).
    var isFailed: Bool = false
    /// A working session's progress with credit for the task in progress and
    /// the time left; nil otherwise (the bar then counts completed tasks only).
    var taskEstimate: TaskEstimate? = nil

    /// Ready for review: "Mark reviewed" means something.
    var isReviewable: Bool { bucket == .readyForReview }

    /// Whether the meta line (tasks · context · project · background) has
    /// anything the other lines don't already say.
    var showsMetaLine: Bool {
        !tasks.isEmpty || contextPercent != nil || backgroundTasks > 0 || projectName != title
    }

    /// What the keyboard can do with this row.
    var keyTarget: ClaudeKeyRouter.Target {
        var target = ClaudeKeyRouter.Target(sessionId: id, actions: actions, canMarkReviewed: isReviewable, canJump: canFocus)
        target.canDismissFailure = isFailed
        return target
    }

    static func make(
        _ session: SessionState,
        account: AccountTagModel?,
        rateLimit: RateLimitReset?,
        canFocus: Bool,
        now: Date,
        home: String
    ) -> SessionRowModel {
        let detail = SessionRowContent.detail(for: session, rateLimit: rateLimit, now: now, home: home)
        return SessionRowModel(
            id: session.sessionId,
            title: session.displayTitle,
            glyph: SessionRowContent.glyph(for: session),
            bucket: session.attention.bucket,
            elapsed: SessionRowContent.elapsed(for: session, now: now),
            detail: detail,
            compactDetail: SessionRowContent.compactDetail(for: session, rateLimit: rateLimit, now: now),
            actions: SessionRowContent.primaryActions(for: session, home: home),
            tasks: session.tasks,
            contextPercent: session.contextUsedPercent,
            projectName: session.displayProjectName,
            backgroundTasks: session.backgroundTaskCount,
            account: account,
            canFocus: canFocus,
            focusLabel: session.entrypoint == "claude-vscode" ? "Show in editor" : "Show terminal",
            accessibilityLabel: SessionRowContent.accessibilityLabel(
                for: session, accountLabel: account?.label, rateLimit: rateLimit, now: now
            ),
            isFailed: session.hasFailedTurn,
            taskEstimate: SessionRowContent.taskEstimate(for: session, now: now)
        )
    }
}

/// An account as a row or chip names it: never the colour alone.
nonisolated struct AccountTagModel: Equatable, Hashable, Sendable {
    let label: String
    let colorIndex: Int
}
