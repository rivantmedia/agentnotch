//
//  ClaudeSessionsPanel.swift
//  ClaudeControl
//
//  The sessions panel's content, wired to the engine. The app hosts it in
//  its own NSPanel (ClaudePanelController) and draws the chrome around it
//  (Codenotch's tooltip silhouette, glass or solid), so this view paints no
//  background of its own.
//

import AppKit
import SwiftUI

public struct ClaudeSessionsPanel: View {
    @ObservedObject private var hub: ClaudeControlHub
    @ObservedObject private var state: ClaudePanelState
    @ObservedObject private var monitor = ClaudeSessionMonitor.shared

    /// Sessions whose terminal can be brought to the front; worked out when
    /// the sessions' processes change, never while drawing a row.
    @State private var focusable: Set<String> = []
    /// Bumped when the gear menu changes a setting, to read it back.
    @State private var settingsRevision = 0

    public init(hub: ClaudeControlHub, state: ClaudePanelState) {
        self.hub = hub
        self.state = state
    }

    public var body: some View {
        let model = self.model
        SessionsPanelContent(
            model: model,
            state: state,
            actions: LivePanelActions.make(hub: hub, state: state, settingsChanged: { settingsRevision &+= 1 }),
            chat: { session, hooks in
                LiveChatView(session: session, monitor: monitor, state: state,
                             canFocus: model.focusable.contains(session.sessionId),
                             account: Self.chatAccount(for: session, in: model), hooks: hooks)
            }
        )
        .task(id: FocusKey(monitor.instances, isPresented: state.isPresented)) {
            // Again shortly after (a terminal host is looked up in the
            // background the first time), then now and then while the panel
            // is on screen (terminals and editors come and go).
            var pause = FocusKey.settleDelay
            while !Task.isCancelled {
                let current = Set(monitor.instances.filter { SessionFocusService.shared.canFocus($0) }.map(\.sessionId))
                if current != focusable { focusable = current }
                guard state.isPresented else { return }
                try? await Task.sleep(for: pause)
                pause = FocusKey.recheckInterval
            }
        }
    }

    private var model: SessionsPanelModel {
        _ = settingsRevision
        var model = SessionsPanelModel(sessions: monitor.instances, accounts: hub.accounts)
        model.readings = hub.ringReadings
        model.setup = hub.setup
        model.focusable = focusable
        model.home = AccountPaths.homeDirectory
        model.showsSealedBadge = SealedMode.isOn
        model.forgottenAccountIds = AccountRegistry.shared.forgottenIds
        model.hookHealth = HookHealth.make(accounts: hub.accounts, labels: model.accountLabels,
                                           hooksEnabled: ClaudeControlSettings.hooksEnabled)
        model.hookHealth.controlOff = !SealedMode.isOn && !ClaudeControlSettings.hooksEnabled
            && !hub.setup.needsHookConsent
        model.quickSettings = PanelQuickSettings(
            autoOpen: ClaudeControlSettings.autoOpen,
            notifyNeedsInput: ClaudeControlSettings.notifyNeedsInput,
            notifyReadyForReview: ClaudeControlSettings.notifyReadyForReview
        )
        model.consentFiles = hub.consentFileLines(home: model.home)
        model.consentScope = hub.consentScope.sentence
        model.takeoverCleanupFiles = hub.takeoverCleanupFiles(home: model.home)
        model.takeoverCleansStores = !model.takeoverCleanupFiles.isEmpty
        // Where each session is shown: the hub's rings (a ~/.claude session
        // stays with the account it started as while the extension mirrors).
        model.sessionRings = Dictionary(hub.sessions.map { ($0.id, $0.ringID) }, uniquingKeysWith: { first, _ in first })
        return model
    }

    /// The account tag in the chat header, only when several are in use.
    private static func chatAccount(for session: SessionState, in model: SessionsPanelModel) -> AccountTagModel? {
        guard model.showsAccounts, let account = model.account(for: session) else { return nil }
        return AccountTagModel(label: model.accountLabels[account.id] ?? account.label, colorIndex: account.colorIndex)
    }
}

/// What decides whether a session can be focused: its process and terminal,
/// and whether the panel is on screen to show it.
private struct FocusKey: Equatable {
    struct Entry: Equatable {
        let id: String
        let pid: Int?
        let tty: String?
        let isInTmux: Bool
        let entrypoint: String?
    }

    let entries: [Entry]
    let isPresented: Bool

    /// The second look, once a first-time host lookup has had time to land.
    static let settleDelay: Duration = .seconds(1.5)
    /// Later looks while the panel stays open.
    static let recheckInterval: Duration = .seconds(30)

    init(_ sessions: [SessionState], isPresented: Bool) {
        entries = sessions.map {
            Entry(id: $0.sessionId, pid: $0.pid, tty: $0.tty, isInTmux: $0.isInTmux, entrypoint: $0.entrypoint)
        }
        self.isPresented = isPresented
    }
}

// MARK: - Engine actions

/// The panel's actions carried out by the engine.
enum LivePanelActions {
    static func make(hub: ClaudeControlHub, state: ClaudePanelState, settingsChanged: @escaping () -> Void) -> SessionsPanelActions {
        let monitor = ClaudeSessionMonitor.shared
        return SessionsPanelActions(
            openChat: { state.showChat(sessionId: $0) },
            focus: { sessionId in
                Task {
                    if await hub.focus(sessionId: sessionId) { state.onJumped() }
                }
            },
            // Each answer names the request the row was drawn for; the engine
            // drops it if that request is no longer the pending one.
            approve: { sessionId, toolUseId, always in
                monitor.approvePermission(sessionId: sessionId, toolUseId: toolUseId, alwaysAllow: always)
            },
            deny: { sessionId, toolUseId in
                monitor.denyPermission(sessionId: sessionId, toolUseId: toolUseId, reason: nil)
            },
            keepPlanning: { sessionId, toolUseId in
                monitor.denyPermission(sessionId: sessionId, toolUseId: toolUseId, reason: PlanApprovalCopy.keepPlanningReason)
            },
            answer: { sessionId, toolUseId, answers in
                monitor.answerQuestion(sessionId: sessionId, toolUseId: toolUseId, answers: answers)
            },
            markReviewed: { ids, at in
                for id in ids {
                    hub.markReviewed(sessionId: id, at: at)
                    hub.dismissFailure(sessionId: id, at: at)
                }
            },
            // The hub does nothing in a sealed run (fixtures only).
            turnOnHooks: { _ = hub.grantHookConsent() },
            declineHooks: { hub.declineHookConsent() },
            acknowledgeScope: { hub.acknowledgeInstallScope() },
            turnOffHooks: { hub.turnOffAfterScopeNotice() },
            quitVibeNotch: { hub.quitVibeNotch() },
            openSettings: { state.onOpenSettings() },
            setQuickSettings: { settings in
                ClaudeControlSettings.autoOpen = settings.autoOpen
                ClaudeControlSettings.notifyNeedsInput = settings.notifyNeedsInput
                ClaudeControlSettings.notifyReadyForReview = settings.notifyReadyForReview
                settingsChanged()
            }
        )
    }
}

