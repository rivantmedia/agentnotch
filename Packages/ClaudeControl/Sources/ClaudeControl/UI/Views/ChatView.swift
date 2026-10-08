//
//  ChatView.swift
//  ClaudeControl
//
//  One session's conversation in the panel: a header (back, title, task
//  progress, context, account, show terminal), the transcript (markdown,
//  tool calls with their results, thinking), and one bar at the bottom, in
//  order of precedence: a permission to approve, a question to answer, a
//  plan to approve, then the composer. The composer only appears when a
//  typed reply can reach the session's terminal and Claude isn't showing a
//  dialog there: typing into a dialog would answer it.
//
//  `LiveChatView` feeds `ChatContent` from the engine; the snapshots feed it
//  fixtures.
//

import Combine
import SwiftUI

// MARK: - Live

/// The chat wired to the engine: history loading, live updates, sending.
struct LiveChatView: View {
    let session: SessionState
    let monitor: ClaudeSessionMonitor
    @ObservedObject var state: ClaudePanelState
    let canFocus: Bool
    let account: AccountTagModel?
    let hooks: ChatHooks
    /// How the session stands when Claude isn't working (the row's words).
    var statusLine: ChatStatusLine? = nil

    @State private var history: [ChatHistoryItem]
    @State private var isLoading: Bool
    @State private var agentDescriptions: [String: String] = [:]
    /// How typed messages reach the terminal (tmux / iTerm2 / Terminal); nil: they can't.
    @State private var messageRoute: MessageRoute?
    /// Why replies can't be typed from here now (the engine's words).
    @State private var unavailableReason: String?
    @State private var sendFailed = false
    /// Why the last message was not sent (the engine's words), when it said.
    @State private var sendRefusal: String?

    init(session: SessionState, monitor: ClaudeSessionMonitor, state: ClaudePanelState,
         canFocus: Bool, account: AccountTagModel?, hooks: ChatHooks, statusLine: ChatStatusLine? = nil) {
        self.session = session
        self.statusLine = statusLine
        self.monitor = monitor
        self.state = state
        self.canFocus = canFocus
        self.account = account
        self.hooks = hooks
        // From the cache when this session was opened before: no loading flash.
        let cached = ChatHistoryManager.shared.history(for: session.sessionId)
        _history = State(initialValue: cached)
        _isLoading = State(initialValue: cached.isEmpty && !ChatHistoryManager.shared.isLoaded(sessionId: session.sessionId))
    }

    var body: some View {
        ChatContent(
            session: session,
            history: history,
            isLoading: isLoading,
            canFocus: canFocus,
            messageRoute: messageRoute,
            account: account,
            agentDescriptions: agentDescriptions,
            sendFailure: sendFailed ? (sendRefusal ?? ChatComposerCopy.failure(for: messageRoute)) : nil,
            state: state,
            hooks: hooks,
            onSend: send,
            unavailableReason: unavailableReason,
            statusLine: statusLine
        )
        .task {
            // Opening the chat counts as reviewing the latest result.
            monitor.markReviewed(sessionId: session.sessionId)
            // Every time the chat appears, not only the first: leaving it
            // releases the history and drops the chat from the manager, so a
            // view that comes back must register again or it stops following.
            // Already loaded is a cheap no-op there.
            await ChatHistoryManager.shared.loadFromFile(sessionId: session.sessionId, cwd: session.cwd)
            history = ChatHistoryManager.shared.history(for: session.sessionId)
            isLoading = false
        }
        // Leaving the chat (back to the list, another session, the panel
        // closing) frees its full history now rather than when two others
        // have been opened.
        .onDisappear { ChatHistoryManager.shared.chatClosed(sessionId: session.sessionId) }
        // This session's history only, and only when it changed: other
        // sessions' updates don't redraw this chat.
        .onReceive(ChatHistoryManager.shared.$histories
            .map { [id = session.sessionId] in $0[id] }
            .removeDuplicates()
        ) { items in
            guard let items else { return }
            history = items
            if isLoading && !items.isEmpty { isLoading = false }
        }
        .onReceive(ChatHistoryManager.shared.$agentDescriptions
            .map { [id = session.sessionId] in $0[id] ?? [:] }
            .removeDuplicates()
        ) { agentDescriptions = $0 }
        .onChange(of: session.isReadyForReview) { _, isReady in
            // Finished while the user is looking at it: already reviewed.
            if isReady && state.isPresented { monitor.markReviewed(sessionId: session.sessionId) }
        }
        .task(id: ChatRouteKey(session)) {
            switch await SessionMessenger.availability(for: session) {
            case .available(let route):
                messageRoute = route
                unavailableReason = nil
            case .unavailable(let reason):
                messageRoute = nil
                unavailableReason = reason
            }
        }
    }

    private func send(_ text: String) {
        sendFailed = false
        sendRefusal = nil
        let target = session
        Task {
            let delivery = await SessionMessenger.deliver(text, to: target)
            switch delivery {
            case .delivered:
                return
            case .typedNotSubmitted(let reason):
                // The text is in Claude's prompt already; keeping the draft
                // too would send it twice.
                sendRefusal = ChatComposerCopy.notSubmitted(reason)
            case .refused(let reason):
                sendRefusal = ChatComposerCopy.refused(reason)
                fallthrough
            case .failed:
                // Keep what was typed.
                if (state.drafts[target.sessionId] ?? "").isEmpty { state.drafts[target.sessionId] = text }
            }
            sendFailed = true
        }
    }
}

/// What decides how to reach a session's terminal.
private struct ChatRouteKey: Hashable {
    let pid: Int?
    let tty: String?
    let isInTmux: Bool
    /// A dialog is open in the terminal: typing is refused until it closes,
    /// so the answer is asked again when it does.
    let hasDialog: Bool

    init(_ session: SessionState) {
        pid = session.pid
        tty = session.tty
        isInTmux = session.isInTmux
        hasDialog = session.attention.needsInputReason.map { !$0.isError } ?? false
    }
}

/// How the chat reaches the panel's commands: key-router commands for
/// approvals and navigation, and full answers to a question.
struct ChatHooks {
    var perform: (ClaudeKeyRouter.Command) -> Void = { _ in }
    var submitAnswers: (_ toolUseId: String, _ answers: [String: String]) -> Void = { _, _ in }
}

enum ChatComposerCopy {
    static func refused(_ reason: String) -> String {
        "Not sent: \(reason). Your message is kept."
    }

    static func notSubmitted(_ reason: String) -> String {
        "Typed but not submitted: \(reason). Press Return in the terminal when it's safe."
    }

    static func failure(for route: MessageRoute?) -> String {
        switch route {
        case .scripted(let terminal):
            return "Couldn't send to \(terminal.displayName). Allow Automation for it in System Settings › Privacy & Security, or type in the terminal."
        case .tmux, .none:
            return "Couldn't reach the session's tmux pane. Type in the terminal instead."
        }
    }
}

// MARK: - Content

struct ChatContent: View {
    let session: SessionState
    let history: [ChatHistoryItem]
    let isLoading: Bool
    let canFocus: Bool
    let messageRoute: MessageRoute?
    let account: AccountTagModel?
    let agentDescriptions: [String: String]
    let sendFailure: String?
    @ObservedObject var state: ClaudePanelState
    let hooks: ChatHooks
    let onSend: (String) -> Void
    /// Why there is no composer, when the engine said (shown in its place).
    var unavailableReason: String? = nil
    /// How the session stands when Claude isn't working: failed, ready for
    /// review, idle. Nil while it works or waits on an answer.
    var statusLine: ChatStatusLine? = nil
    /// Snapshots: open the task board.
    var showsTaskBoard = false
    /// Snapshots: selections already made in a question.
    var initialQuestionSelections: [Int: ChatQuestionSelection] = [:]

    @Environment(\.claudeControlTheme) private var theme
    @Environment(\.claudeStaticRendering) private var isStatic
    @State private var isTaskBoardOpen = false
    @State private var headerHeight: CGFloat = 0
    @State private var messagesHeight: CGFloat = 0
    @State private var barHeight: CGFloat = 0

    private var sessionId: String { session.sessionId }

    var body: some View {
        VStack(spacing: 0) {
            ChatSessionHeader(
                title: session.displayTitle,
                subtitle: subtitle,
                subtitleIsActivity: isWorking && session.tasks.activeItem != nil,
                account: account,
                tasks: session.tasks,
                contextPercent: session.contextUsedPercent,
                canFocus: canFocus,
                focusLabel: session.entrypoint == "claude-vscode" ? "Show in editor" : "Show terminal",
                isTaskBoardOpen: Binding(get: { isTaskBoardOpen || showsTaskBoard }, set: { isTaskBoardOpen = $0 }),
                maxTaskRows: session.activePermission == nil ? ChatTaskBoard.regularRows : ChatTaskBoard.compactRows,
                onBack: { hooks.perform(.back) },
                onFocus: { hooks.perform(.jump(sessionId: sessionId)) }
            )
            .measuredHeight(into: $headerHeight)

            Rectangle().fill(.ink(.separator)).frame(height: theme.hairline)

            messages
                .frame(maxHeight: .infinity)

            Rectangle().fill(.ink(.separator)).frame(height: theme.hairline)

            bottomBar
                .measuredHeight(into: $barHeight)
        }
        .onChange(of: headerHeight + messagesHeight + barHeight, initial: true) { _, total in
            state.reportContentHeight(total + 2 * theme.hairline)
        }
        .onChange(of: session.activePermission?.toolUseId, initial: true) { _, toolUseId in
            state.noteShownRequests(toolUseId.map { [$0] } ?? [], armed: isStatic)
            // A question or plan needs the room an open task board takes.
            if toolUseId != nil { isTaskBoardOpen = false }
        }
    }

    private var isWorking: Bool {
        session.phase == .processing || session.phase == .compacting
    }

    /// The task Claude is on while working, else the project (and
    /// background tasks).
    private var subtitle: String {
        if isWorking, let active = session.tasks.activeItem { return active.activeLabel }
        var parts = [session.displayProjectName]
        if let background = SessionRowContent.backgroundLabel(count: session.backgroundTaskCount) {
            parts.append(background)
        }
        return parts.joined(separator: " · ")
    }

    // MARK: Messages

    private var messages: some View {
        // Equatable: typing in the composer, a request arming or anything
        // else that redraws the chat leaves the transcript alone.
        ChatTranscript(
            history: history,
            isLoading: isLoading,
            workingLabel: isWorking
                ? (session.phase == .compacting ? "Compacting context…" : "Working…")
                : (session.attention == .working ? session.backgroundWaitDescription.map { "Waiting on \($0)…" } : nil),
            agentDescriptions: agentDescriptions,
            // Never beside the working indicator: the line is for when Claude stopped.
            statusLine: isWorking || session.attention == .working ? nil : statusLine,
            onDismiss: { hooks.perform(.markReviewed(sessionId: sessionId)) },
            onHeight: { [messagesHeight = $messagesHeight] height in
                if abs(messagesHeight.wrappedValue - height) > 0.5 { messagesHeight.wrappedValue = height }
            }
        )
        .equatable()
    }

    // MARK: Bottom bar

    @ViewBuilder
    private var bottomBar: some View {
        if let permission = session.activePermission {
            // Redrawn when it arms: this view observes the state's `armingTick`.
            let isArmed = isStatic || state.isAnswerArmed(permission.toolUseId)
            pendingBar(permission)
                .id(permission.toolUseId)
                .allowsHitTesting(isArmed)
                .transition(.opacity)
        } else if let reason = session.attention.needsInputReason, !reason.isError {
            // A dialog in the terminal the panel can't answer. Typing here
            // would land in it, so there is no composer.
            ChatTerminalOnlyBar(
                title: reason.displayText,
                message: "Answer it in the terminal. Anything typed here would go straight into that dialog.",
                canFocus: canFocus,
                onFocus: { hooks.perform(.jump(sessionId: sessionId)) }
            )
        } else if messageRoute == nil {
            ChatTerminalOnlyBar(
                title: nil,
                message: unavailableReason.map { "\($0). Type in the terminal instead." }
                    ?? "Replies can be typed from here for sessions in tmux, iTerm2 and Terminal.",
                canFocus: canFocus,
                onFocus: { hooks.perform(.jump(sessionId: sessionId)) }
            )
        } else {
            ChatComposer(
                draft: Binding(
                    get: { state.drafts[sessionId] ?? "" },
                    set: { state.drafts[sessionId] = $0 }
                ),
                failure: sendFailure,
                isFocused: Binding(get: { state.isComposerFocused }, set: { state.isComposerFocused = $0 }),
                onFocusRequest: state.onRequestKey,
                onSend: { text in
                    state.drafts[sessionId] = nil
                    onSend(text)
                }
            )
        }
    }

    @ViewBuilder
    private func pendingBar(_ permission: PermissionContext) -> some View {
        let toolUseId = permission.toolUseId
        switch permission.toolName {
        case "AskUserQuestion":
            let questions = ChatQuestion.parse(toolInput: permission.toolInput)
            if questions.isEmpty {
                ChatTerminalOnlyBar(
                    title: "Claude has a question",
                    message: "It can't be shown here. Answer it in the terminal.",
                    canFocus: canFocus,
                    onFocus: { hooks.perform(.jump(sessionId: sessionId)) }
                )
            } else {
                ChatQuestionPanel(
                    questions: questions,
                    canFocusTerminal: canFocus,
                    maxQuestionsHeight: 300,
                    initialSelections: initialQuestionSelections,
                    onSubmit: { answers in hooks.submitAnswers(toolUseId, answers) },
                    onGoToTerminal: { hooks.perform(.jump(sessionId: sessionId)) }
                )
            }
        case "ExitPlanMode":
            ChatPlanApprovalBar(
                plan: permission.toolInput?["plan"]?.value as? String,
                maxPlanHeight: 280,
                onApprove: { hooks.perform(.approvePlan(sessionId: sessionId, toolUseId: toolUseId)) },
                onKeepPlanning: { hooks.perform(.keepPlanning(sessionId: sessionId, toolUseId: toolUseId)) }
            )
        default:
            ChatApprovalBar(
                tool: permission.toolName,
                request: PermissionPreview.text(toolName: permission.toolName, toolInput: permission.toolInput,
                                                home: AccountPaths.homeDirectory) ?? permission.formattedInput,
                alwaysAllow: SessionRowContent.alwaysAllowOffer(permission.permissionSuggestions),
                onApprove: { hooks.perform(.allow(sessionId: sessionId, toolUseId: toolUseId)) },
                onAlwaysAllow: { hooks.perform(.alwaysAllow(sessionId: sessionId, toolUseId: toolUseId)) },
                onDeny: { hooks.perform(.deny(sessionId: sessionId, toolUseId: toolUseId)) }
            )
        }
    }
}

// MARK: - Transcript

/// The conversation, newest at the bottom. A long session holds thousands
/// of items, each drawn eagerly (the chat measures its height for the
/// window), so it opens on the newest `pageSize` and grows by a page on
/// "Show earlier messages". Items arriving while it is open are added
/// below; the oldest shown stays put, so nothing shifts under a reader who
/// scrolled up.
struct ChatTranscript: View, Equatable {
    let history: [ChatHistoryItem]
    let isLoading: Bool
    /// "Working…" or "Compacting context…" under the last message; nil when idle.
    let workingLabel: String?
    let agentDescriptions: [String: String]
    /// Under the last message when Claude is not working.
    var statusLine: ChatStatusLine? = nil
    var onDismiss: () -> Void = {}
    /// The laid-out height of the messages (not of the scroll view).
    let onHeight: (CGFloat) -> Void

    /// Items drawn when the chat opens, and added per "Show earlier messages".
    nonisolated static let pageSize = 150

    static func == (lhs: ChatTranscript, rhs: ChatTranscript) -> Bool {
        lhs.isLoading == rhs.isLoading && lhs.workingLabel == rhs.workingLabel
            && lhs.statusLine == rhs.statusLine
            && lhs.agentDescriptions == rhs.agentDescriptions && lhs.history == rhs.history
    }

    @Environment(\.claudeControlTheme) private var theme
    @Environment(\.claudeStaticRendering) private var isStatic
    /// The oldest item shown; nil until the transcript first has items.
    @State private var firstShownId: String?

    var body: some View {
        if isLoading {
            ChatPlaceholder(kind: .loading)
                .measuredHeight(onHeight)
        } else if history.isEmpty && workingLabel == nil && statusLine?.glyph != .error {
            // "Idle · last active…" alone would read as a broken chat; a
            // failure is worth showing even with nothing above it.
            ChatPlaceholder(kind: .empty)
                .measuredHeight(onHeight)
        } else {
            let start = Self.startIndex(in: history, firstShownId: firstShownId)
            let list = VStack(alignment: .leading, spacing: 12) {
                if start > 0 {
                    EarlierMessagesButton(count: min(start, Self.pageSize)) {
                        firstShownId = history[Self.earlierStart(from: start)].id
                    }
                }
                ForEach(history[start...]) { item in
                    MessageItemView(item: item, agentDescriptions: agentDescriptions)
                }
                if let workingLabel {
                    WorkingIndicator(label: workingLabel)
                } else if let statusLine {
                    ChatStatusRow(line: statusLine, onDismiss: onDismiss)
                }
            }
            .padding(.horizontal, theme.padding)
            .padding(.vertical, theme.padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .measuredHeight(onHeight)
            .onChange(of: WindowKey(first: history.first?.id, count: history.count), initial: true) { _, _ in
                // Pin the window's start once there is something to show,
                // and again if the history was replaced (cleared, reloaded).
                if firstShownId == nil || !history.contains(where: { $0.id == firstShownId }) {
                    firstShownId = history.isEmpty ? nil : history[Self.startIndex(in: history, firstShownId: nil)].id
                }
            }

            if isStatic {
                // The newest messages, as the live chat opens on them.
                list
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(minHeight: 0, maxHeight: .infinity, alignment: .bottom)
                    .clipped()
            } else {
                BottomAnchoredScroll { list }
            }
        }
    }

    /// Where the drawn items start: at `firstShownId` when it is still in
    /// the history, else a page from the end.
    nonisolated static func startIndex(in history: [ChatHistoryItem], firstShownId: String?) -> Int {
        if let firstShownId, let index = history.firstIndex(where: { $0.id == firstShownId }) {
            return index
        }
        return max(0, history.count - pageSize)
    }

    /// The start after "Show earlier messages" from `start`.
    nonisolated static func earlierStart(from start: Int) -> Int {
        max(0, start - pageSize)
    }
}

/// What can move the transcript's window: a new history, or items added.
private struct WindowKey: Equatable {
    let first: String?
    let count: Int
}

/// "Show 150 earlier messages" at the top of a long transcript.
private struct EarlierMessagesButton: View {
    let count: Int
    let action: () -> Void

    var body: some View {
        Button("Show \(count) earlier message\(count == 1 ? "" : "s")", action: action)
            .buttonStyle(.claude(.quiet, compact: true))
            .frame(maxWidth: .infinity)
    }
}

// MARK: - Scrolling

/// A transcript that opens on its newest message and stays there as
/// messages arrive, unless the reader has scrolled up.
private struct BottomAnchoredScroll<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView(.vertical) {
            content
        }
        .defaultScrollAnchor(.bottom)
        .scrollIndicators(.automatic)
        .scrollBounceBehavior(.basedOnSize)
    }
}

// MARK: - Placeholders

private struct ChatPlaceholder: View {
    enum Kind { case loading, empty }
    let kind: Kind

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        VStack(spacing: 8) {
            if kind == .loading {
                ArcSpinner(color: theme.textSecondary, lineWidth: theme.statusRingStroke)
                    .frame(width: 12, height: 12)
            }
            Text(kind == .loading ? "Loading the conversation…" : "No messages yet")
                .claudeFont(.body)
                .foregroundStyle(.ink(.secondary))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 36)
    }
}

/// "◠ Working…" under the last message while a turn runs. Still: the arc
/// turns on its own layer, and nothing here ticks.
private struct WorkingIndicator: View {
    let label: String

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(spacing: 7) {
            ArcSpinner(color: theme.working, lineWidth: theme.statusRingStroke)
                .frame(width: theme.statusRing, height: theme.statusRing)
            Text(label)
                .claudeFont(.body)
                .foregroundStyle(.ink(.secondary))
        }
        .accessibilityElement(children: .combine)
    }
}

/// "● Rate limited · 5-hour limit resets in 47m" under the last message once
/// Claude has stopped. The text wraps rather than clips: the panel can be narrow.
private struct ChatStatusRow: View {
    let line: ChatStatusLine
    let onDismiss: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            StatusRing(kind: line.glyph)
                .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 4 }
            Text(line.text)
                .claudeFont(.body)
                .foregroundStyle(.ink(line.glyph == .error ? .critical : .secondary))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            if line.canDismiss {
                Button("Dismiss", action: onDismiss)
                    .buttonStyle(.claude(.quiet, compact: true))
                    .help("Dismiss this failure (⌘R)")
            }
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Messages

struct MessageItemView: View {
    let item: ChatHistoryItem
    var agentDescriptions: [String: String] = [:]

    var body: some View {
        switch item.type {
        case .user(let text):
            UserMessageView(text: text)
        case .assistant(let text):
            AssistantMessageView(text: text)
        case .toolCall(let tool):
            ToolCallView(tool: tool, agentDescriptions: agentDescriptions)
        case .thinking(let text):
            ThinkingView(text: text)
        case .image(let block):
            ImageMessageView(image: block)
        case .interrupted:
            Text("Interrupted")
                .claudeFont(.body, weight: .medium)
                .foregroundStyle(.ink(.critical))
        }
    }
}

struct UserMessageView: View {
    let text: String

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            MarkdownText(text, token: .chat)
                .padding(.horizontal, 11)
                .padding(.vertical, 7)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(theme.controlFillHover))
        }
    }
}

struct AssistantMessageView: View {
    let text: String

    var body: some View {
        // Tool-only turns carry no text: draw nothing rather than an empty line.
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            MarkdownText(text, token: .chat)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
    }
}

struct ImageMessageView: View {
    let image: ImageBlock

    @Environment(\.claudeControlTheme) private var theme
    /// Decoded once, off the main thread.
    @State private var decoded: NSImage?

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            if let decoded {
                Image(nsImage: decoded)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: 240, maxHeight: 240)
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            } else {
                Label("Image (\(image.mediaType))", systemImage: "photo")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(theme.controlFill))
            }
        }
        .task(id: image.id) {
            let data = image.base64Data
            decoded = await Task.detached(priority: .userInitiated) {
                Data(base64Encoded: data).flatMap(NSImage.init(data:))
            }.value
        }
    }
}

// MARK: - Tool calls

/// "✓ Bash  npm test · 12 passed", expandable to its result. A running
/// tool's mark is the still arc; one waiting for approval is half a ring in
/// the needs-you colour.
struct ToolCallView: View {
    let tool: ToolCallItem
    var agentDescriptions: [String: String] = [:]

    @Environment(\.claudeControlTheme) private var theme
    @State private var isExpanded = false
    @State private var isHovered = false

    private var hasResult: Bool { tool.result != nil || tool.structuredResult != nil }
    /// Edit shows its diff without asking; containers show their subagent's tools.
    private var canExpand: Bool { !tool.isSubagentContainer && tool.name != "Edit" && hasResult }
    private var showsResult: Bool { tool.name == "Edit" || isExpanded }
    private var isActive: Bool { tool.status == .running || tool.status == .waitingForApproval }

    private var summary: String { ToolCallSummary.text(for: tool, agentDescriptions: agentDescriptions) }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                ToolStatusMark(status: tool.status)
                    .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 3 }
                Text(MCPToolFormatter.formatToolName(tool.name))
                    .claudeFont(.body, weight: .semibold)
                    .foregroundStyle(.ink(tool.status == .error ? .critical : .primary))
                    .fixedSize()
                Text(summary)
                    .claudeFont(.body)
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 0)
                if canExpand && !isActive {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.ink(isHovered ? .primary : .tertiary))
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                }
            }

            if tool.isSubagentContainer && !tool.subagentTools.isEmpty {
                SubagentToolsList(tools: tool.subagentTools)
                    .padding(.leading, 13)
            }
            if showsResult && tool.status != .running && !tool.isSubagentContainer && (hasResult || tool.name == "Edit") {
                ToolResultContent(tool: tool)
                    .padding(.leading, 13)
                    .transition(.opacity)
            }
            if tool.name == "Edit" && tool.status == .running {
                EditInputDiffView(input: tool.input)
                    .padding(.leading, 13)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onHover { isHovered = $0 && canExpand }
        .onTapGesture {
            guard canExpand else { return }
            isExpanded.toggle()
        }
        .claudeAnimation(ClaudeMotion.quick, value: isExpanded)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(canExpand ? .isButton : [])
        .accessibilityHint(canExpand ? (isExpanded ? "Hides the result" : "Shows the result") : "")
    }
}

/// The quiet text after a tool's name: what it worked on, not the tool's
/// name again, and never a bare "Completed" when the input says more.
nonisolated enum ToolCallSummary {
    static func text(for tool: ToolCallItem, agentDescriptions: [String: String] = [:]) -> String {
        if tool.isSubagentContainer && !tool.subagentTools.isEmpty {
            let description = tool.input["description"] ?? "Running an agent"
            return "\(description) · \(tool.subagentTools.count) tool\(tool.subagentTools.count == 1 ? "" : "s")"
        }
        if tool.name == "AgentOutputTool", let agentId = tool.input["agentId"], let description = agentDescriptions[agentId] {
            return tool.input["block"] == "true" ? "Waiting: \(description)" : description
        }
        if MCPToolFormatter.isMCPTool(tool.name) && !tool.input.isEmpty {
            return MCPToolFormatter.formatArgs(tool.input)
        }
        let status = tool.statusDisplay.text
        if tool.status == .success && (status == "Completed" || status.isEmpty), let subject = subject(of: tool.input) {
            return subject
        }
        // "Read Read redirect.ts (5+ lines)" says the name twice.
        if status.hasPrefix(tool.name + " ") {
            return String(status.dropFirst(tool.name.count + 1))
        }
        return status
    }

    /// What the input is about: its description, file, command, pattern,
    /// query or address.
    static func subject(of input: [String: String]) -> String? {
        func value(_ key: String) -> String? {
            input[key].flatMap { $0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : $0 }
        }
        if let description = value("description") { return description }
        if let path = value("file_path") ?? value("notebook_path") ?? value("path") {
            return URL(fileURLWithPath: path).lastPathComponent
        }
        if let command = value("command") {
            return command.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init)
        }
        return value("pattern") ?? value("query") ?? value("url")
    }
}

/// A tool's state as a small ring, in the same vocabulary as the rows.
private struct ToolStatusMark: View {
    let status: ToolStatus

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        Group {
            switch status {
            case .running:
                ArcSpinner(color: theme.working, lineWidth: 1.3)
            case .waitingForApproval:
                StatusArc(trim: 0.5)
                    .stroke(theme.needsYou, style: StrokeStyle(lineWidth: 1.3, lineCap: .round))
                    .padding(0.65)
            case .success:
                StatusArc(trim: 1)
                    .stroke(theme.textSecondary, style: StrokeStyle(lineWidth: 1.3, lineCap: .round))
                    .padding(0.65)
            case .error, .interrupted:
                Circle().fill(theme.critical).padding(1)
            }
        }
        .frame(width: 7, height: 7)
        .accessibilityLabel(status.spoken)
    }
}

extension ToolStatus {
    var spoken: String {
        switch self {
        case .running: return "Running"
        case .waitingForApproval: return "Waiting for approval"
        case .success: return "Done"
        case .error: return "Failed"
        case .interrupted: return "Interrupted"
        }
    }
}

/// The last two tools a subagent ran, and how many before them.
struct SubagentToolsList: View {
    let tools: [SubagentToolCall]

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if tools.count > 2 {
                Text("+\(tools.count - 2) earlier tool uses")
                    .claudeFont(.caption)
                    .foregroundStyle(.ink(.secondary))
            }
            ForEach(tools.suffix(2)) { tool in
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    ToolStatusMark(status: tool.status)
                        .scaleEffect(0.8)
                    Text(tool.name)
                        .claudeFont(.caption, weight: .semibold)
                        .foregroundStyle(.ink(.primary))
                    Text(tool.status == .interrupted ? "Interrupted" : ToolStatusDisplay.running(for: tool.name, input: tool.input).text)
                        .claudeFont(.caption)
                        .foregroundStyle(.ink(.secondary))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
        }
    }
}

/// Claude's thinking: one quiet line, the rest on a click.
struct ThinkingView: View {
    let text: String

    @State private var isExpanded = false

    private static let previewLength = 90
    private var canExpand: Bool { text.count > Self.previewLength }

    var body: some View {
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(isExpanded || !canExpand ? text : String(text.prefix(Self.previewLength)) + "…")
                    .claudeFont(.body)
                    .italic()
                    .foregroundStyle(.ink(.secondary))
                    .lineLimit(isExpanded ? nil : 1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if canExpand {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.ink(.tertiary))
                        .rotationEffect(.degrees(isExpanded ? 90 : 0))
                }
            }
            .contentShape(Rectangle())
            .onTapGesture { if canExpand { isExpanded.toggle() } }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Thinking: \(text)")
        }
    }
}
