//
//  SessionsPanelContent.swift
//  ClaudeControl
//
//  The sessions panel itself, the same on every edge: the header (title,
//  attention strip, account chips, pin, gear, close), the setup banners,
//  then the grouped list, or one session's chat. It reports the height it
//  would like at the window's width, turns clicks and keys into one set of
//  commands (`ClaudeKeyRouter.Command`), and never answers a request twice
//  or before it has been on screen for a moment (`AnswerGate`).
//
//  Values in, closures out: `ClaudeSessionsPanel` feeds it from the engine,
//  the snapshots from fixtures.
//

import SwiftUI

struct SessionsPanelContent<Chat: View>: View {
    let model: SessionsPanelModel
    @ObservedObject var state: ClaudePanelState
    let actions: SessionsPanelActions
    /// The chat for a session (live, or from fixtures), and how it reaches
    /// the panel's commands.
    @ViewBuilder let chat: (SessionState, ChatHooks) -> Chat
    /// Snapshots: draw this row with its pointer actions showing.
    var hoveredSessionId: String? = nil

    @Environment(\.claudeControlTheme) private var theme
    @Environment(\.claudeStaticRendering) private var isStatic
    @Environment(\.claudeStaticKeepsScrolling) private var keepsScrolling
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Display order while the pointer is over the list.
    @State private var frozenOrder: [String]?
    @State private var chromeHeight: CGFloat = 0
    @State private var listHeight: CGFloat = 0
    @State private var footerHeight: CGFloat = 0

    var body: some View {
        Group {
            if let now = model.now {
                page(now: now)
            } else {
                // One clock for every elapsed label; spinners turn on their own.
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    page(now: context.date)
                }
            }
        }
        .frame(width: state.contentWidth, alignment: .top)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    // MARK: - Routes

    @ViewBuilder
    private func page(now: Date) -> some View {
        switch state.route {
        case .session(let id):
            if let session = model.sessions.first(where: { $0.sessionId == id }) {
                chat(session, ChatHooks(
                    perform: { perform($0, now: now) },
                    submitAnswers: { toolUseId, answers in
                        answer(toolUseId) { actions.answer(id, toolUseId, answers) }
                    }
                ))
                .id(id)
                .modifier(PanelKeys(context: chatContext(for: session, now: now), perform: { perform($0, now: now) }))
            } else {
                endedSession
            }
        case .sessions, .setup:
            listPage(now: now)
        }
    }

    private var endedSession: some View {
        VStack(alignment: .leading, spacing: theme.blockSpacing) {
            HStack(spacing: 6) {
                ClaudeIconButton(systemName: "chevron.left", label: "Back to sessions (Esc)") { state.back() }
                Text("Session ended")
                    .claudeFont(.title)
                    .foregroundStyle(.ink(.primary))
                Spacer(minLength: 0)
                ClaudeIconButton(systemName: "xmark", label: "Close", action: state.onClose)
            }
            Text("This session has ended or was cleared, so there is nothing more to show.")
                .claudeFont(.body)
                .foregroundStyle(.ink(.secondary))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(theme.padding)
        .onAppear { state.reportContentHeight(ClaudePanelState.minimumContentHeight) }
    }

    // MARK: - List page

    private func listPage(now: Date) -> some View {
        let sessions = displayedSessions
        let layout = listLayout(sessions: sessions, now: now)
        let counts = AttentionCounts(sessions)
        let shownRequests = layout.sections.filter { !$0.isCollapsed }
            .flatMap { $0.rows.compactMap(\.actions.toolUseId) }
        // Worked out here, in the body that observes the state, so the
        // moment a request arms (`armingTick`) its row draws it live.
        let clock = isStatic ? Date.distantFuture : Date()
        let armedRequests = Set(shownRequests.filter { state.isAnswerArmed($0, now: clock) })

        return VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: theme.blockSpacing) {
                PanelHeader(
                    counts: counts,
                    showsSealedBadge: model.showsSealedBadge,
                    chips: ringChips(),
                    ringFilter: $state.ringFilter,
                    isPinned: $state.isPinned,
                    quickSettings: model.quickSettings,
                    canMarkAllReviewed: counts.readyForReview > 0,
                    onQuickSettings: actions.setQuickSettings,
                    onMarkAllReviewed: { markAllReviewed(in: layout) },
                    onOpenSettings: actions.openSettings,
                    onClose: state.onClose
                )
                SetupBanners(
                    setup: model.setup,
                    hookHealth: model.hookHealth,
                    consentFiles: model.consentFiles,
                    consentScope: model.consentScope,
                    takeoverCleansStores: model.takeoverCleansStores,
                    takeoverCleanupFiles: model.takeoverCleanupFiles,
                    didAnswerConsent: state.didAnswerSetup,
                    onTurnOn: {
                        state.didAnswerSetup = true
                        actions.turnOnHooks()
                    },
                    onNotNow: {
                        state.didAnswerSetup = true
                        actions.declineHooks()
                    },
                    onQuitVibeNotch: actions.quitVibeNotch,
                    onOpenSettings: actions.openSettings,
                    onAcknowledgeScope: actions.acknowledgeScope,
                    onTurnOffHooks: actions.turnOffHooks
                )
            }
            .padding(.horizontal, theme.padding)
            .padding(.top, theme.padding)
            .padding(.bottom, theme.blockSpacing)
            .measuredHeight(into: $chromeHeight)

            Rectangle()
                .fill(.ink(.separator))
                .frame(height: theme.hairline)
                .padding(.horizontal, theme.padding)

            listBody(layout: layout, armedRequests: armedRequests, now: now)
                .frame(maxHeight: .infinity, alignment: .top)

            if let pending = state.pendingReview {
                UndoReviewToast(count: pending.sessionIds.count, onUndo: state.undoPendingReview)
                    .padding(.horizontal, theme.padding)
                    .padding(.bottom, theme.padding)
                    .measuredHeight(into: $footerHeight)
                    .transition(.opacity)
            }
        }
        .onChange(of: chromeHeight + listHeight + (state.pendingReview == nil ? 0 : footerHeight), initial: true) { _, total in
            state.reportContentHeight(total + theme.hairline)
        }
        .onChange(of: shownRequests, initial: true) { _, ids in
            state.noteShownRequests(ids, armed: isStatic)
        }
        .onChange(of: state.isPresented) { _, isPresented in
            // A closed panel isn't under the pointer: the next one opens in
            // the natural order, not the one frozen when it was last hovered.
            if !isPresented { frozenOrder = nil }
        }
        .onChange(of: state.highlightedSessionId, initial: true) { _, id in
            // A banner or auto-open points at a row: unfold its section.
            if let bucket = layout.foldedBucket(containing: id) {
                state.sectionFolds[bucket] = false
            }
        }
        .onChange(of: layout.visibleOrder) { _, order in
            // A selection folded away or ended is dropped; the arrows pick
            // up from the top again.
            if let selected = state.selectedSessionId, !order.contains(selected) {
                state.selectedSessionId = nil
            }
        }
        .modifier(PanelKeys(context: .list(selection: state.selectedSessionId.flatMap(layout.row(id:)).map(\.keyTarget)),
                            perform: { perform($0, layout: layout, now: now) }))
        .claudeAnimation(ClaudeMotion.glide, value: layout.visibleOrder)
        .claudeAnimation(ClaudeMotion.quick, value: state.pendingReview)
    }

    @ViewBuilder
    private func listBody(layout: SessionListLayout, armedRequests: Set<String>, now: Date) -> some View {
        if layout.isEmpty {
            SessionsEmptyState(isFiltered: state.ringFilter != nil && !model.sessions.isEmpty)
                .measuredHeight(into: $listHeight)
        } else {
            let list = SessionListView(
                layout: layout,
                selectedId: state.selectedSessionId,
                isArmed: { armedRequests.contains($0) },
                hoveredId: hoveredSessionId,
                onToggleSection: { bucket in
                    let count = layout.sections.first { $0.bucket == bucket }?.rows.count ?? 0
                    state.toggleSection(bucket, count: count)
                },
                onMarkAllReviewed: { markAllReviewed(in: layout) },
                perform: { perform($0, layout: layout, now: now) }
            )
            .padding(.horizontal, theme.padding - 8)
            .padding(.top, theme.blockSpacing - 2)
            .padding(.bottom, theme.padding - 4)
            .measuredHeight(into: $listHeight)
            .onHover { inside in
                frozenOrder = inside ? layout.sections.flatMap { $0.rows.map(\.id) } : nil
            }

            if isStatic && !keepsScrolling {
                list
            } else {
                ScrollViewReader { proxy in
                    ScrollView(.vertical) {
                        list
                    }
                    .scrollIndicators(.automatic)
                    .scrollBounceBehavior(.basedOnSize)
                    .onChange(of: state.selectedSessionId) { _, id in
                        guard let id else { return }
                        withAnimation(ClaudeMotion.animation(ClaudeMotion.quick, reduceMotion: reduceMotion)) {
                            proxy.scrollTo(id)
                        }
                    }
                    .onAppear {
                        if let id = state.highlightedSessionId { proxy.scrollTo(id, anchor: .center) }
                    }
                }
            }
        }
    }

    // MARK: - Model

    /// Sessions of the filtered ring, with a pending "mark all reviewed"
    /// already applied so they leave the review section at once.
    private var displayedSessions: [SessionState] {
        let pending = Set(state.pendingReview?.sessionIds ?? [])
        return model.visibleSessions(ringFilter: state.ringFilter).map { session in
            guard pending.contains(session.sessionId), session.attention == .readyForReview else { return session }
            var reviewed = session
            reviewed.reviewedAt = session.completedAt ?? model.now ?? Date()
            return reviewed
        }
    }

    private func listLayout(sessions: [SessionState], now: Date) -> SessionListLayout {
        let sections = SessionSections.build(sessions, preservingOrder: frozenOrder)
        let labels = model.accountLabels
        // Filtered to one account, every row would name the same one.
        let showsAccounts = model.showsAccounts && state.ringFilter == nil
        return SessionListLayout.make(sections: sections, rows: { session in
            let account = model.account(for: session)
            let tag = showsAccounts
                ? account.map { AccountTagModel(label: labels[$0.id] ?? $0.label, colorIndex: $0.colorIndex) }
                : nil
            return SessionRowModel.make(
                session,
                account: tag,
                rateLimit: model.rateLimit(for: session, now: now),
                canFocus: model.focusable.contains(session.sessionId),
                now: now,
                home: model.home
            )
        }, folds: state.sectionFolds)
    }

    private func ringChips() -> [RingChip] {
        guard model.showsAccounts else { return [] }
        let labels = model.accountLabels
        let all = model.visibleSessions(ringFilter: nil)
        return model.trackedAccounts.map { account in
            let own = all.filter { model.ringID(of: $0) == account.ringID }
            return RingChip(
                ringID: account.ringID,
                label: labels[account.id] ?? account.label,
                colorIndex: account.colorIndex,
                count: own.count,
                needsYouCount: own.filter { $0.attention.bucket == .needsInput && !$0.attention.isError }.count
            )
        }
    }

    private func chatContext(for session: SessionState, now: Date) -> ClaudeKeyRouter.Context {
        let row = SessionRowModel.make(session, account: nil, rateLimit: nil,
                                       canFocus: model.focusable.contains(session.sessionId), now: now, home: model.home)
        return .chat(row.keyTarget, isTyping: state.isComposerFocused)
    }

    // MARK: - Commands

    private func perform(_ command: ClaudeKeyRouter.Command, layout: SessionListLayout? = nil, now: Date) {
        switch command {
        case .moveSelection(let delta):
            guard let layout else { return }
            state.onRequestKey()
            state.selectedSessionId = ClaudeKeyRouter.moved(state.selectedSessionId, by: delta, in: layout.visibleOrder)
        case .openChat(let id):
            state.onRequestKey()
            state.showChat(sessionId: id)
        case let .allow(id, toolUseId):
            answer(toolUseId) { actions.approve(id, toolUseId, false) }
        case let .alwaysAllow(id, toolUseId):
            answer(toolUseId) { actions.approve(id, toolUseId, true) }
        case let .deny(id, toolUseId):
            answer(toolUseId) { actions.deny(id, toolUseId) }
        case let .approvePlan(id, toolUseId):
            answer(toolUseId) { actions.approve(id, toolUseId, false) }
        case let .keepPlanning(id, toolUseId):
            answer(toolUseId) { actions.keepPlanning(id, toolUseId) }
        case let .chooseOption(id, toolUseId, index):
            guard let question = question(sessionId: id, toolUseId: toolUseId),
                  question.options.indices.contains(index) else { return }
            let answers = ChatQuestionAnswers.answers(for: question, choosing: question.options[index].label)
            answer(toolUseId) { actions.answer(id, toolUseId, answers) }
        case .markReviewed(let id):
            actions.markReviewed([id], Date())
        case .markAllReviewed:
            if let layout { markAllReviewed(in: layout) } else { markAllReviewed(in: listLayout(sessions: displayedSessions, now: now)) }
        case .jump(let id):
            actions.focus(id)
        case .back:
            state.back()
        case .close:
            state.onClose()
        }
    }

    /// Send an answer once, and only for a request that has been on screen.
    private func answer(_ toolUseId: String, send: () -> Void) {
        guard state.claimAnswer(toolUseId, now: isStatic ? .distantFuture : Date()) else { return }
        send()
    }

    private func question(sessionId: String, toolUseId: String) -> ChatQuestion? {
        guard let session = model.sessions.first(where: { $0.sessionId == sessionId }),
              case let .questionChips(id, question) = SessionRowContent.primaryActions(for: session, home: model.home),
              id == toolUseId else { return nil }
        return question
    }

    private func markAllReviewed(in layout: SessionListLayout) {
        let ids = layout.sections.first { $0.bucket == .readyForReview }?.rows.map(\.id) ?? []
        state.markReviewedWithUndo(ids, commitAt: actions.markReviewed)
    }
}

// MARK: - Undo

/// "Marked 3 reviewed · Undo", for the few seconds a "mark all reviewed" can
/// still be taken back.
struct UndoReviewToast: View {
    let count: Int
    let onUndo: () -> Void

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        HStack(spacing: 8) {
            StatusRing(kind: .review)
            Text("Marked \(count) reviewed")
                .claudeFont(.body, weight: .medium)
                .foregroundStyle(.ink(.primary))
            Spacer(minLength: 8)
            Button("Undo", action: onUndo)
                .buttonStyle(.claude(.secondary, compact: true))
                .keyboardShortcut("z", modifiers: .command)
                .help("Put them back in Ready for review (⌘Z)")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: theme.rowCorner, style: .continuous).fill(theme.controlFill))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Marked \(count) reviewed")
        .accessibilityAction(named: "Undo", onUndo)
    }
}

// MARK: - Measuring

extension View {
    /// Write this view's laid-out height into `binding`.
    func measuredHeight(into binding: Binding<CGFloat>) -> some View {
        measuredHeight { height in
            if abs(binding.wrappedValue - height) > 0.5 { binding.wrappedValue = height }
        }
    }

    /// Hand this view's laid-out height to `action` whenever it changes.
    func measuredHeight(_ action: @escaping (CGFloat) -> Void) -> some View {
        onGeometryChange(for: CGFloat.self) { proxy in
            proxy.size.height
        } action: { height in
            action(height)
        }
    }
}
