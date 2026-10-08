//
//  ClaudeControlSnapshotSheets.swift
//  ClaudeControl
//
//  Fixture-backed views for the ClaudeControlSnapshots tool and the app's
//  `--snapshot-claude`: the sessions panel in every state it can be in, the
//  chat with each of its bottom bars, and the settings pane. Static: no
//  animation, a fixed clock, fields drawn as text, answers armed.
//

import SwiftUI

@_spi(Snapshots) public enum ClaudeControlSnapshotSheets {
    /// One picture to render.
    public struct Sheet {
        /// How tall to render the sheet.
        public enum Height {
            /// As tall as the content wants (the whole list, unscrolled).
            case fitting
            /// What the panel window would be: the height the content
            /// reports to its `ClaudePanelState`, within [220, cap].
            case reported(ClaudePanelState)
            case fixed(CGFloat)
        }

        public let name: String
        public let width: CGFloat
        public let height: Height
        public let view: AnyView
    }

    /// The panel widths: a side edge's list, and the top edge's list or chat.
    public static let widths: [CGFloat] = [400, 520]

    /// Every sheet at every width.
    public static func all() -> [Sheet] {
        widths.flatMap { width in panelSheets(width: width) + chatSheets(width: width) + settingsSheets(width: width) }
    }

    /// The panel with every attention state, two accounts, at `width`, with
    /// no background of its own (the caller draws the chrome).
    public static func sessionList(width: CGFloat) -> some View {
        panel(UIFixtures.everyState(), accounts: UIFixtures.accounts(), width: width)
            .claudeStaticRendering()
            .environment(\.colorScheme, .dark)
    }

    // MARK: - Panel

    static func panelSheets(width: CGFloat) -> [Sheet] {
        let accounts = UIFixtures.accounts()
        var sheets: [Sheet] = []
        func add(_ name: String, height: Sheet.Height = .fitting, _ view: some View) {
            sheets.append(Sheet(name: name, width: width, height: height, view: AnyView(card(view))))
        }

        add("panel-every-state", panel(UIFixtures.everyState(), accounts: accounts, width: width))
        add("panel-regular-rows", panel(UIFixtures.regular(), accounts: accounts, width: width))
        add("panel-needs-you", panel(UIFixtures.needsYou(), accounts: accounts, width: width, hovered: "needs-plan"))

        let windowState = state(width: width)
        add("panel-busy-window", height: .reported(windowState),
            panel(UIFixtures.density(), accounts: UIFixtures.collidingAccounts(), width: width, state: windowState)
                .environment(\.claudeStaticKeepsScrolling, true))
        add("panel-busy-full", panel(UIFixtures.density(), accounts: UIFixtures.collidingAccounts(), width: width))

        let filtered = state(width: width, ringFilter: UIFixtures.ringID(UIFixtures.work))
        add("panel-filtered", panel(UIFixtures.density(), accounts: UIFixtures.collidingAccounts(), width: width, state: filtered))

        let selected = state(width: width)
        selected.highlightedSessionId = "needs-question"
        selected.sectionFolds = [.working: true]
        add("panel-keyboard-folded", panel(UIFixtures.everyState(), accounts: accounts, width: width, state: selected))

        var consent = SessionsPanelModel(sessions: [], accounts: accounts)
        consent.setup = ClaudeSetupState(needsHookConsent: true, legacyHooksFound: true,
                                         superpoweredVibeNotchHooksFound: true, vibeNotchHooksFound: true)
        // Claude Parallel Profiles: ~/.claude and three VS Code windows get
        // hooks; the stores and the shared history only lose Superpowered
        // Vibe Notch's leftovers.
        consent.consentFiles = UIFixtures.consentFiles
        consent.consentScope = ConsentScope(includesDefault: true, windowCount: 3, storeCount: 3, parallelProfiles: true).sentence
        consent.takeoverCleansStores = true
        consent.takeoverCleanupFiles = UIFixtures.cleanupFiles
        add("panel-consent", panel(model: consent, width: width))

        // A yes given to an earlier build: the one-time notice (S2).
        var scopeNotice = SessionsPanelModel(sessions: UIFixtures.regular(), accounts: accounts)
        scopeNotice.readings = UIFixtures.readings()
        scopeNotice.setup = UIFixtures.settingsScopeNotice().setup
        add("panel-scope-notice", panel(model: scopeNotice, width: width))

        var blocked = SessionsPanelModel(sessions: UIFixtures.regular(), accounts: accounts)
        blocked.readings = UIFixtures.readings()
        blocked.setup = ClaudeSetupState(vibeNotchRunning: true, socketError: "The hook socket couldn't be opened (address in use).")
        var withoutHooks = accounts
        withoutHooks[1].hooks = ClaudeHookStatus()
        blocked.accounts = withoutHooks
        blocked.hookHealth = HookHealth.make(accounts: withoutHooks, labels: blocked.accountLabels, hooksEnabled: true)
        add("panel-banners", panel(model: blocked, width: width))

        let undo = state(width: width)
        undo.markReviewedWithUndo(["review-darkmode", "review-tests"], now: UIFixtures.now, schedulesCommit: false) { _ in }
        add("panel-undo", panel(UIFixtures.regular() + SampleSessions.review(), accounts: accounts, width: width, state: undo))

        add("panel-empty", panel([], accounts: [accounts[0]], width: width))
        return sheets
    }

    private static func state(width: CGFloat, ringFilter: String? = nil) -> ClaudePanelState {
        let state = ClaudePanelState(route: .sessions(ringID: ringFilter))
        state.contentWidth = width
        return state
    }

    private static func panel(_ sessions: [SessionState], accounts: [ClaudeAccountSummary], width: CGFloat,
                              hovered: String? = nil, state: ClaudePanelState? = nil) -> some View {
        var model = SessionsPanelModel(sessions: sessions, accounts: accounts)
        model.readings = UIFixtures.readings()
        return panel(model: model, width: width, hovered: hovered, state: state)
    }

    private static func panel(model: SessionsPanelModel, width: CGFloat, hovered: String? = nil,
                              state: ClaudePanelState? = nil) -> some View {
        var model = model
        model.now = UIFixtures.now
        model.home = UIFixtures.home
        model.showsSealedBadge = true
        model.focusable = Set(model.sessions.map(\.sessionId))
        let panelState = state ?? Self.state(width: width)
        panelState.contentWidth = width
        return SessionsPanelContent(model: model, state: panelState, actions: SessionsPanelActions(),
                                    chat: { _, _, _ in EmptyView() }, hoveredSessionId: hovered)
    }

    // MARK: - Chat

    static func chatSheets(width: CGFloat) -> [Sheet] {
        let height: Sheet.Height = .fixed(640)
        var sheets: [Sheet] = []
        func add(_ name: String, _ session: SessionState, history: [ChatHistoryItem]? = nil,
                 route: MessageRoute? = .tmux, draft: String = "",
                 selections: [Int: ChatQuestionSelection] = [:], taskBoard: Bool = false) {
            let state = ClaudePanelState(route: .session(id: session.sessionId))
            state.contentWidth = width
            if !draft.isEmpty { state.drafts[session.sessionId] = draft }
            let view = ChatContent(
                session: session, history: history ?? UIFixtures.chatHistory(for: session), isLoading: false, canFocus: true, messageRoute: route,
                account: chatAccount(for: session), agentDescriptions: [:], sendFailure: nil,
                state: state, hooks: ChatHooks(), onSend: { _ in },
                statusLine: statusLine(for: session),
                showsTaskBoard: taskBoard, initialQuestionSelections: selections
            )
            .frame(width: width)
            sheets.append(Sheet(name: name, width: width, height: height, view: AnyView(card(view))))
        }

        // Its own session and story (CS-14): "Speed up the test suite", 4/4 · 71%.
        let finished = UIFixtures.reviewDone()
        add("chat-composer", finished, draft: "Great, do the same for the e2e suite")
        add("chat-tasks", SampleSessions.working()[0], taskBoard: true)
        add("chat-approval", SampleSessions.permission())
        var other = ChatQuestionSelection()
        other.toggleOther(multiSelect: false)
        other.otherText = "Victory, it matches our design system"
        add("chat-question-other", SampleSessions.question(), selections: [0: other])
        add("chat-plan", planSession())
        add("chat-terminal-only", UIFixtures.dialog())
        add("chat-no-route", finished, route: nil)
        // The turn failed on the account's limit: the line says when it lifts.
        add("chat-rate-limited", SampleSessions.rateLimited())
        return sheets
    }

    /// The chat's status line, as the panel works it out for the session.
    private static func statusLine(for session: SessionState) -> ChatStatusLine? {
        var model = SessionsPanelModel(sessions: [session], accounts: UIFixtures.accounts())
        model.readings = UIFixtures.readings()
        model.home = UIFixtures.home
        return ChatStatusLine.make(for: session, rateLimit: model.rateLimit(for: session, now: UIFixtures.now),
                                   now: UIFixtures.now, home: UIFixtures.home)
    }

    /// The session's own account, named as the live chat names it.
    private static func chatAccount(for session: SessionState) -> AccountTagModel? {
        let model = SessionsPanelModel(sessions: [session], accounts: UIFixtures.accounts())
        guard model.showsAccounts, let account = model.account(for: session) else { return nil }
        return AccountTagModel(label: model.accountLabels[account.id] ?? account.label, colorIndex: account.colorIndex)
    }

    private static func planSession() -> SessionState {
        var session = SampleSessions.plan()
        session.phase = .waitingForApproval(PermissionContext(
            toolUseId: "toolu_sample_plan",
            toolName: "ExitPlanMode",
            toolInput: ["plan": AnyCodable("""
            ## Move settings to SwiftData

            1. Add `SettingsRecord` and `ProfileRecord` models.
            2. Migrate existing values from `UserDefaults` on first launch, **once**.
            3. Read and write through the new store; keep a fallback for one release.
            4. Remove the `UserDefaults` keys in the release after.

            Risks: the migration must be idempotent if the app quits halfway.
            """)],
            receivedAt: UIFixtures.now.addingTimeInterval(-11 * 60)
        ))
        return session
    }

    // MARK: - Settings

    static func settingsSheets(width: CGFloat) -> [Sheet] {
        [
            // Narrower wraps more lines: at 400 the pane runs past 3400.
            Sheet(name: "settings", width: width, height: .fixed(width < 480 ? 3500 : 3150),
                  view: AnyView(settings(UIFixtures.settings()))),
            Sheet(name: "settings-first-run", width: width, height: .fixed(1000),
                  view: AnyView(settings(UIFixtures.settingsFirstRun(), newAccount: .naming("research")))),
            // Claude Parallel Profiles: every folder of each account listed,
            // and "New account…" pointing to VS Code.
            Sheet(name: "settings-parallel-profiles", width: width, height: .fixed(width < 480 ? 1600 : 1400),
                  view: AnyView(settings(UIFixtures.settings(), newAccount: .parallelProfilesGuidance, expandsFolders: true))),
            // The same accounts without names of their own: the default rule.
            Sheet(name: "settings-unnamed-accounts", width: width, height: .fixed(width < 480 ? 1300 : 1150),
                  view: AnyView(settings(UIFixtures.settingsUnnamed(), expandsFolders: true))),
            // A yes given to an earlier build: what it covers now, once.
            Sheet(name: "settings-scope-notice", width: width, height: .fixed(700),
                  view: AnyView(settings(UIFixtures.settingsScopeNotice()))),
            // The Cloud section alone: the build's website, signed out; a
            // build with no website; a development run's AGENTNOTCH_WEB_URL;
            // and signed in with sync on and summaries off. The website is
            // shown, never edited.
            Sheet(name: "settings-cloud-signed-out", width: width, height: .fixed(width < 480 ? 220 : 200),
                  view: AnyView(cloudSettings(UIFixtures.cloudSignedOut()))),
            Sheet(name: "settings-cloud-no-website", width: width, height: .fixed(width < 480 ? 200 : 180),
                  view: AnyView(cloudSettings(UIFixtures.cloudNoWebsite()))),
            Sheet(name: "settings-cloud-overridden", width: width, height: .fixed(width < 480 ? 240 : 220),
                  view: AnyView(cloudSettings(UIFixtures.cloudOverridden()))),
            Sheet(name: "settings-cloud-signed-in", width: width, height: .fixed(width < 480 ? 620 : 530),
                  view: AnyView(cloudSettings(UIFixtures.cloudSignedIn()))),
        ]
    }

    /// The Cloud section in a pane of its own, styled as `settings` styles it.
    private static func cloudSettings(_ cloud: ClaudeCloudState) -> some View {
        Form {
            CloudSection(cloud: cloud, readsDesktopUsage: true, now: UIFixtures.now, actions: SettingsPaneActions())
        }
        .formStyle(.grouped)
        .buttonStyle(.claude(.secondary))
        .claudeControlTheme(.codenotchDark)
        .environment(\.colorScheme, .dark)
    }

    private static func settings(_ model: SettingsPaneModel, newAccount: NewAccountForm.Step = .closed,
                                 expandsFolders: Bool = false) -> some View {
        SettingsPaneContent(model: model, actions: SettingsPaneActions(), initialNewAccountStep: newAccount,
                            expandsFolders: expandsFolders)
            // What Codenotch's settings window sets over every pane.
            .buttonStyle(.claude(.secondary))
            .claudeControlTheme(.codenotchDark)
            .environment(\.colorScheme, .dark)
    }

    // MARK: - Chrome

    /// The black card on a dark desktop, as the panel's chrome draws it.
    private static func card(_ content: some View) -> some View {
        content
            .claudeStaticRendering()
            .claudeControlTheme(.codenotchDark)
            .background(RoundedRectangle(cornerRadius: ClaudeControlTheme.codenotchDark.corner, style: .continuous)
                .fill(ClaudeControlTheme.codenotchDark.card))
            .clipShape(RoundedRectangle(cornerRadius: ClaudeControlTheme.codenotchDark.corner, style: .continuous))
            .environment(\.colorScheme, .dark)
    }
}

extension View {
    /// Draw the resting state: no animation, fields as text, answers armed.
    func claudeStaticRendering() -> some View {
        environment(\.claudeStaticRendering, true)
    }
}
