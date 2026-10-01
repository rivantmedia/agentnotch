//
//  SessionRow.swift
//  ClaudeControl
//
//  One session in the list, in Codenotch's hover-card vocabulary: the title
//  in primary ink on the left, the status ring and elapsed time in the
//  state's colour on the right, the detail in secondary ink below, then a
//  meta line (account · tasks · context · project) and, when the session
//  waits on an answer, the answer itself.
//
//      Fix the login redirect loop                              ◐ 2m
//      Bash  npm run test -- --watch=false auth/redirect.spec.ts
//      ● Work · ▬▬▬▭▭▭ 3/7 · ~4m left · ▬ 42% context · acme-web
//                                         Deny   Always   Allow
//
//  Past eight sessions, rows that don't need an answer collapse to one line.
//  Mark reviewed, show terminal and open chat appear under the pointer or
//  the keyboard selection, and are VoiceOver actions on every row.
//

import SwiftUI

struct SessionRow: View, Equatable {
    let row: SessionRowModel
    let isCompact: Bool
    let isSelected: Bool
    /// The answer the row shows can't be taken yet (just replaced, or
    /// already sent).
    let isAnswerArmed: Bool
    /// Draw the pointer's actions without a pointer (snapshots).
    var forcesHover = false
    let perform: (ClaudeKeyRouter.Command) -> Void

    static func == (lhs: SessionRow, rhs: SessionRow) -> Bool {
        lhs.row == rhs.row && lhs.isCompact == rhs.isCompact && lhs.isSelected == rhs.isSelected
            && lhs.isAnswerArmed == rhs.isAnswerArmed && lhs.forcesHover == rhs.forcesHover
    }

    @Environment(\.claudeControlTheme) private var theme
    @State private var isHovered = false

    private var showsRowActions: Bool { isHovered || isSelected || forcesHover }
    /// "Needs you" rows never go compact: what they wait on is the point.
    private var drawsCompact: Bool { isCompact && row.bucket != .needsInput }

    var body: some View {
        Group {
            if drawsCompact {
                compactBody
            } else {
                regularBody
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, drawsCompact ? 4 : 7)
        .background(background)
        .contentShape(RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous))
        // Buttons inside take their own clicks first.
        .onTapGesture { perform(.openChat(sessionId: row.id)) }
        .onHover { isHovered = $0 }
        .claudeAnimation(ClaudeMotion.quick, value: isHovered)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(row.accessibilityLabel)
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction { perform(.openChat(sessionId: row.id)) }
        .modifier(RowAccessibilityActions(row: row, perform: perform))
    }

    private var background: some View {
        RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous)
            .fill(isSelected ? theme.rowSelection : isHovered || forcesHover ? theme.rowHover : .clear)
            .overlay {
                if isSelected {
                    RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous)
                        .strokeBorder(theme.rowSelectionStroke, lineWidth: theme.hairline)
                }
            }
    }

    // MARK: Regular

    private var regularBody: some View {
        VStack(alignment: .leading, spacing: theme.lineGap) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(row.title)
                    .claudeFont(.rowTitle)
                    .foregroundStyle(.ink(.primary))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                trailing
            }
            SessionDetailView(detail: row.detail, showsWholeRequest: row.actions.allowsFromRow)
            if row.showsMetaLine || row.account != nil {
                SessionMetaLine(row: row)
                    .padding(.top, 1)
            }
            if row.actions.hasActionBar {
                RowActionBar(sessionId: row.id, actions: row.actions, canFocus: row.canFocus, perform: perform)
                    .allowsHitTesting(isAnswerArmed)
                    .padding(.top, 3)
            }
        }
    }

    // MARK: Compact

    private var compactBody: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            // The detail only when it fits whole: a word cut to one letter
            // says nothing, and the title and progress matter more.
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    compactTitle
                    compactDetail.fixedSize()
                    Spacer(minLength: 0)
                }
                HStack(spacing: 0) {
                    compactTitle
                    Spacer(minLength: 0)
                }
            }
            if !showsRowActions {
                CompactProgress(row: row)
            }
            trailing
        }
    }

    private var compactTitle: some View {
        Text(row.title)
            .claudeFont(.body, weight: .medium)
            .foregroundStyle(.ink(.primary))
            .lineLimit(1)
            .truncationMode(.tail)
    }

    private var compactDetail: some View {
        Text(row.compactDetail)
            .claudeFont(.caption)
            .foregroundStyle(.ink(.secondary))
            .lineLimit(1)
    }

    // MARK: Trailing

    /// The ring and elapsed time, or the row's actions under the pointer.
    /// They share one slot, so the title never reflows when the pointer
    /// arrives.
    private var trailing: some View {
        ZStack(alignment: .trailing) {
            StatusTrail(glyph: row.glyph, elapsed: row.elapsed)
                .opacity(showsRowActions ? 0 : 1)
            RowHoverActions(row: row, perform: perform)
                .opacity(showsRowActions ? 1 : 0)
                .allowsHitTesting(showsRowActions)
        }
        .fixedSize()
    }
}

// MARK: - Status trail

/// "◐ 2m": the ring and the elapsed time in the state's colour, the way
/// Codenotch's hover card sets the ring beside its state word.
struct StatusTrail: View {
    let glyph: SessionGlyphKind
    let elapsed: String?

    var body: some View {
        HStack(spacing: 5) {
            StatusRing(kind: glyph)
            if let elapsed {
                Text(elapsed)
                    .claudeFont(.caption, weight: .medium, monospacedDigits: true)
                    .foregroundStyle(.ink(glyph.ink))
                    .lineLimit(1)
            }
        }
        .accessibilityHidden(true)
    }
}

extension SessionGlyphKind {
    /// The signal colour of the state.
    var ink: ClaudeInk.Token {
        switch self {
        case .needsInput: return .needsYou
        case .error: return .critical
        case .review: return .review
        case .working: return .working
        case .idle: return .secondary
        }
    }
}

// MARK: - Detail

/// The row's second line.
struct SessionDetailView: View {
    let detail: SessionDetail
    /// Allow sits right below: show the request uncut, however it wraps
    /// (anything too long for that goes to the chat instead, see
    /// `PermissionPreview.isTooLongToReviewInline`).
    var showsWholeRequest = false

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        switch detail {
        case let .tool(name, input, isAttention) where isAttention:
            VStack(alignment: .leading, spacing: 3) {
                Text(name)
                    .claudeFont(.body, weight: .semibold)
                    .foregroundStyle(.ink(.needsYou))
                if let input {
                    // The whole request, wrapped, so nothing that matters
                    // hides past the edge (too long for the row goes to chat).
                    Text(input)
                        .claudeFont(.mono)
                        .foregroundStyle(.ink(.primary))
                        .lineLimit(showsWholeRequest ? nil : PermissionPreview.inlineLineLimit)
                        .truncationMode(.tail)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 4)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(
                            RoundedRectangle(cornerRadius: 6, style: .continuous).fill(theme.controlFill)
                        )
                }
            }
        case let .tool(name, input, _):
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Text(name)
                    .claudeFont(.body, weight: .semibold)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize()
                if let input {
                    Text(input)
                        .claudeFont(.body)
                        .foregroundStyle(.ink(.secondary))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
        case let .prompt(label, text):
            (Text(label + "  ").fontWeight(.semibold).foregroundStyle(.ink(.needsYou))
                + Text(text).foregroundStyle(.ink(.primary)))
                .claudeFont(.body)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        case let .text(text, tone, lineLimit):
            Text(text)
                .claudeFont(.body)
                .foregroundStyle(.ink(tone.ink))
                .lineLimit(lineLimit)
                .truncationMode(.tail)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

extension SessionDetailTone {
    var ink: ClaudeInk.Token {
        switch self {
        case .primary: return .primary
        case .secondary: return .secondary
        case .error: return .critical
        }
    }
}

// MARK: - Meta line

/// "● Work · ▬▬▬▭▭ 3/7 · ~4m left · ▬ 42% context · acme-web · 2 background".
///
/// When the row is too narrow for all of it, the project goes first, then
/// the word "context", then the word "left"; the account, tasks, time left
/// and context percentage always stay.
struct SessionMetaLine: View {
    let row: SessionRowModel

    var body: some View {
        ViewThatFits(in: .horizontal) {
            line(project: true, contextLabel: true, remaining: .full)
            line(project: true, contextLabel: false, remaining: .full)
            line(project: false, contextLabel: false, remaining: .full)
            line(project: false, contextLabel: false, remaining: .short)
            line(project: false, contextLabel: false, remaining: .short, truncatesAccount: true)
        }
    }

    private func line(
        project showsProject: Bool,
        contextLabel: Bool,
        remaining: TaskProgressBar.RemainingStyle,
        truncatesAccount: Bool = false
    ) -> some View {
        var items: [AnyView] = []
        if let account = row.account {
            items.append(AnyView(
                AccountTag(label: account.label, colorIndex: account.colorIndex)
                    .fixedSize(horizontal: !truncatesAccount, vertical: false)
            ))
        }
        if !row.tasks.isEmpty {
            items.append(AnyView(
                TaskProgressBar(tasks: row.tasks, estimate: row.taskEstimate, remainingStyle: remaining).fixedSize()
            ))
        }
        if let context = row.contextPercent {
            items.append(AnyView(ContextMeter(percent: context, isCompact: !contextLabel).fixedSize()))
        }
        if showsProject && row.projectName != row.title {
            items.append(AnyView(
                Text(row.projectName)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                    .fixedSize()
            ))
        }
        if let background = SessionRowContent.backgroundLabel(count: row.backgroundTasks) {
            items.append(AnyView(
                Text(background)
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .fixedSize()
                    .help("\(row.backgroundTasks) background task\(row.backgroundTasks == 1 ? "" : "s") still running")
            ))
        }
        return HStack(spacing: 6) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                if index > 0 {
                    Text("·")
                        .claudeFont(.caption, weight: .bold)
                        .foregroundStyle(.ink(.tertiary))
                }
                item
            }
        }
        .lineLimit(1)
    }
}

/// "3/7 ~4m · 42%" beside a one-line row's ring.
private struct CompactProgress: View {
    let row: SessionRowModel

    var body: some View {
        HStack(spacing: 6) {
            if !row.tasks.isEmpty {
                TaskProgressBar(tasks: row.tasks, estimate: row.taskEstimate, width: 28, remainingStyle: .short)
            }
            if let context = row.contextPercent {
                ContextMeter(percent: context, isCompact: true)
            }
            if let account = row.account {
                AccountDot(colorIndex: account.colorIndex)
                    .help(account.label)
            }
        }
        .fixedSize()
    }
}

// MARK: - Pointer actions

/// ✓ mark reviewed, ↗ show terminal, 💬 open chat.
private struct RowHoverActions: View {
    let row: SessionRowModel
    let perform: (ClaudeKeyRouter.Command) -> Void

    var body: some View {
        HStack(spacing: 1) {
            if row.isReviewable {
                ClaudeIconButton(systemName: "checkmark.circle", label: "Mark reviewed (⌘R)", tint: .review) {
                    perform(.markReviewed(sessionId: row.id))
                }
            } else if row.isFailed {
                ClaudeIconButton(systemName: "xmark.circle", label: "Dismiss (⌘R)") {
                    perform(.markReviewed(sessionId: row.id))
                }
            }
            if row.canFocus {
                ClaudeIconButton(systemName: "arrow.up.forward.app", label: "\(row.focusLabel) (⌘J)") {
                    perform(.jump(sessionId: row.id))
                }
            }
            ClaudeIconButton(systemName: "bubble.left", label: "Open chat (⏎)") {
                perform(.openChat(sessionId: row.id))
            }
        }
    }
}

/// The same actions for VoiceOver, which never hovers.
private struct RowAccessibilityActions: ViewModifier {
    let row: SessionRowModel
    let perform: (ClaudeKeyRouter.Command) -> Void

    func body(content: Content) -> some View {
        content
            .accessibilityAction(named: "Open chat") { perform(.openChat(sessionId: row.id)) }
            .accessibilityActions {
                // By position: two options may share a label.
                ForEach(Array(RowAnswerChoices.accessibilityActions(for: row).enumerated()), id: \.offset) { _, choice in
                    Button(choice.label) { perform(choice.command) }
                }
            }
    }
}

/// The answers a row offers, as labelled commands (VoiceOver actions and
/// tests read the same list the action bar draws).
nonisolated struct RowAnswerChoice: Identifiable, Equatable, Sendable {
    let label: String
    let command: ClaudeKeyRouter.Command
    var id: String { label }
}

nonisolated enum RowAnswerChoices {
    static func all(for row: SessionRowModel) -> [RowAnswerChoice] {
        let id = row.id
        switch row.actions {
        case .none:
            return []
        case let .permission(toolUseId, always, needsReview):
            var choices = [RowAnswerChoice(label: "Deny", command: .deny(sessionId: id, toolUseId: toolUseId))]
            if let always, always.isInline, !needsReview {
                choices.append(RowAnswerChoice(label: "Always allow", command: .alwaysAllow(sessionId: id, toolUseId: toolUseId)))
            }
            choices.append(needsReview
                ? RowAnswerChoice(label: "Review…", command: .openChat(sessionId: id))
                : RowAnswerChoice(label: "Allow", command: .allow(sessionId: id, toolUseId: toolUseId)))
            return choices
        case let .questionChips(toolUseId, question):
            return question.options.enumerated().map { index, option in
                RowAnswerChoice(label: option.label, command: .chooseOption(sessionId: id, toolUseId: toolUseId, index: index))
            } + [RowAnswerChoice(label: "Other…", command: .openChat(sessionId: id))]
        case .answerInChat:
            return [RowAnswerChoice(label: "Answer…", command: .openChat(sessionId: id))]
        case .plan(let toolUseId):
            return [
                RowAnswerChoice(label: "Review plan", command: .openChat(sessionId: id)),
                RowAnswerChoice(label: "Approve plan", command: .approvePlan(sessionId: id, toolUseId: toolUseId)),
            ]
        case .answerInTerminal:
            return row.canFocus ? [RowAnswerChoice(label: row.focusLabel, command: .jump(sessionId: id))] : []
        }
    }

    /// Every VoiceOver action of the row besides "Open chat": its answers,
    /// then Mark reviewed and Show terminal, each once.
    static func accessibilityActions(for row: SessionRowModel) -> [RowAnswerChoice] {
        var choices = all(for: row)
        if row.isReviewable {
            choices.append(RowAnswerChoice(label: "Mark reviewed", command: .markReviewed(sessionId: row.id)))
        } else if row.isFailed {
            choices.append(RowAnswerChoice(label: "Dismiss", command: .markReviewed(sessionId: row.id)))
        }
        let jump = ClaudeKeyRouter.Command.jump(sessionId: row.id)
        // A dialog only the terminal answers offers it already.
        if row.canFocus && !choices.contains(where: { $0.command == jump }) {
            choices.append(RowAnswerChoice(label: row.focusLabel, command: jump))
        }
        return choices
    }
}
