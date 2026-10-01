//
//  ChatSessionHeader.swift
//  ClaudeControl
//
//  The chat's header: back and the title, what Claude is on (or the
//  project), then at a glance the task progress and time left (a click
//  opens the task board: percent done, time left, and how long each task
//  took or has run), context use, the account and a button that brings up
//  the session's terminal.
//

import SwiftUI

struct ChatSessionHeader: View {
    let title: String
    /// Second line: the task Claude is on, else the project folder.
    let subtitle: String
    /// `subtitle` is live activity rather than the project name.
    var subtitleIsActivity = false
    /// Shown only when several accounts are in use.
    let account: AccountTagModel?
    let tasks: SessionTaskList
    /// A working session's progress and time left (SessionRowContent.taskEstimate).
    var taskEstimate: TaskEstimate? = nil
    /// The panel's clock, for how long the task in progress has run.
    var now = Date()
    let contextPercent: Double?
    let canFocus: Bool
    let focusLabel: String
    @Binding var isTaskBoardOpen: Bool
    /// Rows the task board shows before it scrolls; fewer while an answer
    /// bar needs the room below.
    var maxTaskRows = ChatTaskBoard.regularRows
    let onBack: () -> Void
    let onFocus: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: theme.blockSpacing) {
            HStack(alignment: .center, spacing: 6) {
                ClaudeIconButton(systemName: "chevron.left", label: "Back to sessions (Esc)", action: onBack)
                    .padding(.leading, -6)

                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .claudeFont(.rowTitle, weight: .semibold)
                        .foregroundStyle(.ink(.primary))
                        .lineLimit(1)
                        .accessibilityAddTraits(.isHeader)
                    HStack(spacing: 6) {
                        if let account {
                            AccountTag(label: account.label, colorIndex: account.colorIndex)
                                .fixedSize()
                            Text("·")
                                .claudeFont(.caption, weight: .bold)
                                .foregroundStyle(.ink(.tertiary))
                        }
                        Text(subtitle)
                            .claudeFont(.caption)
                            .foregroundStyle(.ink(subtitleIsActivity ? .primary : .secondary))
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                HStack(spacing: 8) {
                    if !tasks.isEmpty {
                        ChatTaskSummaryButton(tasks: tasks, estimate: taskEstimate, isOpen: isTaskBoardOpen) {
                            isTaskBoardOpen.toggle()
                        }
                    }
                    if let contextPercent {
                        ContextMeter(percent: contextPercent, isCompact: true)
                    }
                    if canFocus {
                        ClaudeIconButton(systemName: "arrow.up.forward.app", label: "\(focusLabel) (⌘J)", action: onFocus)
                    }
                }
                .fixedSize()
            }

            if isTaskBoardOpen && !tasks.isEmpty {
                ChatTaskBoard(tasks: tasks, estimate: taskEstimate, now: now, maxVisibleRows: maxTaskRows)
                    .transition(.opacity)
            }
        }
        .padding(.horizontal, theme.padding)
        .padding(.vertical, 9)
        .claudeAnimation(ClaudeMotion.quick, value: isTaskBoardOpen)
    }
}

// MARK: - Task summary

/// "▬▬▭ 3/7 ~4m ⌄": toggles the task board.
struct ChatTaskSummaryButton: View {
    let tasks: SessionTaskList
    var estimate: TaskEstimate? = nil
    let isOpen: Bool
    let action: () -> Void

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                TaskProgressBar(tasks: tasks, estimate: estimate, width: 30, remainingStyle: .short)
                Image(systemName: "chevron.down")
                    .font(.system(size: 7, weight: .bold))
                    .foregroundStyle(.ink(isHovered || isOpen ? .primary : .tertiary))
                    .rotationEffect(.degrees(isOpen ? 180 : 0))
            }
            .padding(.horizontal, 6)
            .frame(height: theme.controlHeight - 2)
            .background(Capsule().fill(isHovered || isOpen ? theme.controlFill : .clear))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
        .help(Self.help(tasks, estimate: estimate))
        .accessibilityLabel(SessionRowContent.taskSummary(tasks, estimate: estimate))
        .accessibilityHint(isOpen ? "Hides the task list" : "Shows the task list")
    }

    /// "Now: Writing tests · 46% · ~4m left", "Tasks".
    nonisolated static func help(_ tasks: SessionTaskList, estimate: TaskEstimate?) -> String {
        var parts = tasks.activeItem.map { ["Now: \($0.activeLabel)"] } ?? []
        if let estimate {
            parts.append("\(estimate.percent)%")
            if let remaining = estimate.remaining { parts.append(remaining) }
        }
        return parts.isEmpty ? "Tasks" : parts.joined(separator: " · ")
    }
}

/// Every task with its state (done, in progress, to do) and how long it
/// took or has run, under "3 of 7 done · 46% · ~4m left".
struct ChatTaskBoard: View {
    let tasks: SessionTaskList
    /// A working session's progress and time left; nil shows completed
    /// tasks only, and no running time for the task in progress.
    var estimate: TaskEstimate? = nil
    var now = Date()
    var maxVisibleRows = Self.regularRows

    static let regularRows = 8
    /// While a question, plan or approval bar needs the room.
    static let compactRows = 4

    private let rowHeight: CGFloat = 17

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            SplitLine(leading: "Tasks", trailing: Self.headline(tasks, estimate: estimate))
            ChatAdaptiveScroll(maxHeight: CGFloat(maxVisibleRows) * (rowHeight + 2)) {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(tasks.items) { item in
                        ChatTaskRow(item: item, time: Self.time(of: item, isRunning: estimate != nil, now: now))
                            .frame(minHeight: rowHeight)
                    }
                }
            }
        }
        .padding(9)
        .background(RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous).fill(theme.controlFill))
    }
}

extension ChatTaskBoard {
    /// "3 of 7 done · 42%", with the time left once there is a pace:
    /// "3 of 7 done · 46% · ~4m left".
    nonisolated static func headline(_ tasks: SessionTaskList, estimate: TaskEstimate?) -> String {
        let percent = estimate?.percent ?? Int((tasks.fraction * 100 + 1e-9).rounded(.down))
        var parts = ["\(tasks.completedCount) of \(tasks.totalCount) done", "\(percent)%"]
        if let remaining = estimate?.remaining { parts.append(remaining) }
        return parts.joined(separator: " · ")
    }

    /// How long a completed task took ("4m", "<1m"), or how long the task in
    /// progress has run while the session works; nil when a time is unknown.
    nonisolated static func time(of item: SessionTaskItem, isRunning: Bool, now: Date) -> String? {
        guard item.status == .completed || isRunning else { return nil }
        return item.duration(now: now).map { UsageFormatter.duration($0) }
    }
}

private struct ChatTaskRow: View {
    let item: SessionTaskItem
    /// "4m": how long it took, or has run.
    var time: String?

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(spacing: 7) {
            mark
                .frame(width: 8, height: 8)
            Text(item.status == .inProgress ? item.activeLabel : item.subject)
                .claudeFont(.body, weight: item.status == .inProgress ? .medium : .regular)
                .foregroundStyle(.ink(item.status == .pending ? .secondary : .primary))
                .strikethrough(item.status == .completed, color: theme.textSecondary)
                .opacity(item.status == .completed ? 0.55 : 1)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 0)
            if let time {
                Text(time)
                    .claudeFont(.caption, monospacedDigits: true)
                    .foregroundStyle(.ink(item.status == .inProgress ? .secondary : .tertiary))
                    .fixedSize()
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(time.map { "\(item.subject), \(item.status.spoken), \($0)" } ?? "\(item.subject), \(item.status.spoken)")
    }

    @ViewBuilder
    private var mark: some View {
        switch item.status {
        case .completed:
            StatusArc(trim: 1)
                .stroke(theme.textPrimary, style: StrokeStyle(lineWidth: 1.3))
                .padding(0.65)
                .overlay(Circle().fill(theme.textPrimary).padding(2.2))
        case .inProgress:
            ArcSpinner(color: theme.working, lineWidth: 1.3)
        case .pending:
            StatusArc(trim: 1)
                .stroke(theme.textSecondary, style: StrokeStyle(lineWidth: 1.3))
                .padding(0.65)
        }
    }
}

extension SessionTaskItem.Status {
    var spoken: String {
        switch self {
        case .completed: return "done"
        case .inProgress: return "in progress"
        case .pending: return "to do"
        }
    }
}

// MARK: - Split line

/// A label on the left and a quieter value on the right: the row shape
/// Codenotch's cards use throughout.
struct SplitLine: View {
    let leading: String
    let trailing: String
    var leadingToken: ClaudeInk.Token = .primary
    var trailingToken: ClaudeInk.Token = .secondary

    var body: some View {
        HStack(spacing: 8) {
            Text(leading).foregroundStyle(.ink(leadingToken))
            Spacer(minLength: 0)
            Text(trailing).foregroundStyle(.ink(trailingToken)).monospacedDigit()
        }
        .claudeFont(.caption)
        .lineLimit(1)
    }
}

// MARK: - Adaptive scroll

/// A vertical scroll view exactly as tall as its content, up to
/// `maxHeight`. (A bare ScrollView in a VStack takes every point it is
/// offered.) Snapshots draw the content directly.
struct ChatAdaptiveScroll<Content: View>: View {
    let maxHeight: CGFloat
    @ViewBuilder let content: Content

    @Environment(\.claudeStaticRendering) private var isStatic
    @State private var contentHeight: CGFloat = 0

    var body: some View {
        if isStatic {
            content
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxHeight: maxHeight, alignment: .top)
                .fixedSize(horizontal: false, vertical: true)
                .clipped()
        } else {
            ScrollView(.vertical) {
                content.measuredHeight(into: $contentHeight)
            }
            .scrollIndicators(.automatic)
            .frame(height: min(max(contentHeight, 1), maxHeight))
            .scrollBounceBehavior(.basedOnSize)
        }
    }
}
